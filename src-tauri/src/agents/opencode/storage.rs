//! Read-only compatibility projections. TEMP views live on this connection only;
//! main remains SQLITE_OPEN_READ_ONLY. Never migrate the user's OpenCode database.
//! Schema reference: anomalyco/opencode @ 5ed03ad1377b549df83aa52dc11bdb07593a5327,
//! packages/core/src/session/sql.ts and packages/schema/src/session-message.ts.
use rusqlite::{params_from_iter, Connection};
use std::collections::HashMap;

pub(super) fn exists(conn: &Connection, name: &str) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM main.sqlite_master WHERE name=?1 AND type IN ('table','view'))",
        [name],
        |r| r.get(0),
    )
}

// Measure raw payloads before the v2 projections split and reshape them.
pub(super) fn session_sizes(conn: &Connection, session: Option<&str>) -> HashMap<String, u64> {
    let mut sizes = HashMap::new();
    let scope = if session.is_some() {
        "m.session_id = ?1"
    } else {
        "1"
    };
    let legacy_messages = if exists(conn, "session_v2").unwrap_or(false) {
        let sql = format!(
            "SELECT m.session_id, SUM(octet_length(m.data))
             FROM temp._asm_v2_messages m JOIN main.session_v2 s ON s.id = m.session_id
             WHERE {scope} GROUP BY m.session_id"
        );
        sizes.extend(read_sizes(conn, &sql, session).unwrap_or_default());
        "temp._asm_legacy_messages"
    } else {
        "main.message"
    };
    let sql = format!(
        "WITH messages AS (
            SELECT m.id, m.session_id, octet_length(m.data) AS bytes
            FROM {legacy_messages} m WHERE {scope} AND typeof(m.session_id) = 'text'
         )
         SELECT session_id, SUM(bytes) FROM (
            SELECT session_id, bytes FROM messages
            UNION ALL
            SELECT m.session_id, octet_length(p.data) FROM messages m
            JOIN main.part p ON p.message_id = m.id
         ) GROUP BY session_id"
    );
    sizes.extend(read_sizes(conn, &sql, session).unwrap_or_default());
    sizes
}

fn read_sizes(
    conn: &Connection,
    sql: &str,
    session: Option<&str>,
) -> rusqlite::Result<HashMap<String, u64>> {
    let mut statement = conn.prepare(sql)?;
    let rows = statement.query_map(params_from_iter(session), |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, Option<i64>>(1)?))
    })?;
    let mut sizes = HashMap::new();
    for row in rows {
        let (id, bytes) = row?;
        if let Some(bytes) = bytes {
            sizes.insert(id, bytes as u64);
        }
    }
    Ok(sizes)
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

pub(crate) const MESSAGE_ERROR_SQL: &str = "CASE json_type(data,'$.error')
    WHEN 'object' THEN json_object(
        'kind', coalesce(nullif(json_extract(data,'$.error.kind'),''),nullif(json_extract(data,'$.error.type'),''),json_extract(data,'$.error.name')),
        'message', coalesce(nullif(json_extract(data,'$.error.message'),''),json_extract(data,'$.error.data.message')),
        'status', coalesce(json_extract(data,'$.error.status'),json_extract(data,'$.error.data.statusCode')))
    WHEN 'text' THEN json_object('message',json_extract(data,'$.error'))
    END";

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
    let scope = session
        .map(|id| format!("session_id='{}'", id.replace('\'', "''")))
        .unwrap_or_else(|| "1".into());
    conn.execute_batch(&format!(
        "CREATE TEMP VIEW _asm_v2_messages AS SELECT * FROM main.session_message WHERE {scope}"
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
    let mut messages = format!("SELECT id,session_id,time_created,seq AS sort_order,
        CASE WHEN json_valid(data) AND json_type(data)='object' THEN json_object(
            'role',type,'system',json_extract(data,'$.system'),'model',json_extract(data,'$.model'),
            'modelID',json_extract(data,'$.model.id'),'providerID',json_extract(data,'$.model.providerID'),
            'time',json_extract(data,'$.time'),'tokens',json_extract(data,'$.tokens'),
            'finish',json_extract(data,'$.finish'),
            'variant',json_extract(data,'$.variant'),'serviceTier',coalesce(json_extract(data,'$.serviceTier'),json_extract(data,'$.service_tier'),json_extract(data,'$.tier')),
            'httpStatus',json_extract(data,'$.httpStatus'),
            'error',{MESSAGE_ERROR_SQL},'outcome',json_extract(data,'$.outcome'),
            '_asm_failed',json_type(data,'$.error') IS NOT NULL AND json_type(data,'$.error') != 'null',
            '_asm_native_session',CASE WHEN json_extract(data,'$.model.providerID')='claude-agent'
              THEN (SELECT json_extract(value,'$.native.sessionID') FROM json_each(data,'$.content')
                    WHERE CASE WHEN type='object' THEN length(trim(json_extract(value,'$.native.sessionID')))>0 END LIMIT 1) END,
            'agent',json_extract(data,'$.agent'),'mode',json_extract(data,'$.mode'),'path',json_extract(data,'$.path'))
        ELSE '{{' END AS data
        FROM temp._asm_v2_messages WHERE type IN ('user','assistant','compaction','synthetic','system','skill','shell','idle')");
    let legacy_messages = exists(conn, "message")?;
    if legacy_messages {
        conn.execute_batch(&format!(
            "CREATE TEMP VIEW _asm_legacy_messages AS
             SELECT m.* FROM main.message m WHERE {scope}
             AND NOT EXISTS (SELECT 1 FROM main.session_v2 v WHERE v.id=m.session_id)"
        ))?;
        messages.push_str(
            " UNION ALL SELECT id,session_id,time_created,time_created AS sort_order,data
            FROM temp._asm_legacy_messages",
        );
    }
    // Array index is the stable part identity and ordering key. Do not sort tool
    // content by timestamp: v2 seq/content order is authoritative even after replay.
    let file_data = if session.is_some() {
        "json_extract(f.value,'$.data')"
    } else {
        "NULL"
    };
    let mut parts = format!(
        "SELECT id || ':v2:0000000000' AS id,id AS message_id,time_created,
            json_object('type','text','text',json_extract(data,'$.text')) AS data
        FROM temp._asm_v2_messages WHERE type='user' AND json_valid(data)
        UNION ALL
        SELECT m.id || ':v2:file:' || printf('%010d',f.key),m.id,m.time_created,
            json_object('type','file','filename',json_extract(f.value,'$.name'),
                'mime',json_extract(f.value,'$.mime'),'data',{file_data})
        FROM temp._asm_v2_messages m, json_each(CASE
            WHEN json_valid(m.data) AND json_type(m.data,'$.files')='array'
            THEN json_extract(m.data,'$.files') ELSE '[]' END) f
        WHERE m.type='user' AND f.type='object' AND (json_type(f.value,'$.name')='text' OR json_type(f.value,'$.data')='text')
        UNION ALL
        SELECT m.id || ':v2:' || printf('%010d',p.key),m.id,m.time_created,
            CASE WHEN p.type='object' THEN p.value ELSE '{{' END
        FROM temp._asm_v2_messages m, json_each(CASE
            WHEN json_valid(m.data) AND json_type(m.data,'$.content')='array'
            THEN json_extract(m.data,'$.content') ELSE '[]' END) p
        WHERE m.type='assistant'
        UNION ALL
        SELECT id || ':v2:context',id,time_created,
            json_object('type','context','contextKind',type,'record',json(data))
        FROM temp._asm_v2_messages WHERE type IN ('compaction','synthetic','system','skill') AND json_valid(data)
        UNION ALL
        SELECT id || ':v2:shell',id,time_created,
            json_object('type','tool','tool','shell','callID',json_extract(data,'$.shellID'),
                'state',json_object(
                    'input',json_object('command',json_extract(data,'$.command')),
                    'status',CASE json_extract(data,'$.status')
                        WHEN 'running' THEN 'running'
                        WHEN 'exited' THEN CASE WHEN coalesce(json_extract(data,'$.exit'),0)=0 THEN 'completed' ELSE 'error' END
                        WHEN 'timeout' THEN 'error' WHEN 'killed' THEN 'error' END,
                    'error',CASE WHEN json_extract(data,'$.status')='timeout' THEN '终端命令超时'
                        WHEN json_extract(data,'$.status')='killed' THEN '终端命令已取消'
                        WHEN json_extract(data,'$.exit')!=0 THEN '退出码：' || json_extract(data,'$.exit') END,
                    'metadata',json_object('exit',CASE WHEN json_extract(data,'$.status')='exited' THEN json_extract(data,'$.exit') END),
                    'content', json_extract(data,'$.output.output')))
        FROM temp._asm_v2_messages WHERE type='shell' AND json_valid(data)
        UNION ALL
        SELECT id || ':v2:invalid',id,time_created,'{{'
        FROM temp._asm_v2_messages WHERE type='assistant' AND json_valid(data)
            AND (
                (json_type(data,'$.content') IS NULL AND COALESCE(json_type(data,'$.error'),'null')='null')
                OR COALESCE(json_type(data,'$.content'),'null') NOT IN ('array','null')
            )",
    );
    if legacy_messages && exists(conn, "part")? {
        parts.push_str(
            " UNION ALL SELECT p.id,p.message_id,p.time_created,p.data
            FROM main.part p JOIN temp._asm_legacy_messages m ON m.id=p.message_id",
        );
    }
    conn.execute_batch(&format!(
        "CREATE TEMP VIEW session AS {sessions};
        CREATE TEMP VIEW message AS {messages}; CREATE TEMP VIEW part AS {parts};"
    ))
}
