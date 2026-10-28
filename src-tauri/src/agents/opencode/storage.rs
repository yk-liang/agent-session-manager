//! Read-only compatibility projections. TEMP views live on this connection only;
//! main remains SQLITE_OPEN_READ_ONLY. Never migrate the user's OpenCode database.
//! Schema reference: anomalyco/opencode @ 5ed03ad1377b549df83aa52dc11bdb07593a5327,
//! packages/core/src/session/sql.ts and packages/schema/src/session-message.ts.
use rusqlite::Connection;

fn exists(conn: &Connection, name: &str) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM main.sqlite_master WHERE name=?1 AND type IN ('table','view'))",
        [name],
        |r| r.get(0),
    )
}

pub(super) fn order_column(conn: &Connection) -> &'static str {
    if conn
        .prepare("SELECT sort_order FROM temp.message LIMIT 0")
        .is_ok()
    {
        "sort_order"
    } else {
        "time_created"
    }
}

pub(super) fn is_v2_session(conn: &Connection, id: &str) -> bool {
    order_column(conn) == "sort_order"
        && conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM main.session_v2 WHERE id=?1)",
                [id],
                |r| r.get(0),
            )
            .unwrap_or(false)
}

pub(super) fn prepare(conn: &Connection, session: Option<&str>) -> rusqlite::Result<()> {
    if !exists(conn, "session_v2")? {
        return Ok(());
    }
    // Validate the required fields before installing projections; unknown/incomplete
    // schemas must report an error rather than look like an empty session library.
    conn.prepare(
        "SELECT id,session_id,type,seq,time_created,data FROM main.session_message LIMIT 0",
    )?;
    conn.prepare("SELECT id,time_created FROM main.session_v2 LIMIT 0")?;
    // Scope before JSON expansion. A LEFT JOIN can materialize a UNION view;
    // detail/search must never expand the content of unrelated sessions.
    let filter = session
        .map(|id| format!(" WHERE session_id='{}'", id.replace('\'', "''")))
        .unwrap_or_default();
    conn.execute_batch(&format!(
        "CREATE TEMP VIEW _asm_v2_messages AS SELECT * FROM main.session_message{filter}"
    ))?;
    let columns = [
        "id",
        "title",
        "directory",
        "version",
        "time_created",
        "time_updated",
        "parent_id",
        "time_archived",
    ];
    let projection = |table: &str| -> rusqlite::Result<String> {
        let stmt = conn.prepare(&format!("SELECT * FROM main.{table} LIMIT 0"))?;
        let names = stmt.column_names();
        Ok(columns
            .iter()
            .map(|c| {
                if names.contains(c) {
                    c.to_string()
                } else {
                    format!("NULL AS {c}")
                }
            })
            .collect::<Vec<_>>()
            .join(","))
    };
    let mut sessions = format!("SELECT {} FROM main.session_v2", projection("session_v2")?);
    if exists(conn, "session")? {
        sessions.push_str(&format!(" UNION ALL SELECT {} FROM main.session s WHERE NOT EXISTS (SELECT 1 FROM main.session_v2 v WHERE v.id=s.id)", projection("session")?));
    }
    // Keep only metadata in the message projection. Text/tools are projected lazily
    // from content below; usage queries never transfer conversation bodies to Rust.
    let mut messages = String::from("SELECT id,session_id,time_created,seq AS sort_order,
        CASE WHEN json_valid(data) AND json_type(data)='object' THEN json_object(
            'role',type,'system',json_extract(data,'$.system'),'model',json_extract(data,'$.model'),
            'modelID',json_extract(data,'$.model.id'),'providerID',json_extract(data,'$.model.providerID'),
            'time',json_extract(data,'$.time'),'tokens',json_extract(data,'$.tokens'),
            'cost',json_extract(data,'$.cost'),'finish',json_extract(data,'$.finish'),
            '_asm_failed',json_type(data,'$.error') IS NOT NULL AND json_type(data,'$.error') != 'null',
            'agent',json_extract(data,'$.agent'),'mode',json_extract(data,'$.mode'),'path',json_extract(data,'$.path'))
        ELSE '{' END AS data
        FROM temp._asm_v2_messages WHERE type IN ('user','assistant','compaction','synthetic')");
    let legacy_messages = exists(conn, "message")?;
    if legacy_messages {
        conn.execute_batch(&format!(
            "CREATE TEMP VIEW _asm_legacy_messages AS SELECT * FROM main.message{filter}"
        ))?;
        messages.push_str(" UNION ALL SELECT id,session_id,time_created,time_created AS sort_order,data
            FROM temp._asm_legacy_messages m WHERE NOT EXISTS (SELECT 1 FROM main.session_v2 v WHERE v.id=m.session_id)");
    }
    // Array index is the stable part identity and ordering key. Do not sort tool
    // content by timestamp: v2 seq/content order is authoritative even after replay.
    let mut parts = String::from(
        "SELECT id || ':v2:0000000000' AS id,id AS message_id,time_created,
            json_object('type','text','text',json_extract(data,'$.text')) AS data
        FROM temp._asm_v2_messages WHERE type='user' AND json_valid(data)
        UNION ALL
        SELECT m.id || ':v2:' || printf('%010d',p.key),m.id,m.time_created,
            CASE WHEN p.type='object' THEN p.value ELSE '{' END
        FROM temp._asm_v2_messages m, json_each(CASE
            WHEN json_valid(m.data) AND json_type(m.data,'$.content')='array'
            THEN json_extract(m.data,'$.content') ELSE '[]' END) p
        WHERE m.type='assistant'
        UNION ALL
        SELECT id || ':v2:context',id,time_created,
            json_object('type','context','contextKind',type,'record',json(data))
        FROM temp._asm_v2_messages WHERE type IN ('compaction','synthetic') AND json_valid(data)
        UNION ALL
        SELECT id || ':v2:invalid',id,time_created,'{'
        FROM temp._asm_v2_messages WHERE type='assistant' AND json_valid(data)
            AND COALESCE(json_type(data,'$.content'),'null') != 'array'",
    );
    if legacy_messages && exists(conn, "part")? {
        parts.push_str(
            " UNION ALL SELECT p.id,p.message_id,p.time_created,p.data
            FROM main.part p JOIN temp._asm_legacy_messages m ON m.id=p.message_id
            WHERE NOT EXISTS (SELECT 1 FROM main.session_v2 v WHERE v.id=m.session_id)",
        );
    }
    conn.execute_batch(&format!(
        "CREATE TEMP VIEW session AS {sessions};
        CREATE TEMP VIEW message AS {messages}; CREATE TEMP VIEW part AS {parts};"
    ))
}
