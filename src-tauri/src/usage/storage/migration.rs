use super::{decode, encode, MigrationResult, SOURCES};
use crate::usage::{cache::FileCache, events::RequestEvent, request_parse::codex_client_kind};
use rusqlite::{params, Connection};
use serde_json::{json, Map, Value};

// v0.1.33–v0.1.37 使用文本 JSON 与路径关联来源。
pub(super) fn upgrade_v4(db: &Connection) -> MigrationResult {
    db.execute_batch(
        "ALTER TABLE usage_source_keys RENAME TO old_source_keys;
        ALTER TABLE usage_request_events RENAME TO old_request_events;
        ALTER TABLE usage_ledger_sources RENAME TO old_ledger_sources;",
    )?;
    db.execute_batch(SOURCES)?;
    {
        let mut select = db.prepare("SELECT rowid,agent,path,data,dirty FROM old_ledger_sources")?;
        let mut rows = select.query([])?;
        let mut insert = db.prepare("INSERT INTO usage_ledger_sources VALUES (?1,?2,?3,?4,?5)")?;
        while let Some(row) = rows.next()? {
            let agent: String = row.get(1)?;
            let mut file: Value = serde_json::from_str(row.get_ref(3)?.as_str()?)?;
            normalize_source(&agent, &mut file)?;
            insert.execute(params![
                row.get::<_, i64>(0)?,
                agent,
                row.get::<_, String>(2)?,
                encode(&file)?,
                row.get::<_, i64>(4)?
            ])?;
        }
    }
    db.execute_batch(
        "INSERT INTO usage_source_keys
            SELECT (SELECT s.id FROM usage_ledger_sources s WHERE s.agent=k.agent AND s.path=k.path),k.key
            FROM old_source_keys k;
        INSERT INTO usage_request_events
            SELECT e.id,e.agent,e.time,e.model,e.status,e.data,CASE WHEN e.source='' THEN 0 ELSE s.id END
            FROM old_request_events e LEFT JOIN usage_ledger_sources s ON s.agent=e.agent AND s.path=e.source;
        DROP TABLE old_source_keys;
        DROP TABLE old_request_events;
        DROP TABLE old_ledger_sources;
        CREATE TABLE IF NOT EXISTS usage_storage_state(key TEXT PRIMARY KEY,value INTEGER NOT NULL);
        INSERT OR REPLACE INTO usage_storage_state VALUES ('compact_pending',1);",
    )?;
    normalize_events(db)
}

// v0.1.38–v0.1.39 的来源已使用 Zstandard 和整数 ID。
pub(super) fn upgrade_v5(db: &Connection) -> MigrationResult {
    db.execute_batch("ALTER TABLE usage_ledger_sources DROP COLUMN health;")?;
    let mut select = db.prepare("SELECT id,agent,data FROM usage_ledger_sources ORDER BY id")?;
    let mut rows = select.query([])?;
    let mut update = db.prepare("UPDATE usage_ledger_sources SET data=?1 WHERE id=?2")?;
    while let Some(row) = rows.next()? {
        let agent: String = row.get(1)?;
        let mut file: Value = decode(row.get_ref(2)?)?;
        normalize_source(&agent, &mut file)?;
        update.execute(params![encode(&file)?, row.get::<_, i64>(0)?])?;
    }
    normalize_events(db)
}

fn normalize_source(agent: &str, file: &mut Value) -> MigrationResult {
    file.as_object_mut().ok_or("无效的用量来源")?.remove("problem");
    for field in ["samples", "failures"] {
        for sample in file.get_mut(field).and_then(Value::as_array_mut).into_iter().flatten() {
            let partial = sample["partial"].as_bool().unwrap_or(true);
            let input = sample["tokens"]["inputTokens"].as_u64().unwrap_or(0);
            let output = sample["tokens"]["outputTokens"].as_u64().unwrap_or(0);
            if sample["details"].is_null() {
                sample["details"] = json!({});
            }
            normalize_availability(sample["details"].as_object_mut().ok_or("无效的用量分项")?, partial, input, output);
            if let Some(request) = sample.get_mut("request") {
                normalize_request(agent, request);
            }
            if sample["model"].as_str() == Some("") {
                sample["model"] = json!("unknown");
            }
        }
    }
    if let Some(context) = file.pointer_mut("/state/requests/context") {
        normalize_request(agent, context);
    }
    serde_json::from_value::<FileCache>(file.clone())?;
    Ok(())
}

fn normalize_events(db: &Connection) -> MigrationResult {
    let mut select = db.prepare("SELECT rowid,agent,data FROM usage_request_events ORDER BY rowid")?;
    let mut rows = select.query([])?;
    let mut update = db.prepare("UPDATE usage_request_events SET status=?1,model=?2,data=?3 WHERE rowid=?4")?;
    while let Some(row) = rows.next()? {
        let raw: String = row.get(2)?;
        let agent: String = row.get(1)?;
        let mut event: Value = serde_json::from_str(&raw)?;
        let partial = event["partial"].as_bool().unwrap_or(true);
        let input = event["inputTokens"].as_u64().unwrap_or(0);
        let output = event["outputTokens"].as_u64().unwrap_or(0);
        let fields = event.as_object_mut().ok_or("无效的请求记录")?;
        fields.remove("modelName");
        normalize_availability(fields, partial, input, output);
        if event["status"] == "unknown" {
            event["status"] = json!("success");
        }
        if event["model"].as_str() == Some("") {
            event["model"] = json!("unknown");
        }
        if normalize_request(&agent, &mut event["request"]) {
            event["outputTps"] = Value::Null;
        }
        let event_data: RequestEvent = serde_json::from_value(event.clone())?;
        update.execute(params![
            event_data.status.as_str(),
            event_data.model,
            serde_json::to_string(&event)?,
            row.get::<_, i64>(0)?
        ])?;
    }
    Ok(())
}

fn normalize_availability(fields: &mut Map<String, Value>, partial: bool, input: u64, output: u64) {
    let known = fields.remove("availabilityKnown").and_then(|flag| flag.as_bool())
        .unwrap_or_else(|| ["inputRecorded", "outputRecorded", "inputIncomplete", "outputIncomplete"]
            .iter().all(|name| fields.get(*name).is_some_and(Value::is_boolean)));
    if !known {
        fields.insert("inputRecorded".into(), json!(input > 0 || !partial));
        fields.insert("outputRecorded".into(), json!(output > 0 || !partial));
        fields.insert("inputIncomplete".into(), json!(partial));
        fields.insert("outputIncomplete".into(), json!(partial));
    }
    for name in ["cacheReadRecorded", "cacheWriteRecorded", "reasoningRecorded"] {
        fields.entry(name).or_insert(Value::Bool(false));
    }
}

fn normalize_request(agent: &str, request: &mut Value) -> bool {
    let Some(fields) = request.as_object_mut() else { return false };
    fields.remove("costUsd");
    if fields.get("status").is_some_and(|status| status == "unknown") {
        fields.insert("status".into(), Value::Null);
    }
    if agent == "codex" {
        let current = fields.get("kind").and_then(Value::as_str);
        let client = fields.get("client").and_then(Value::as_str);
        if let Some(kind) = codex_client_kind(current, client) {
            fields.insert("kind".into(), json!(kind));
        }
    }
    let clear = fields.get("scope").is_some_and(|scope| scope == "usage_delta")
        || agent == "codex" && fields.get("timing").is_some_and(|timing| timing == "estimated");
    let turn_ttft = agent == "codex" && fields.get("ttftScope").is_some_and(|scope| scope == "turn");
    if clear {
        for key in ["startedAt", "timing", "latencyMs"] {
            fields.insert(key.into(), Value::Null);
        }
    }
    if clear || turn_ttft {
        fields.insert("ttftMs".into(), Value::Null);
        fields.insert("ttftScope".into(), Value::Null);
    }
    clear
}
