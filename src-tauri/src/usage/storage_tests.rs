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
    fs::write(
        path,
        records.iter().map(|r| format!("{r}\n")).collect::<String>(),
    )
    .unwrap();
}

/// 独立重建已发布的 v4 布局，迁移输入不依赖未来字段顺序或 BLOB 表示。
fn legacy(dir: &Path) -> (PathBuf, PathBuf, PathBuf) {
    let logs = dir.join("logs");
    let ledger = dir.join("ledger.sqlite3");
    let cache = dir.join("cache.sqlite3");
    write(
        &logs.join("parent.jsonl"),
        &[record("msg_shared", 10), record("msg_other", 20)],
    );
    write(&logs.join("copy.jsonl"), &[record("msg_shared", 10)]);
    let mut store = Store::open_with_ledger(Some(&cache), Some(&ledger));
    summary::sync_at("claude", &[logs.clone()], &mut store, &|| false).unwrap();
    drop(store);
    let mut db = Connection::open(&ledger).unwrap();
    let tx = db.transaction().unwrap();
    tx.execute_batch("CREATE TABLE old_sources(agent TEXT,path TEXT,data TEXT NOT NULL,dirty INTEGER NOT NULL DEFAULT 1,health TEXT,PRIMARY KEY(agent,path));
        CREATE TABLE old_keys(agent TEXT NOT NULL,path TEXT NOT NULL,key TEXT NOT NULL,PRIMARY KEY(agent,path,key));
        CREATE TABLE old_events(id TEXT PRIMARY KEY,agent TEXT NOT NULL,time INTEGER NOT NULL,model TEXT NOT NULL,status TEXT NOT NULL,data TEXT NOT NULL,source TEXT NOT NULL DEFAULT '');
        INSERT INTO old_keys SELECT s.agent,s.path,k.key FROM usage_source_keys k JOIN usage_ledger_sources s ON s.id=k.source;
        INSERT INTO old_events SELECT e.id,e.agent,e.time,e.model,e.status,e.data,s.path FROM usage_request_events e JOIN usage_ledger_sources s ON s.id=e.source;") .unwrap();
    {
        let mut stmt = tx
            .prepare("SELECT agent,path,data,dirty,health FROM usage_ledger_sources")
            .unwrap();
        let mut rows = stmt.query([]).unwrap();
        while let Some(row) = rows.next().unwrap() {
            let mut value: Value = storage::decode(row.get_ref(2).unwrap()).unwrap();
            value["unknown_legacy_metadata"] = json!({"keep":"exactly", "count":17});
            tx.execute(
                "INSERT INTO old_sources VALUES (?1,?2,?3,?4,?5)",
                params![
                    row.get::<_, String>(0).unwrap(),
                    row.get::<_, String>(1).unwrap(),
                    value.to_string(),
                    row.get::<_, i64>(3).unwrap(),
                    row.get::<_, Option<String>>(4).unwrap()
                ],
            )
            .unwrap();
        }
    }
    tx.execute_batch("DROP TABLE usage_source_keys; DROP TABLE usage_request_events; DROP TABLE usage_ledger_sources; DROP TABLE usage_storage_state;
        ALTER TABLE old_sources RENAME TO usage_ledger_sources;
        ALTER TABLE old_keys RENAME TO usage_source_keys;
        ALTER TABLE old_events RENAME TO usage_request_events;
        CREATE INDEX usage_source_key_lookup ON usage_source_keys(agent,key,path);
        CREATE INDEX usage_source_dirty ON usage_ledger_sources(agent,dirty,path);
        CREATE INDEX usage_request_source ON usage_request_events(agent,source);
        PRAGMA user_version=4;") .unwrap();
    events::schema(&tx).unwrap();
    tx.commit().unwrap();
    (logs, ledger, cache)
}

fn sources(db: &Connection) -> Vec<(String, String, Value)> {
    db.prepare("SELECT agent,path,data FROM usage_ledger_sources ORDER BY agent,path")
        .unwrap()
        .query_map([], |r| {
            Ok((r.get(0)?, r.get(1)?, storage::decode(r.get_ref(2)?)?))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn events_snapshot(db: &mut Connection) -> Value {
    let page = events::page(
        db,
        events::EventQuery {
            limit: Some(200),
            ..Default::default()
        },
        &|| false,
    )
    .unwrap();
    json!({"events":page.events,"total":page.total,"revision":page.revision,
        "summary":summary::read_at(db,Default::default(),&|| false).unwrap()})
}

#[test]
fn storage_migration_preserves_deleted_history_identity_and_every_event_field() {
    let dir = tempfile::tempdir().unwrap();
    let (logs, ledger, cache) = legacy(dir.path());
    let mut old = ledger::reader(&ledger).unwrap();
    let raw = sources(&old);
    let expected = events_snapshot(&mut old);
    // 原日志全部消失后，迁移仍必须仅依靠旧账本完成。
    fs::remove_dir_all(&logs).unwrap();
    drop(old);
    let mut store = Store::open_with_ledger(Some(&cache), Some(&ledger));
    let db = &mut store.ledger().unwrap().db;
    assert_eq!(sources(db), raw, "migration lost raw or unknown fields");
    assert_eq!(events_snapshot(db), expected);
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM usage_ledger_sources WHERE typeof(data)='blob'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        2
    );
    summary::sync_at("claude", &[logs.clone()], &mut store, &|| false).unwrap();
    assert_eq!(events_snapshot(&mut store.ledger().unwrap().db), expected);
    // 恢复副本及迟到的最终计量，仍走原有归并；恢复历史不会多计一次。
    write(
        &logs.join("restored.jsonl"),
        &[record("msg_shared", 10), record("msg_new", 30)],
    );
    summary::sync_at("claude", &[logs], &mut store, &|| false).unwrap();
    event_tests::assert_index_matches_full(&mut store, "claude");
    let page = events::page(&mut store.ledger().unwrap().db, Default::default(), &|| {
        false
    })
    .unwrap();
    assert_eq!(page.total, 3);
    assert_eq!(
        page.events.iter().map(|e| e.tokens.total()).sum::<u64>(),
        360
    );
}

#[test]
fn failed_storage_migration_rolls_back_records_schema_and_relationships() {
    for corrupted in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let (_, ledger, _) = legacy(dir.path());
        let db = Connection::open(&ledger).unwrap();
        if corrupted {
            db.execute("UPDATE usage_ledger_sources SET data='broken' WHERE rowid=(SELECT max(rowid) FROM usage_ledger_sources)", []).unwrap();
        } else {
            // 模拟晚于所有压缩写入才发现的不完整关系，必须整体回滚。
            db.execute(
                "INSERT INTO usage_source_keys VALUES ('claude','missing','retained-key')",
                [],
            )
            .unwrap();
        }
        let before: Vec<(String, String)> = db
            .prepare("SELECT path,data FROM usage_ledger_sources ORDER BY path")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        drop(db);
        assert!(ledger::Ledger::open(Some(&ledger)).is_err());
        let db = Connection::open(&ledger).unwrap();
        assert_eq!(
            db.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            4
        );
        let after: Vec<(String, String)> = db
            .prepare("SELECT path,data FROM usage_ledger_sources ORDER BY path")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(after, before);
        assert_eq!(
            db.query_row(
                "SELECT count(*) FROM sqlite_master WHERE name LIKE '%_v5'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
    }
}

#[test]
fn cache_eviction_keeps_warm_stamps_and_replayed_usage_deduplicated() {
    let dir = tempfile::tempdir().unwrap();
    let logs = dir.path().join("logs");
    let cache = dir.path().join("cache.sqlite3");
    let ledger = dir.path().join("ledger.sqlite3");
    let file = logs.join("a.jsonl");
    write(&file, &[record("msg_a", 10)]);
    let mut store = Store::open_with_ledger(Some(&cache), Some(&ledger));
    summary::sync_at("claude", &[logs.clone()], &mut store, &|| false).unwrap();
    let db = Connection::open(&cache).unwrap();
    // 未证实已入账的旧缓存不参与淘汰，即使会使总量暂时超过预算。
    db.execute(
        "INSERT INTO usage_files SELECT 'legacy','legacy',data,0,0 FROM usage_files LIMIT 1",
        [],
    )
    .unwrap();
    Store::trim(&db, 0).unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM usage_files", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        summary::sync_at("claude", &[logs.clone()], &mut store, &|| false)
            .unwrap()
            .bytes_read,
        0
    );
    writeln!(
        fs::OpenOptions::new().append(true).open(&file).unwrap(),
        "{}",
        record("msg_b", 20)
    )
    .unwrap();
    summary::sync_at("claude", &[logs.clone()], &mut store, &|| false).unwrap();
    event_tests::assert_index_matches_full(&mut store, "claude");
    fs::remove_file(file).unwrap();
    summary::sync_at("claude", &[logs], &mut store, &|| false).unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM usage_file_stamps", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        events::page(&mut store.ledger().unwrap().db, Default::default(), &|| {
            false
        })
        .unwrap()
        .total,
        2
    );
}

#[test]
fn legacy_cache_import_keeps_unreadable_rows_even_when_upgrade_fails() {
    for fail_upgrade in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let logs = dir.path().join("logs");
        let cache = dir.path().join("cache.sqlite3");
        let ledger = dir.path().join("ledger.sqlite3");
        write(&logs.join("gone.jsonl"), &[record("msg_legacy", 10)]);
        let mut store = Store::open_with_ledger(Some(&cache), Some(&ledger));
        summary::sync_at("claude", &[logs.clone()], &mut store, &|| false).unwrap();
        drop(store);
        let db = Connection::open(&cache).unwrap();
        let (scope, path, data): (String, String, Value) = db
            .query_row("SELECT scope,path,data FROM usage_files", [], |r| {
                Ok((r.get(0)?, r.get(1)?, storage::decode(r.get_ref(2)?)?))
            })
            .unwrap();
        db.execute_batch(
            "DROP TABLE usage_files;
        CREATE TABLE usage_files(scope TEXT,path TEXT,data TEXT NOT NULL,PRIMARY KEY(scope,path));
        PRAGMA user_version=0;",
        )
        .unwrap();
        if fail_upgrade {
            db.execute_batch("CREATE TRIGGER fail_upgrade BEFORE UPDATE ON usage_files BEGIN SELECT RAISE(ABORT,'simulated cache write failure'); END;").unwrap();
        }
        db.execute(
            "INSERT INTO usage_files VALUES (?1,'unreadable','broken original bytes')",
            [&scope],
        )
        .unwrap();
        db.execute(
            "INSERT INTO usage_files VALUES (?1,?2,?3)",
            params![scope, path, data.to_string()],
        )
        .unwrap();
        drop(db);
        fs::remove_file(&ledger).unwrap();
        fs::remove_dir_all(&logs).unwrap();
        let mut store = Store::open_with_ledger(Some(&cache), Some(&ledger));
        summary::sync_at("claude", &[logs], &mut store, &|| false).unwrap();
        let page = events::page(&mut store.ledger().unwrap().db, Default::default(), &|| {
            false
        })
        .unwrap();
        assert_eq!(page.total, 1);
        assert_eq!(page.events[0].tokens.total(), 110);
        let db = Connection::open(&cache).unwrap();
        assert_eq!(
            db.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            if fail_upgrade { 0 } else { 1 }
        );
        if !fail_upgrade {
            Store::trim(&db, 0).unwrap();
        }
        assert_eq!(
            db.query_row(
                "SELECT data FROM usage_files WHERE path='unreadable'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            "broken original bytes"
        );
    }
}

#[test]
fn existing_reader_keeps_its_snapshot_through_storage_migration() {
    let dir = tempfile::tempdir().unwrap();
    let (_, ledger, _) = legacy(dir.path());
    let mut reader = ledger::reader(&ledger).unwrap();
    let expected = events_snapshot(&mut reader);
    reader.execute_batch("BEGIN").unwrap();
    let original_sources = sources(&reader);
    let mut migrated = ledger::Ledger::open(Some(&ledger)).unwrap();
    assert_eq!(sources(&reader), original_sources);
    assert_eq!(
        reader
            .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        4
    );
    reader.execute_batch("ROLLBACK").unwrap();
    assert_eq!(events_snapshot(&mut migrated.db), expected);
    assert_eq!(
        reader
            .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        storage::VERSION
    );
}

#[test]
fn cache_reclaims_multiple_pages_without_rescanning_unchanged_logs() {
    let dir = tempfile::tempdir().unwrap();
    let logs = dir.path().join("logs");
    let cache = dir.path().join("cache.sqlite3");
    write(&logs.join("a.jsonl"), &[record("msg_a", 10)]);
    let mut store = Store::open_with_ledger(Some(&cache), Some(&dir.path().join("ledger.sqlite3")));
    summary::sync_at("claude", &[logs.clone()], &mut store, &|| false).unwrap();
    let db = Connection::open(&cache).unwrap();
    // 用临时表制造已释放的数据库页，避免依赖压缩率或生成大量无关日志。
    db.execute_batch("CREATE TABLE scratch(data BLOB); INSERT INTO scratch VALUES (zeroblob(2097152)); DROP TABLE scratch;").unwrap();
    let free = || {
        db.query_row("PRAGMA freelist_count", [], |r| r.get::<_, i64>(0))
            .unwrap()
    };
    let before = free();
    assert!(before > 256);
    let sync = summary::sync_at("claude", &[logs], &mut store, &|| false).unwrap();
    assert_eq!(sync.bytes_read, 0);
    assert!(
        before - free() >= 256,
        "cache reclamation stopped after the first PRAGMA row"
    );
}
