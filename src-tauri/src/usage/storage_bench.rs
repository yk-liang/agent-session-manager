//! 显式运行的存储对照：固定源目录、隔离账本，输出哈希和数字，不输出用户记录。
use super::*;
use rusqlite::Connection;
use serde_json::{json, Value};
use std::{path::Path, time::Instant};

fn elapsed_ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}

fn median(mut run: impl FnMut()) -> f64 {
    let mut values = Vec::new();
    for _ in 0..9 {
        let start = Instant::now();
        run();
        values.push(elapsed_ms(start));
    }
    values.sort_by(f64::total_cmp);
    values[4]
}

fn digest(db: &Connection, query: &str, json_column: Option<usize>) -> String {
    let mut hash = Sha256::new();
    let mut stmt = db.prepare(query).unwrap();
    let columns = stmt.column_count();
    let mut rows = stmt.query([]).unwrap();
    while let Some(row) = rows.next().unwrap() {
        for column in 0..columns {
            let raw = row.get_ref(column).unwrap();
            let bytes = match raw {
                rusqlite::types::ValueRef::Text(bytes) => bytes.to_vec(),
                rusqlite::types::ValueRef::Blob(bytes) => zstd::stream::decode_all(bytes).unwrap(),
                _ => panic!("Unexpected fingerprint column"),
            };
            let bytes = if json_column == Some(column) {
                serde_json::to_vec(&serde_json::from_slice::<Value>(&bytes).unwrap()).unwrap()
            } else {
                bytes
            };
            hash.update((bytes.len() as u64).to_le_bytes());
            hash.update(bytes);
        }
    }
    format!("{:x}", hash.finalize())
}

#[test]
#[ignore = "requires explicit isolated storage, fixed logs, and report paths"]
fn storage_comparison() {
    let storage = PathBuf::from(std::env::var("ASM_STORAGE_BENCH_DIR").unwrap());
    let logs = PathBuf::from(std::env::var("ASM_STORAGE_BENCH_LOGS").unwrap());
    let report = PathBuf::from(std::env::var("ASM_STORAGE_BENCH_REPORT").unwrap());
    let cache = storage.join("cache.sqlite3");
    let ledger = storage.join("ledger.sqlite3");
    assert!(storage != logs && logs.is_dir());
    fs::create_dir_all(&storage).unwrap();
    let start = Instant::now();
    let mut store = Store::open_with_ledger(Some(&cache), Some(&ledger));
    store.ledger().unwrap();
    let open_ms = elapsed_ms(start);
    let mut syncs = Vec::new();
    for agent in AGENTS {
        let start = Instant::now();
        let result = summary::sync_at(agent, &[logs.join(agent)], &mut store, &|| false).unwrap();
        syncs.push(json!({"agent":agent,"ms":elapsed_ms(start),"bytes_read":result.bytes_read,"changed_files":result.changed_files}));
    }
    drop(store);
    let mut store = Store::open_with_ledger(Some(&cache), Some(&ledger));
    let warm_ms = median(|| {
        for agent in AGENTS {
            let result =
                summary::sync_at(agent, &[logs.join(agent)], &mut store, &|| false).unwrap();
            assert_eq!(result.bytes_read, 0, "warm sync reread source logs");
        }
    });
    let db = &mut store.ledger().unwrap().db;
    let summary_ms = median(|| {
        summary::read_at(db, Default::default(), &|| false).unwrap();
    });
    let first_page_ms = median(|| {
        events::page(db, Default::default(), &|| false).unwrap();
    });
    let last_page_ms = median(|| {
        events::page(
            db,
            events::EventQuery {
                page: Some(u64::MAX),
                ..Default::default()
            },
            &|| false,
        )
        .unwrap();
    });
    let summary = summary::read_at(db, Default::default(), &|| false).unwrap();
    let source_hash = digest(
        db,
        "SELECT agent,path,data FROM usage_ledger_sources ORDER BY agent,path",
        Some(2),
    );
    let event_hash = digest(
        db,
        "SELECT id,data FROM usage_request_events ORDER BY id",
        Some(1),
    );
    let compact: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info('usage_source_keys') WHERE name='source')", [], |r| r.get(0)).unwrap();
    let identity_hash = digest(
        db,
        if compact {
            "SELECT s.agent,s.path,k.key FROM usage_source_keys k JOIN usage_ledger_sources s ON s.id=k.source ORDER BY s.agent,s.path,k.key"
        } else {
            "SELECT agent,path,key FROM usage_source_keys ORDER BY agent,path,key"
        },
        None,
    );
    let request_count: i64 = db
        .query_row("SELECT count(*) FROM usage_request_events", [], |r| {
            r.get(0)
        })
        .unwrap();
    let version: i64 = db
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        db.query_row("PRAGMA quick_check", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
    drop(store);
    let file_bytes = |path: &Path| fs::metadata(path).unwrap().len();
    let result = json!({"schema":version,"open_ms":open_ms,"syncs":syncs,"restart_warm_median_ms":warm_ms,
        "summary_median_ms":summary_ms,"first_page_median_ms":first_page_ms,"last_page_median_ms":last_page_ms,
        "ledger_bytes":file_bytes(&ledger),"cache_bytes":file_bytes(&cache),"request_count":request_count,
        "sources_sha256":source_hash,"events_sha256":event_hash,"identities_sha256":identity_hash,"summary":summary});
    fs::write(report, serde_json::to_vec_pretty(&result).unwrap()).unwrap();
    println!("Storage comparison complete: schema={version}, requests={request_count}, ledger_bytes={}, cache_bytes={}", file_bytes(&ledger), file_bytes(&cache));
}
