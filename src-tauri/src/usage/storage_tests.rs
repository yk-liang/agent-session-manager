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
        let mut db = ledger::reader(&path, &|| false).unwrap();
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
    let dir = tempfile::tempdir().unwrap();
    let development = dir.path().join("development.sqlite3");
    let db = released_ledger(&development, 5);
    db.pragma_update(None, "user_version", 6).unwrap();
    let error = ledger::Ledger::open(Some(&development)).err().unwrap();
    assert!(error.contains("不支持的用量账本结构版本 6"));
    assert_eq!(db.pragma_query_value::<i64,_>(None, "user_version", |row| row.get(0)).unwrap(), 6);
}

// 由发布提交 c677aac、1c1b353 的 cache::read_file 与 scope 导出；路径和文件戳已固定。
const RELEASED_CACHE: &str = include_str!("../../../tests/fixtures/usage-migration/released-claude-cache.json");
const RELEASED_PATH: &str = "/fixture/.claude/projects/deleted.jsonl";

fn released_cache(path: &Path, version: i64) -> Connection {
    let db = Connection::open(path).unwrap();
    db.execute_batch(
        "CREATE TABLE usage_files(scope TEXT,path TEXT,data TEXT NOT NULL,PRIMARY KEY(scope,path));
         CREATE TABLE usage_file_stamps(scope TEXT,path TEXT,version INTEGER,stamps TEXT,PRIMARY KEY(scope,path));
         CREATE TABLE usage_scans(scope TEXT PRIMARY KEY,updated INTEGER NOT NULL);",
    ).unwrap();
    if version == 1 {
        db.execute_batch("ALTER TABLE usage_files ADD COLUMN touched INTEGER NOT NULL DEFAULT 0;
            ALTER TABLE usage_files ADD COLUMN durable INTEGER NOT NULL DEFAULT 0;").unwrap();
    }
    db.pragma_update(None, "user_version", version).unwrap();
    db
}

fn insert_released_cache(db: &Connection, version: i64, scope: &str, path: &str, data: &Value) {
    let data = if version == 0 {
        rusqlite::types::Value::Text(data.to_string())
    } else {
        rusqlite::types::Value::Blob(storage::encode(data).unwrap())
    };
    db.execute("INSERT INTO usage_files(scope,path,data) VALUES (?1,?2,?3)", params![scope, path, data]).unwrap();
}

fn claude_page(store: &mut Store) -> events::EventPage {
    events::page(&mut store.ledger().unwrap().db,
        events::EventQuery { agent: Some("claude".into()), ..Default::default() }, &|| false).unwrap()
}

#[test]
fn released_cache_recovers_deleted_sources_before_reset_and_can_retry() {
    let fixture: Value = serde_json::from_str(RELEASED_CACHE).unwrap();
    let scope = fixture["scope"].as_str().unwrap();
    for version in [0_i64, 1] {
        let dir = tempfile::tempdir().unwrap();
        let logs = dir.path().join("logs");
        write(&logs.join("current.jsonl"), &[record("current", 7)]);
        let cache = dir.path().join("cache.sqlite3");
        let ledger = dir.path().join("ledger.sqlite3");
        let old = released_ledger(&ledger, if version == 0 { 4 } else { 5 });
        old.execute("INSERT INTO usage_ledger_updates VALUES ('claude',1790000000000)", []).unwrap();
        drop(old);

        let legacy = released_cache(&cache, version);
        insert_released_cache(&legacy, version, scope, RELEASED_PATH, &fixture["file"]);
        // 过去的 v1 会保留未能解码的缓存行，当前可恢复其他完好记录。
        let broken = "/fixture/.claude/projects/broken.jsonl";
        legacy.execute("INSERT INTO usage_files(scope,path,data) VALUES (?1,?2,'bad JSON')", params![scope, broken]).unwrap();
        if version == 1 {
            legacy.execute("INSERT INTO usage_files(scope,path,data,durable) VALUES (?1,'durable','bad JSON',1)", [scope]).unwrap();
        }

        let mut memory_only = Store::open_with_ledger(Some(&cache), None);
        assert!(ingest::sync_at("claude", &[logs.clone()], &mut memory_only, &|| false)
            .err().unwrap().contains("缺少持久用量账本路径"));
        drop(memory_only);

        let mut store = Store::open_with_ledger(Some(&cache), Some(&ledger));
        let error = ingest::sync_at("claude", &[], &mut store, &|| false).err().unwrap();
        assert!(error.contains(broken), "{error}");
        assert_eq!(legacy.pragma_query_value::<i64,_>(None, "user_version", |r| r.get(0)).unwrap(), version);
        assert_eq!(legacy.query_row::<String,_,_>("SELECT data FROM usage_files WHERE path=?1", [broken], |r| r.get(0)).unwrap(), "bad JSON");
        assert_eq!(claude_page(&mut store).events[0].tokens.total(), 113);

        let error = ingest::sync_at("claude", &[logs.clone()], &mut store, &|| false).err().unwrap();
        assert!(error.contains(broken));
        assert_eq!(claude_page(&mut store).total, 2);
        legacy.execute("DELETE FROM usage_files WHERE path=?1", [broken]).unwrap();
        drop(legacy);
        let warm = ingest::sync_at("claude", &[logs.clone()], &mut store, &|| false).unwrap();
        assert_eq!(warm.bytes_read, 0);
        drop(store);
        let mut store = Store::open_with_ledger(Some(&cache), Some(&ledger));
        ingest::sync_at("claude", &[logs], &mut store, &|| false).unwrap();
        let page = claude_page(&mut store);
        assert_eq!(page.total, 2);
        assert_eq!(page.events.iter().map(|event| event.tokens.total()).sum::<u64>(), 220);
        let cache = Connection::open(cache).unwrap();
        assert_eq!(cache.pragma_query_value::<i64,_>(None, "user_version", |r| r.get(0)).unwrap(), 2);
        assert_eq!(cache.query_row("SELECT count(*) FROM usage_files", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
    }
}

#[test]
fn released_cache_merges_same_stamp_across_scopes_after_interrupted_indexing() {
    let fixture: Value = serde_json::from_str(RELEASED_CACHE).unwrap();
    let scope = fixture["scope"].as_str().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("cache.sqlite3");
    let ledger = dir.path().join("ledger.sqlite3");
    let db = released_cache(&cache, 1);
    // 旧文件也能通过其内部项目根目录被另一次已发布的采集发现。
    let shared = "/fixture/.claude/projects/nested/projects/deleted.jsonl";
    let nested_scope = "da85e3f217dfe0636fb93487cd7c4375d31275c9160a45ae13332b399dc41a93";
    insert_released_cache(&db, 1, scope, shared, &fixture["file"]);
    let mut second = fixture["file"].clone();
    second["samples"][0]["key"] = json!("message:msg_second");
    second["samples"][0]["tokens"]["outputTokens"] = json!(31);
    insert_released_cache(&db, 1, nested_scope, shared, &second);

    let mut store = Store::open_with_ledger(Some(&cache), Some(&ledger));
    let published = Connection::open(&ledger).unwrap();
    let cancelled = || {
        published.query_row("SELECT EXISTS(SELECT 1 FROM usage_ledger_sources)", [], |r| r.get(0)).unwrap()
    };
    assert!(store.recover_released_cache("claude", &[], &cancelled).is_err());
    assert_eq!(db.pragma_query_value::<i64,_>(None, "user_version", |r| r.get(0)).unwrap(), 1);
    assert_eq!(published.query_row("SELECT count(*) FROM usage_ledger_sources", [], |r| r.get::<_, i64>(0)).unwrap(), 1);
    assert_eq!(published.query_row("SELECT count(*) FROM usage_request_events", [], |r| r.get::<_, i64>(0)).unwrap(), 0);

    ingest::sync_at("claude", &[], &mut store, &|| false).unwrap();
    let page = claude_page(&mut store);
    assert_eq!(page.total, 2);
    assert_eq!(page.events.iter().map(|event| event.tokens.total()).sum::<u64>(), 244);
    assert_eq!(published.query_row("SELECT count(*) FROM usage_ledger_stamps", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
    assert_eq!(db.pragma_query_value::<i64,_>(None, "user_version", |r| r.get(0)).unwrap(), 2);
    let mut payload = fixture["file"].clone();
    payload["samples"][0]["request"]["status"] = Value::Null;
    let mut rescanned: cache::FileCache = serde_json::from_value(payload).unwrap();
    rescanned.parser_version = source::Source::for_agent("claude").parser_version(Path::new(RELEASED_PATH));
    let sources = store.ledger().unwrap().load("claude").unwrap();
    let merged = ledger::sources::merge("claude", sources.values().next().unwrap(), &rescanned);
    assert_eq!(merged.samples.len(), 2, "重读当前文件后仍保留来自另一份缓存的独有请求");
}

#[test]
fn unavailable_cache_is_reported_without_losing_history() {
    let fixture: Value = serde_json::from_str(RELEASED_CACHE).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("cache.sqlite3");
    let ledger = dir.path().join("ledger.sqlite3");
    let mut db = released_cache(&cache, 1);
    insert_released_cache(&db, 1, fixture["scope"].as_str().unwrap(), RELEASED_PATH, &fixture["file"]);
    let lock = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).unwrap();
    let mut store = Store::open_with_ledger(Some(&cache), Some(&ledger));
    let error = ingest::sync_at("claude", &[], &mut store, &|| false).err().unwrap();
    assert!(error.contains("已保留原数据"));
    lock.pragma_update(None, "user_version", 9).unwrap();
    lock.commit().unwrap();
    let error = ingest::sync_at("claude", &[], &mut store, &|| false).err().unwrap();
    assert!(error.contains("不支持的解析缓存结构版本 9"), "{error}");
    assert_eq!(db.query_row("SELECT count(*) FROM usage_files", [], |r| r.get::<_, i64>(0)).unwrap(), 1);
    assert_eq!(db.pragma_query_value::<i64,_>(None, "user_version", |r| r.get(0)).unwrap(), 9);
    db.pragma_update(None, "user_version", 1).unwrap();
    ingest::sync_at("claude", &[], &mut store, &|| false).unwrap();
    assert_eq!(claude_page(&mut store).total, 1);
    drop(db);
    let logs = dir.path().join("logs");
    drop(store);
    fs::write(&cache, "not a SQLite database").unwrap();
    write(&logs.join("current.jsonl"), &[record("live", 7)]);
    let mut store = Store::open_with_ledger(Some(&cache), Some(&ledger));
    let error = ingest::sync_at("claude", &[logs], &mut store, &|| false).err().unwrap();
    assert!(error.contains("已保留原数据"), "{error}");
    assert_eq!(fs::read_to_string(cache).unwrap(), "not a SQLite database");
    assert_eq!(claude_page(&mut store).total, 2);
}

#[test]
fn released_v4_upgrade_waits_for_the_writer_and_preserves_orphaned_requests() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ledger.sqlite3");
    let db = released_ledger(&path, 4);
    db.execute_batch("INSERT INTO usage_source_keys VALUES ('codex','deleted','request:orphan');
        UPDATE usage_request_events SET source='deleted';").unwrap();
    let lock = ledger::SyncLock::acquire(&path, &|| false).unwrap();
    let attempts = std::cell::Cell::new(0);
    let stop = || {
        attempts.set(attempts.get() + 1);
        attempts.get() > 1
    };
    assert!(ledger::reader(&path, &stop).err().unwrap().contains("采集已停止"));
    assert_eq!(db.pragma_query_value::<i64, _>(None, "user_version", |r| r.get(0)).unwrap(), 4);
    drop(lock);
    drop(db);
    let mut db = ledger::reader(&path, &|| false).unwrap();
    let page = events::page(&mut db, Default::default(), &|| false).unwrap();
    assert_eq!(page.total, 1);
    assert_eq!(page.events[0].tokens.total(), 110);
    assert_eq!(db.query_row("SELECT source FROM usage_request_events", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
    assert_eq!(db.query_row("SELECT count(*) FROM usage_source_keys WHERE key='request:orphan'", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
    drop(db);
    let mut store = Store::open_with_ledger(None, Some(&path));
    let db = &mut store.ledger().unwrap().db;
    db.execute("UPDATE usage_request_index SET indexed=0", []).unwrap();
    events::refresh(db, "codex", &|| false).unwrap();
    assert_eq!(db.query_row("SELECT source FROM usage_request_events WHERE id='event'", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
}

#[test]
fn reparsing_partial_history_keeps_distinct_codex_failures_and_uses_stable_claude_keys() {
    let sample = |key: &str| types::Sample::unmeasured(key.into(), "retained", "fixture".into(), 0, 0);
    let old = cache::FileCache {
        parser_version: 3,
        failures: vec![sample("first-error")],
        samples: vec![sample("session:retained:resp_same"), sample("message:deleted")],
        ..Default::default()
    };
    let incoming = cache::FileCache {
        parser_version: 4,
        failures: vec![sample("second-error")],
        samples: vec![sample("message:resp_same")],
        ..Default::default()
    };
    let codex = ledger::sources::merge("codex", &old, &incoming);
    assert_eq!(codex.failures.len(), 2);
    let claude = ledger::sources::merge("claude", &old, &incoming);
    assert_eq!(claude.samples.iter().map(|sample| sample.key.as_str()).collect::<Vec<_>>(),
        ["message:deleted", "message:resp_same"]);
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
