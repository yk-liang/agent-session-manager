use super::*;
use rusqlite::{params, Connection};
use serde_json::{json, Value};
use std::{io::Write, path::Path};

fn record(id: &str, output: u64) -> Value {
    json!({"type":"assistant","sessionId":"retained","timestamp":1790000000000i64,
        "message":{"id":id,"model":"fixture","usage":{"input_tokens":100,"output_tokens":output}}})
}

fn write(path: &Path, records: &[Value]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, records.iter().map(|r| format!("{r}\n")).collect::<String>()).unwrap();
}

fn released_ledger(path: &Path, version: i64) -> Connection {
    let db = Connection::open(path).unwrap();
    let (id_column, path_key, data_type, key_columns, source_type) = if version == 4 {
        ("", "PRIMARY KEY", "TEXT", "agent TEXT, path TEXT, key TEXT, PRIMARY KEY(agent,path,key)", "TEXT")
    } else {
        ("id INTEGER PRIMARY KEY,", "UNIQUE", "BLOB", "source INTEGER, key TEXT, PRIMARY KEY(source,key)", "INTEGER")
    };
    db.execute_batch(&format!(
        "CREATE TABLE usage_ledger_sources({id_column} agent TEXT,path TEXT,data {data_type} NOT NULL,
         dirty INTEGER NOT NULL DEFAULT 1,health TEXT,{path_key}(agent,path));
         CREATE TABLE usage_source_keys({key_columns});
         CREATE TABLE usage_ledger_updates(agent TEXT PRIMARY KEY,updated INTEGER NOT NULL);
         CREATE TABLE usage_request_index(agent TEXT PRIMARY KEY,revision INTEGER,indexed INTEGER);
         CREATE TABLE usage_request_events(id TEXT PRIMARY KEY,agent TEXT,time INTEGER,model TEXT,status TEXT,data TEXT,source {source_type});
         INSERT INTO usage_request_index VALUES ('codex',1,1);
         INSERT INTO usage_ledger_updates VALUES ('codex',1790000000000);
         PRAGMA user_version={version};"
    )).unwrap();
    let tokens = json!({"inputTokens":100,"outputTokens":10,"cacheReadTokens":0,"cacheWriteTokens":0,
        "reasoningTokens":0,"unclassifiedTokens":0});
    let request = json!({"status":"unknown","kind":"vscode","client":"codex-tui","scope":"request",
        "startedAt":1790000000000i64,"finishedAt":1790000000010i64,"latencyMs":10,
        "timing":"estimated","ttftScope":"turn","ttftMs":30,"costUsd":0.5});
    let file = json!({
        "parser_version":3,"origin_path":"/deleted.jsonl","stamps":[],"offset":1,"head_len":0,
        "head":"","tail":"","pending":false,"problem":null,
        "state":{"session":"saved","model":"fixture","requests":{"context":{"status":"unknown"}}},
        "samples":[{"key":"key","session":"saved","model":"fixture","partial":false,"offset":0,
            "timestamp":1790000000010i64,"complete":true,"cumulative":false,"request":request,"tokens":tokens}],
        "additionalField":{"kept":true}
    });
    let value = if version == 4 {
        rusqlite::types::Value::Text(file.to_string())
    } else {
        rusqlite::types::Value::Blob(storage::encode(&file).unwrap())
    };
    db.execute("INSERT INTO usage_ledger_sources(agent,path,data,dirty,health) VALUES ('codex','archived',?1,0,'old')", [value]).unwrap();
    let source = if version == 4 {
        db.execute("INSERT INTO usage_source_keys VALUES ('codex','archived','request:key')", []).unwrap();
        rusqlite::types::Value::Text("archived".into())
    } else {
        db.execute("INSERT INTO usage_source_keys VALUES (1,'request:key')", []).unwrap();
        rusqlite::types::Value::Integer(1)
    };
    let mut event = tokens;
    event.as_object_mut().unwrap().extend(json!({
        "id":"event","agent":"codex","session":"saved","time":1790000000000i64,
        "model":"fixture","modelName":"Fixture","status":"unknown","partial":false,"request":request,
        "outputTps":1000,"availabilityKnown":true,"inputRecorded":true,"outputRecorded":true,
        "cacheReadRecorded":false,"cacheWriteRecorded":false,"reasoningRecorded":false
    }).as_object().unwrap().clone());
    db.execute("INSERT INTO usage_request_events VALUES ('event','codex',1790000000000,'fixture','unknown',?1,?2)",
        params![event.to_string(), source]).unwrap();
    db
}

#[test]
fn released_ledgers_upgrade_deleted_history_and_normalize_persisted_metadata() {
    for version in [4, 5] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ledger.sqlite3");
        drop(released_ledger(&path, version));
        let mut db = ledger::reader(&path).unwrap();
        assert_eq!(db.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0)).unwrap(), storage::VERSION);
        let (health, source_id): (i64, i64) = db.query_row(
            "SELECT (SELECT count(*) FROM pragma_table_info('usage_ledger_sources') WHERE name='health'),source
             FROM usage_source_keys WHERE key='request:key'", [], |r| Ok((r.get(0)?, r.get(1)?))
        ).unwrap();
        assert_eq!((health, source_id), (0, 1));
        let file: Value = db.query_row("SELECT data FROM usage_ledger_sources", [], |r| storage::decode(r.get_ref(0)?)).unwrap();
        let sample = &file["samples"][0];
        assert!(sample["request"]["latencyMs"].is_null());
        assert_eq!(sample["details"]["inputRecorded"], true);
        assert_eq!(file["additionalField"]["kept"], true);
        let result = events::page(&mut db, Default::default(), &|| false).unwrap();
        assert_eq!(result.total, 1);
        let event = &result.events[0];
        assert_eq!(event.id, "event");
        assert_eq!(event.tokens.total(), 110);
        assert_eq!(event.status, request_parse::RequestOutcome::Success);
        assert_eq!(event.request.kind.as_deref(), Some("cli"));
        assert_eq!((event.request.latency_ms, event.request.ttft_ms, event.output_tps), (None, None, None));
    }
}

#[test]
fn invalid_released_history_aborts_migration() {
    for version in [4, 5] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ledger.sqlite3");
        let db = released_ledger(&path, version);
        db.execute("UPDATE usage_request_events SET data='broken'", []).unwrap();
        assert!(ledger::Ledger::open(Some(&path)).is_err());
        assert_eq!(db.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0)).unwrap(), version);
        let (health, status): (String, String) = db.query_row(
            "SELECT health,(SELECT status FROM usage_request_events) FROM usage_ledger_sources",
            [], |r| Ok((r.get(0)?, r.get(1)?))
        ).unwrap();
        assert_eq!((health.as_str(), status.as_str()), ("old", "unknown"));
    }
}

#[test]
fn evicted_sources_skip_reads_only_when_the_ledger_has_their_measurements() {
    let dir = tempfile::tempdir().unwrap();
    let logs = dir.path().join("logs");
    let cache = dir.path().join("cache.sqlite3");
    let ledger = dir.path().join("ledger.sqlite3");
    let file = logs.join("a.jsonl");
    write(&file, &[record("msg_a", 10)]);
    let mut store = Store::open_with_ledger(Some(&cache), Some(&ledger));
    ingest::sync_at("claude", &[logs.clone()], &mut store, &|| false).unwrap();
    let db = Connection::open(&cache).unwrap();
    Store::trim(&db, 0).unwrap();
    assert_eq!(db.query_row("SELECT count(*) FROM usage_files", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
    assert_eq!(ingest::sync_at("claude", &[logs.clone()], &mut store, &|| false).unwrap().bytes_read, 0);

    let fresh = dir.path().join("fresh.sqlite3");
    drop(store);
    let mut store = Store::open_with_ledger(Some(&cache), Some(&fresh));
    assert!(ingest::sync_at("claude", &[logs.clone()], &mut store, &|| false).unwrap().bytes_read > 0);
    writeln!(fs::OpenOptions::new().append(true).open(&file).unwrap(), "{}", record("msg_b", 20)).unwrap();
    ingest::sync_at("claude", &[logs.clone()], &mut store, &|| false).unwrap();
    event_tests::assert_index_matches_full(&mut store, "claude");
    fs::remove_file(file).unwrap();
    ingest::sync_at("claude", &[logs], &mut store, &|| false).unwrap();
    assert_eq!(db.query_row("SELECT count(*) FROM usage_files", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
    assert_eq!(events::page(&mut store.ledger().unwrap().db, Default::default(), &|| false).unwrap().total, 2);
}
