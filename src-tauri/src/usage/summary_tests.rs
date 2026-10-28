use super::summary::{Summary, SummaryQuery};
use super::*;
use chrono::{Days, NaiveTime};
use serde_json::{json, Value};
use std::{io::Write, path::Path};

fn write(path: &Path, lines: &[Value]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        path,
        lines.iter().map(|v| format!("{v}\n")).collect::<String>(),
    )
    .unwrap();
}
fn record(id: &str, time: i64, input: u64, output: u64) -> Value {
    json!({"type":"assistant","sessionId":"fixture","timestamp":time,"message":{"id":format!("msg_{id}"),"model":"claude-fixture","stop_reason":"end_turn","content":"PRIVATE_BODY","usage":{"input_tokens":input,"output_tokens":output,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}})
}
fn read(store: &mut Store, bounds: Vec<i64>) -> Summary {
    summary::read_at(
        &mut store.ledger().unwrap().db,
        SummaryQuery {
            from: bounds.first().copied(),
            to: bounds.last().copied(),
            bounds,
        },
        &|| false,
    )
    .unwrap()
}
fn sum(result: &Summary) -> u64 {
    result.rows.iter().map(|r| r.usage.total_tokens).sum()
}

#[test]
fn summary_upgrades_v2_without_deleted_sources_or_losing_events() {
    let dir = tempfile::tempdir().unwrap();
    let logs = dir.path().join("logs");
    let source = logs.join("a.jsonl");
    let path = dir.path().join("ledger.sqlite");
    write(&source, &[record("a", 1_790_000_000_000, 100, 10)]);
    let mut store = Store::open_with_ledger(None, Some(&path));
    summary::sync_at("claude", &[logs], &mut store, &|| false).unwrap();
    let db = &store.ledger().unwrap().db;
    db.execute_batch(
        "DROP INDEX usage_source_dirty; DROP INDEX usage_request_source;
        DROP TABLE usage_source_keys;
        ALTER TABLE usage_ledger_sources DROP COLUMN dirty;
        ALTER TABLE usage_ledger_sources DROP COLUMN health;
        ALTER TABLE usage_request_events DROP COLUMN source;
        PRAGMA user_version=2;",
    )
    .unwrap();
    drop(store);
    fs::remove_file(source).unwrap();
    let mut upgraded = Store::open_with_ledger(None, Some(&path));
    summary::sync_at("claude", &[dir.path().join("logs")], &mut upgraded, &|| {
        false
    })
    .unwrap();
    let result = read(&mut upgraded, vec![]);
    assert_eq!(sum(&result), 110);
    assert!(!result.health["claude"].partial);
    assert_eq!(
        upgraded
            .ledger()
            .unwrap()
            .db
            .pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
            .unwrap(),
        4
    );
}

#[test]
fn summary_uses_request_time_not_scan_time_and_matches_event_table() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("a.jsonl");
    let t = 1_790_000_000_000i64;
    write(
        &p,
        &[
            json!({"type":"api-request","id":"request-a","timestamp":t-100,"params":{}}),
            {
                let mut v = record("a", t + 100, 100, 10);
                v["requestRef"] = json!("request-a");
                v
            },
            record("b", t + 3600000, 200, 20),
            record("c", t + 7200000, 300, 30),
        ],
    );
    let mut store = Store::open(None);
    summary::sync_at("claude", &[dir.path().into()], &mut store, &|| false).unwrap();
    let result = read(&mut store, vec![t, t + 3600000, t + 7200000]);
    assert_eq!(
        sum(&result),
        220,
        "start-before-boundary and end-boundary requests are excluded"
    );
    assert_eq!(result.rows[0].bucket_start, Some(t + 3600000));
    let events = events::page(
        &mut store.ledger().unwrap().db,
        events::EventQuery {
            from: Some(t),
            to: Some(t + 7200000),
            ..Default::default()
        },
        &|| false,
    )
    .unwrap();
    assert_eq!(
        sum(&result),
        events.events.iter().map(|e| e.tokens.total()).sum::<u64>()
    );
    // 无源文件也能切换分桶；sync_at 的当前时间从不成为请求发生时间。
    fs::remove_file(&p).unwrap();
    assert_eq!(sum(&read(&mut store, vec![t - 1000, t + 8000000])), 660);
    assert_eq!(
        sum(&read(
            &mut store,
            vec![
                Utc::now().timestamp_millis(),
                Utc::now().timestamp_millis() + 3600000
            ]
        )),
        0
    );
}

#[test]
fn summary_query_reads_persisted_events_without_reprocessing_source_observations() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("a.jsonl");
    let t = 1_790_000_000_000i64;
    write(&p, &[record("a", t, 100, 10)]);
    let mut store = Store::open(None);
    summary::sync_at("claude", &[dir.path().into()], &mut store, &|| false).unwrap();
    store
        .ledger()
        .unwrap()
        .db
        .execute("UPDATE usage_ledger_sources SET data='not-json'", [])
        .unwrap();
    // 归并索引未脏时，用量只从已保存事件查，不加载或重新解析原始观测。
    assert_eq!(sum(&read(&mut store, vec![t, t + 3600000])), 110);
}

#[test]
fn summary_sync_skips_unchanged_history_including_after_restart() {
    let dir = tempfile::tempdir().unwrap();
    let logs = dir.path().join("logs");
    let p = logs.join("a.jsonl");
    let cache = dir.path().join("cache.sqlite");
    let ledger = dir.path().join("ledger.sqlite");
    let t = 1_790_000_000_000;
    write(
        &p,
        &(0..100)
            .map(|i| record(&i.to_string(), t + i, 100, 10))
            .collect::<Vec<_>>(),
    );
    let mut store = Store::open_with_ledger(Some(&cache), Some(&ledger));
    let first = summary::sync_at("claude", &[logs.clone()], &mut store, &|| false).unwrap();
    assert!(first.bytes_read > 0);
    drop(store);
    let mut store = Store::open_with_ledger(Some(&cache), Some(&ledger));
    let warm = summary::sync_at("claude", &[logs.clone()], &mut store, &|| false).unwrap();
    assert_eq!(
        (warm.checked_files, warm.changed_files, warm.bytes_read),
        (1, 0, 0)
    );
    assert_eq!(store.file_loads.get(), 0);
    let line = record("new", t + 1000, 200, 20).to_string() + "\n";
    fs::OpenOptions::new()
        .append(true)
        .open(&p)
        .unwrap()
        .write_all(line.as_bytes())
        .unwrap();
    let added = summary::sync_at("claude", &[logs], &mut store, &|| false).unwrap();
    assert_eq!(added.bytes_read, line.len() as u64);
    assert_eq!(sum(&read(&mut store, vec![])), 11220);
}

#[test]
fn summary_deletion_flushes_new_events_without_forcing_full_history_read() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("a.jsonl");
    let t = 1_790_000_000_000;
    write(&p, &[record("a", t, 100, 10)]);
    let mut store = Store::open(None);
    summary::sync_at("claude", &[dir.path().into()], &mut store, &|| false).unwrap();
    writeln!(
        fs::OpenOptions::new().append(true).open(&p).unwrap(),
        "{}",
        record("b", t + 1000, 200, 20)
    )
    .unwrap();
    preserve_before_delete_at("claude", &[dir.path().into()], &mut store, || {
        fs::remove_file(&p).map_err(|e| e.to_string())
    })
    .unwrap();
    assert!(!p.exists());
    assert_eq!(sum(&read(&mut store, vec![])), 330);
    let db = &store.ledger().unwrap().db;
    assert_eq!(
        db.query_row("SELECT count(*) FROM usage_request_events", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        2
    );
}

#[test]
fn summary_missing_timestamp_stays_in_all_and_invalid_boundaries_fail() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("a.jsonl");
    let t = 1_790_000_000_000i64;
    write(&p, &[record("a", 0, 100, 10)]);
    let mut store = Store::open(None);
    summary::sync_at("claude", &[dir.path().into()], &mut store, &|| false).unwrap();
    let all = read(&mut store, vec![]);
    assert_eq!(sum(&all), 110);
    assert!(all.health["claude"].partial);
    assert_eq!(all.rows[0].usage.date, "");
    assert_eq!(sum(&read(&mut store, vec![t, t + 3600000])), 0);
    for bounds in [vec![t, t], vec![t + 1, t], vec![t, t + 100, t + 50]] {
        assert!(summary::read_at(
            &mut store.ledger().unwrap().db,
            SummaryQuery {
                from: bounds.first().copied(),
                to: bounds.last().copied(),
                bounds
            },
            &|| false
        )
        .is_err());
    }
}

#[test]
fn summary_hour_buckets_follow_supplied_dst_and_quarter_hour_timezone_boundaries() {
    for count in [23, 24, 25] {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.jsonl");
        let t = 1_790_000_000_000i64 + 15 * 60000;
        write(
            &p,
            &(0..count)
                .map(|i| record(&i.to_string(), t + i * 3600000 + 1000, 10, 1))
                .collect::<Vec<_>>(),
        );
        let mut store = Store::open(None);
        summary::sync_at("claude", &[dir.path().into()], &mut store, &|| false).unwrap();
        let bounds = (0..=count).map(|i| t + i * 3600000).collect();
        let result = read(&mut store, bounds);
        assert_eq!(result.rows.len(), count as usize);
        assert_eq!(sum(&result), count as u64 * 11);
    }
}

#[test]
fn export_usage_summary_fixture_if_requested() {
    let Ok(export) = std::env::var("ASM_USAGE_SUMMARY_EXPORT") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let now = Local::now();
    let today = now.date_naive();
    let midnight = |date: chrono::NaiveDate| {
        Local
            .from_local_datetime(&date.and_time(NaiveTime::MIN))
            .earliest()
            .unwrap()
            .timestamp_millis()
    };
    let start = midnight(today);
    let old = midnight(today.checked_sub_days(Days::new(40)).unwrap());
    let yesterday = midnight(today.checked_sub_days(Days::new(1)).unwrap());
    write(
        &dir.path().join("claude/a.jsonl"),
        &[
            record("a", start + 2 * 3600000, 100, 10),
            record("b", start + 15 * 3600000, 200, 20),
            record("old", old + 12 * 3600000, 900, 90),
        ],
    );
    write(
        &dir.path().join("codex/a.jsonl"),
        &[
            json!({"type":"session_meta","payload":{"id":"codex-fixture"}}),
            json!({"type":"turn_context","payload":{"model":"gpt-fixture"}}),
            json!({"type":"token_usage_record","timestamp":start+15*3600000,"payload":{"response_id":"r1","usage":{"input_tokens":300,"output_tokens":30,"cached_input_tokens":200,"reasoning_output_tokens":0}}}),
            json!({"type":"token_usage_record","timestamp":yesterday+12*3600000,"payload":{"response_id":"r2","usage":{"input_tokens":400,"output_tokens":40,"cached_input_tokens":250,"reasoning_output_tokens":0}}}),
        ],
    );
    let mut store = Store::open(None);
    for agent in ["claude", "codex"] {
        summary::sync_at(agent, &[dir.path().join(agent)], &mut store, &|| false).unwrap();
    }
    let end = midnight(today.succ_opt().unwrap());
    let hour_bounds = (start..end)
        .step_by(3600000)
        .chain(std::iter::once(end))
        .collect::<Vec<_>>();
    let day_bounds = |days: u64| {
        (0..days)
            .rev()
            .map(|n| midnight(today.checked_sub_days(Days::new(n)).unwrap()))
            .chain(std::iter::once(end))
            .collect::<Vec<_>>()
    };
    let summaries = json!({"today":read(&mut store,hour_bounds),"week":read(&mut store,day_bounds(7)),"month":read(&mut store,day_bounds(30)),"all":read(&mut store,vec![])});
    let events = events::page(&mut store.ledger().unwrap().db, Default::default(), &|| {
        false
    })
    .unwrap();
    fs::write(
        export,
        serde_json::to_vec(
            &json!({"now":now.timestamp_millis(),"summaries":summaries,"events":events}),
        )
        .unwrap(),
    )
    .unwrap();
}
