use super::*;
use serde_json::json;
use std::time::{Duration, Instant};

fn seed(dir: &std::path::Path) -> Store {
    let logs = dir.join("logs");
    fs::create_dir_all(&logs).unwrap();
    let record = json!({"type":"assistant","sessionId":"cold-start","timestamp":1790000000000i64,
        "message":{"id":"msg_a","model":"fixture","usage":{"input_tokens":100,"output_tokens":10}}});
    fs::write(logs.join("session.jsonl"), format!("{record}\n")).unwrap();
    let mut store = Store::open_with_ledger(None, Some(&dir.join("ledger.sqlite")));
    summary::sync_at("claude", &[logs], &mut store, &|| false).unwrap();
    store
}

#[test]
fn cold_start_queries_remain_readable_during_a_bulk_write() {
    let dir = tempfile::tempdir().unwrap();
    let mut writer = seed(dir.path());
    let mut reader = ledger::Ledger::open(Some(&dir.path().join("ledger.sqlite"))).unwrap();
    // 冷启动建立索引时，写入可持锁数秒甚至更久。读者必须看到完整的旧快照。
    let tx = writer
        .ledger()
        .unwrap()
        .db
        .transaction_with_behavior(rusqlite::TransactionBehavior::Exclusive)
        .unwrap();
    tx.execute("UPDATE usage_request_index SET revision=revision+1", [])
        .unwrap();
    let started = Instant::now();
    let page = events::page(&mut reader.db, Default::default(), &|| false).unwrap();
    let summary = summary::read_at(&mut reader.db, Default::default(), &|| false).unwrap();
    assert_eq!(page.total, 1);
    assert_eq!(
        summary
            .rows
            .iter()
            .map(|r| r.usage.total_tokens)
            .sum::<u64>(),
        110
    );
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "queries waited for the importer"
    );
}

#[test]
fn cold_start_queries_do_not_rebuild_pending_indexes() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = seed(dir.path());
    let db = &mut store.ledger().unwrap().db;
    db.execute_batch(
        "UPDATE usage_ledger_sources SET dirty=1,data='not decoded by readers';
        UPDATE usage_request_index SET revision=revision+1;",
    )
    .unwrap();
    let changes = db.total_changes();
    let page = events::page(db, Default::default(), &|| false).unwrap();
    let summary = summary::read_at(db, Default::default(), &|| false).unwrap();
    assert_eq!(page.total, 1);
    assert_eq!(
        summary
            .rows
            .iter()
            .map(|r| r.usage.total_tokens)
            .sum::<u64>(),
        110
    );
    assert_eq!(
        db.total_changes(),
        changes,
        "a display query mutated the ledger"
    );
}

#[test]
fn cold_start_missing_ledger_is_empty_and_reading_never_creates_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("not-created/ledger.sqlite");
    let mut db = ledger::reader(&path).unwrap();
    assert_eq!(
        events::page(&mut db, Default::default(), &|| false)
            .unwrap()
            .total,
        0
    );
    assert!(summary::read_at(&mut db, Default::default(), &|| false)
        .unwrap()
        .rows
        .is_empty());
    assert!(!path.parent().unwrap().exists());
}

#[test]
fn cold_start_waiting_writer_can_be_cancelled_without_a_hard_timeout() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ledger.sqlite");
    let first = ledger::SyncLock::acquire(&path, &|| false).unwrap();
    let started = Instant::now();
    assert!(
        ledger::SyncLock::acquire(&path, &|| started.elapsed() >= Duration::from_millis(100))
            .is_err()
    );
    assert!(started.elapsed() < Duration::from_secs(1));
    drop(first);
    assert!(ledger::SyncLock::acquire(&path, &|| false).is_ok());
}

#[test]
fn cold_start_cancelled_import_keeps_committed_progress_and_wal_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let logs = dir.path().join("logs");
    fs::create_dir(&logs).unwrap();
    for n in 0..5 {
        let record = json!({"type":"assistant","sessionId":format!("s{n}"),"timestamp":1790000000000i64,
            "message":{"id":format!("msg_{n}"),"model":"fixture","usage":{"input_tokens":100,"output_tokens":10}}});
        fs::write(logs.join(format!("{n}.jsonl")), format!("{record}\n")).unwrap();
    }
    let path = dir.path().join("ledger.sqlite");
    let cache = dir.path().join("cache.sqlite");
    let mut store = Store::open_with_ledger(Some(&cache), Some(&path));
    let cancel = std::cell::Cell::new(false);
    assert!(ingest::collect_with_progress(
        "claude",
        &[logs.clone()],
        &mut store,
        None,
        &|| cancel.get(),
        &|done, _| {
            if done == 2 {
                cancel.set(true);
            }
        }
    )
    .is_err());
    assert_eq!(
        store
            .ledger()
            .unwrap()
            .db
            .query_row("SELECT count(*) FROM usage_ledger_sources", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        2
    );
    for suffix in ["", "-wal", "-shm"] {
        let file = PathBuf::from(format!("{}{suffix}", path.display()));
        assert_eq!(
            fs::metadata(file).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    drop(store);
    let mut restarted = Store::open_with_ledger(Some(&cache), Some(&path));
    let synced = summary::sync_at("claude", &[logs], &mut restarted, &|| false).unwrap();
    assert_eq!((synced.checked_files, synced.changed_files), (5, 3));
    assert_eq!(
        events::page(
            &mut restarted.ledger().unwrap().db,
            Default::default(),
            &|| false
        )
        .unwrap()
        .total,
        5
    );
}

#[test]
fn cold_start_large_import_is_queryable_and_restart_reads_no_unchanged_logs() {
    use std::{
        io::Write,
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        },
    };
    let dir = tempfile::tempdir().unwrap();
    let logs = dir.path().join("logs");
    fs::create_dir(&logs).unwrap();
    // 32,768 次请求、256 份日志，约 270 MiB 正文；计量只保留小字段。
    let body = "x".repeat(8192);
    for file in 0..256 {
        let mut out = std::io::BufWriter::new(
            fs::File::create(logs.join(format!("{file:04}.jsonl"))).unwrap(),
        );
        for row in 0..128 {
            let record = json!({"type":"assistant","sessionId":format!("session-{file}"),
                "timestamp":1790000000000i64+file*128+row,
                "message":{"id":format!("msg_{file}_{row}"),"model":"fixture","content":body,
                    "usage":{"input_tokens":100,"output_tokens":10}}});
            writeln!(out, "{record}").unwrap();
        }
    }
    let path = dir.path().join("ledger.sqlite");
    let cache = dir.path().join("cache.sqlite");
    let done = Arc::new(AtomicBool::new(false));
    let complete = done.clone();
    let writer_path = path.clone();
    let writer_cache = cache.clone();
    let writer_logs = logs.clone();
    let started = Instant::now();
    let worker = std::thread::spawn(move || {
        let _writer = ledger::SyncLock::acquire(&writer_path, &|| false).unwrap();
        let mut store = Store::open_with_ledger(Some(&writer_cache), Some(&writer_path));
        let result = summary::sync_at("claude", &[writer_logs], &mut store, &|| false);
        complete.store(true, Ordering::SeqCst);
        result.unwrap()
    });
    let mut queries = 0;
    let mut slowest = Duration::ZERO;
    while !done.load(Ordering::SeqCst) && !worker.is_finished() {
        let query_started = Instant::now();
        let mut db = ledger::reader(&path).unwrap();
        let page = events::page(&mut db, Default::default(), &|| false).unwrap();
        assert!(
            page.total == 0 || page.total == 32768,
            "a partial index transaction leaked"
        );
        let report = summary::read_at(&mut db, Default::default(), &|| false).unwrap();
        let tokens: u64 = report.rows.iter().map(|r| r.usage.total_tokens).sum();
        assert!(tokens == 0 || tokens == 32768 * 110);
        slowest = slowest.max(query_started.elapsed());
        queries += 1;
        std::thread::sleep(Duration::from_millis(50));
    }
    let result = worker.join().unwrap();
    assert_eq!(result.changed_files, 256);
    assert!(result.bytes_read > 256 * 1024 * 1024);
    assert!(queries > 1);
    assert!(slowest < Duration::from_secs(2));
    let cold_ms = started.elapsed().as_millis();
    let mut db = ledger::reader(&path).unwrap();
    assert_eq!(
        events::page(&mut db, Default::default(), &|| false)
            .unwrap()
            .total,
        32768
    );
    assert_eq!(
        db.query_row("PRAGMA quick_check", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
    drop(db);
    let mut restarted = Store::open_with_ledger(Some(&cache), Some(&path));
    let warm = summary::sync_at("claude", &[logs], &mut restarted, &|| false).unwrap();
    assert_eq!((warm.changed_files, warm.bytes_read), (0, 0));
    let sum: u64 = summary::read_from(&mut restarted, Default::default(), &|| false)
        .unwrap()
        .rows
        .iter()
        .map(|r| r.usage.total_tokens)
        .sum();
    assert_eq!(sum, 32768 * 110);
    println!(
        "cold_import_ms={cold_ms}, concurrent_queries={queries}, slowest_query_ms={}",
        slowest.as_millis()
    );
}
