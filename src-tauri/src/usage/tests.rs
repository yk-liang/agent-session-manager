use super::*;
use serde_json::{json, Value};
use std::io::Write;
use std::path::Path;

fn write(path: &Path, values: &[Value]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let body = values
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    fs::write(path, body).unwrap();
}
fn append(path: &Path, value: Value) {
    writeln!(
        fs::OpenOptions::new().append(true).open(path).unwrap(),
        "{value}"
    )
    .unwrap();
}
fn scan(agent: &str, root: &Path, store: &mut Store) -> UsageReport {
    let report = scan_report(agent, root, store);
    super::event_tests::assert_index_matches_full(store, agent);
    report
}
fn scan_report(agent: &str, root: &Path, store: &mut Store) -> UsageReport {
    read_at(agent, &[root.into()], store, true, false, &|| false).unwrap().unwrap()
}
fn total(report: &UsageReport) -> u64 {
    report.rows.iter().map(|r| r.total_tokens).sum()
}

#[test]
fn installed_v2_runtime_database_if_requested() {
    let Ok(path) = std::env::var("ASM_OPENCODE_LIVE_DB") else {
        return;
    };
    let path = Path::new(&path);
    let dir = tempfile::tempdir().unwrap();
    let ledger = dir.path().join("usage.sqlite");
    let mut store = Store::open_with_ledger(None, Some(&ledger));
    let report = scan("opencode", path.parent().unwrap(), &mut store);
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert_eq!(
        total(&report),
        440,
        "four actual model requests; copied fork history must not be billed again"
    );
    assert!(report.rows.iter().all(|r| r.model == "asm-live/live"));
    assert_eq!(
        scan("opencode", path.parent().unwrap(), &mut store).bytes_read,
        0
    );
    drop(store);
    let mut reopened = Store::open_with_ledger(None, Some(&ledger));
    assert_eq!(
        total(&scan("opencode", path.parent().unwrap(), &mut reopened)),
        440
    );
}
fn family(id: &str, usage: Option<Value>) -> Value {
    let mut value = json!({"type":"assistant","uuid":format!("u-{id}"),"sessionId":"session","timestamp":"2026-10-20T23:00:00Z",
        "message":{"id":id,"role":"assistant","model":"model-a","content":[{"type":"text","text":"PRIVATE_BODY_SHOULD_NOT_BE_CACHED"}]}});
    if let Some(u) = usage {
        value["message"]["usage"] = u;
        value["message"]["stop_reason"] = json!("end_turn");
    }
    value
}
fn counters(input: u64, output: u64) -> Value {
    json!({"input_tokens":input,"cached_input_tokens":0,"cache_write_input_tokens":0,"output_tokens":output,"reasoning_output_tokens":0,"total_tokens":input+output})
}
fn meta(id: &str) -> Value {
    json!({"type":"session_meta","timestamp":"2026-10-20T00:00:00Z","payload":{"id":id,"timestamp":"2026-10-20T00:00:00Z"}})
}
fn counter(input: u64, output: u64, ordinal: u64) -> Value {
    json!({"type":"event_msg","ordinal":ordinal,"timestamp":format!("2026-10-20T00:00:{ordinal:02}Z"),"payload":{"type":"token_count","info":{"total_token_usage":counters(input,output)}}})
}
fn detail(id: &str, input: u64, output: u64, total_input: u64, total_output: u64) -> Value {
    json!({"type":"token_usage_record","timestamp":"2026-10-20T00:00:04Z","payload":{"response_id":id,"usage":counters(input,output),"thread_token_usage":counters(total_input,total_output)}})
}

#[test]
fn scanning_one_agent_does_not_lock_other_agents() {
    let _codex = store("codex").lock().unwrap();
    assert!(store("claude").try_lock().is_ok());
    assert!(store("qoder").try_lock().is_ok());
    assert!(store("codex").try_lock().is_err());
}

#[test]
fn unchanged_and_cached_reads_reuse_the_report_without_materializing_history() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("log.jsonl");
    let mut store = Store::open(None);
    let values: Vec<_> = (0..100)
        .map(|i| family(&format!("msg_{i}"), Some(counters(10, 1))))
        .collect();
    write(&path, &values);
    assert_eq!(total(&scan_report("claude", dir.path(), &mut store)), 1100);
    assert_eq!(store.report_builds, 1);
    for refresh in [false, true, false, true] {
        let report = read_at(
            "claude",
            &[dir.path().into()],
            &mut store,
            refresh,
            false,
            &|| false,
        )
        .unwrap()
        .unwrap();
        assert_eq!(total(&report), 1100);
        assert_eq!(report.bytes_read, 0);
        assert_eq!(
            store.report_builds, 1,
            "unchanged reads must not reload and normalize history"
        );
    }
    append(&path, family("new", Some(counters(10, 1))));
    assert_eq!(total(&scan_report("claude", dir.path(), &mut store)), 1111);
    assert_eq!(store.report_builds, 2);
    assert_eq!(total(&scan_report("claude", dir.path(), &mut store)), 1111);
    assert_eq!(store.report_builds, 2);
}

#[test]
fn report_cache_observes_external_ledger_writes_even_with_the_same_updated_time() {
    let dir = tempfile::tempdir().unwrap();
    let ledger_path = dir.path().join("ledger.sqlite");
    let first_root = dir.path().join("first");
    let second_root = dir.path().join("second");
    write(
        &first_root.join("a.jsonl"),
        &[family("a", Some(counters(10, 1)))],
    );
    write(
        &second_root.join("b.jsonl"),
        &[family("b", Some(counters(20, 2)))],
    );
    let mut first = Store::open_with_ledger(None, Some(&ledger_path));
    let mut second = Store::open_with_ledger(None, Some(&ledger_path));
    let original = scan("claude", &first_root, &mut first);
    assert_eq!(total(&original), 11);
    assert_eq!(total(&scan("claude", &second_root, &mut second)), 33);
    let external = rusqlite::Connection::open(&ledger_path).unwrap();
    external
        .execute(
            "UPDATE usage_ledger_updates SET updated=?1",
            [original.updated_at],
        )
        .unwrap();
    let report = read_at(
        "claude",
        &[first_root.clone()],
        &mut first,
        false,
        false,
        &|| false,
    )
    .unwrap()
    .unwrap();
    assert_eq!(report.updated_at, original.updated_at);
    assert_eq!(total(&report), 33);
    assert_eq!(first.report_builds, 2);
    assert_eq!(total(&scan("claude", &first_root, &mut first)), 33);
    assert_eq!(first.report_builds, 2);
}

#[test]
fn report_cache_invalidates_for_scope_force_timezone_and_legacy_cache_writes() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("logs");
    let other = dir.path().join("other");
    let path = root.join("a.jsonl");
    write(&path, &[family("a", Some(counters(10, 1)))]);
    let db = dir.path().join("cache.sqlite");
    let mut store = Store::open(Some(&db));
    assert_eq!(total(&scan("claude", &root, &mut store)), 11);
    let mut writer = Store::open(Some(&db));
    let legacy = other.join("deleted.jsonl");
    write(&legacy, &[family("legacy", Some(counters(20, 2)))]);
    let parsed = cache::read_file(&legacy, "claude", None, false, &|| false)
        .unwrap()
        .0;
    writer.commit(
        &scope("claude", &[root.clone()]),
        &BTreeMap::from([(legacy.clone(), parsed)]),
        &BTreeSet::from([legacy.clone()]),
        &[],
        1,
    );
    fs::remove_file(legacy).unwrap();
    assert_eq!(total(&scan("claude", &root, &mut store)), 33);
    assert_eq!(store.report_builds, 2);
    assert_eq!(total(&scan("claude", &other, &mut store)), 33);
    assert_eq!(store.report_builds, 3);
    store.snapshot.as_mut().unwrap().timezone = "previous timezone rules".into();
    assert_eq!(total(&scan("claude", &other, &mut store)), 33);
    assert_eq!(store.report_builds, 4);
    read_at("claude", &[other.into()], &mut store, true, true, &|| false).unwrap();
    assert_eq!(store.report_builds, 5);
    assert_eq!(total(&scan("qoder", &root, &mut store)), 11);
}

#[test]
fn timezone_changes_do_not_cache_chronos_temporarily_stale_local_rules() {
    let mut store = Store::open(None);
    let now = Instant::now();
    assert!(store.timezone_ready("first rules", now));
    assert!(!store.timezone_ready("changed rules", now));
    assert!(!store.timezone_ready("changed rules", now + std::time::Duration::from_millis(500)));
    assert!(store.timezone_ready("changed rules", now + std::time::Duration::from_secs(2)));
}

#[test]
fn deleting_a_session_keeps_lifetime_usage_after_restart_and_restore() {
    let dir = tempfile::tempdir().unwrap();
    let logs = dir.path().join("logs");
    let path = logs.join("session.jsonl");
    let db = dir.path().join("cache.sqlite");
    let event = family(
        "msg_lifetime",
        Some(json!({"input_tokens":100,"output_tokens":20})),
    );
    write(&path, &[event.clone()]);
    let mut store = Store::open(Some(&db));
    assert_eq!(total(&scan("claude", &logs, &mut store)), 120);
    fs::remove_file(&path).unwrap();
    assert_eq!(
        total(&scan("claude", &logs, &mut store)),
        120,
        "deleting a source must not erase recorded usage"
    );
    drop(store);
    let mut store = Store::open(Some(&db));
    assert_eq!(total(&scan("claude", &logs, &mut store)), 120);
    write(&path, &[event]);
    assert_eq!(
        total(&scan("claude", &logs, &mut store)),
        120,
        "restoring a source must not bill it again"
    );
}

#[test]
fn cache_cleanup_and_legacy_cache_migration_do_not_erase_cumulative_usage() {
    let dir = tempfile::tempdir().unwrap();
    let logs = dir.path().join("logs");
    let path = logs.join("session.jsonl");
    let cache_path = dir.path().join("cache.sqlite");
    let ledger_path = dir.path().join("durable.sqlite");
    write(&path, &[family("msg_migration", Some(counters(100, 20)))]);
    let parsed = cache::read_file(&path, "claude", None, false, &|| false)
        .unwrap()
        .0;
    let mut store = Store::open_with_ledger(Some(&cache_path), Some(&ledger_path));
    let files = BTreeMap::from([(path.clone(), parsed)]);
    store.commit(
        &scope("claude", &[logs.clone()]),
        &files,
        &BTreeSet::from([path.clone()]),
        &[],
        1,
    );
    fs::remove_file(&path).unwrap();
    let migrated = read_at("claude", &[logs.clone()], &mut store, false, false, &|| {
        false
    })
    .unwrap()
    .unwrap();
    assert_eq!(total(&migrated), 120);
    drop(store);
    fs::remove_file(&cache_path).unwrap();
    let mut store = Store::open_with_ledger(Some(&cache_path), Some(&ledger_path));
    assert_eq!(total(&scan("claude", &logs, &mut store)), 120);
    write(
        &logs.join("restored.jsonl"),
        &[family("msg_migration", Some(counters(100, 20)))],
    );
    assert_eq!(total(&scan("claude", &logs, &mut store)), 120);
}

#[test]
fn deletion_captures_the_last_unseen_reply_and_stops_if_the_ledger_is_unwritable() {
    let dir = tempfile::tempdir().unwrap();
    let logs = dir.path().join("logs");
    let path = logs.join("session.jsonl");
    write(&path, &[family("msg_first", Some(counters(10, 1)))]);
    let mut store = Store::open(None);
    assert_eq!(total(&scan("claude", &logs, &mut store)), 11);
    append(&path, family("msg_last", Some(counters(20, 2))));
    preserve_before_delete_at("claude", &[logs.clone()], &mut store, || {
        fs::remove_file(&path).map_err(|e| e.to_string())
    })
    .unwrap();
    assert_eq!(total(&scan("claude", &logs, &mut store)), 33);

    write(&path, &[family("msg_kept", Some(counters(10, 1)))]);
    let bad = dir.path().join("bad.sqlite");
    fs::write(&bad, b"corrupt ledger must not be replaced").unwrap();
    let mut broken = Store::open_with_ledger(None, Some(&bad));
    assert!(
        preserve_before_delete_at("claude", &[logs], &mut broken, || fs::remove_file(&path)
            .map_err(|e| e.to_string()))
        .is_err()
    );
    assert!(path.exists());
    assert_eq!(
        fs::read(bad).unwrap(),
        b"corrupt ledger must not be replaced"
    );
}

#[test]
fn deletion_stops_before_the_closure_on_current_source_read_failures() {
    let dir = tempfile::tempdir().unwrap();
    let logs = dir.path().join("logs");
    let path = logs.join("session.jsonl");
    write(&path, &[meta("root"), counter(100, 0, 1)]);
    let mut store = Store::open(None);
    assert_eq!(total(&scan("codex", &logs, &mut store)), 100);
    fs::write(logs.join("broken.jsonl.zst"), b"not a zstd stream").unwrap();
    let called = std::cell::Cell::new(false);
    let result = preserve_before_delete_at("codex", &[logs.clone()], &mut store, || {
        called.set(true);
        Ok(())
    });
    assert!(
        result.is_err(),
        "a current file read failure must stop deletion"
    );
    assert!(!called.get());
    assert!(result.unwrap_err().contains("broken.jsonl.zst"));

    let not_a_directory = dir.path().join("unreadable-root");
    fs::write(&not_a_directory, b"not a directory").unwrap();
    let result = preserve_before_delete_at("codex", &[logs, not_a_directory], &mut store, || {
        called.set(true);
        Ok(())
    });
    assert!(
        result.is_err(),
        "a current directory read failure must stop deletion"
    );
    assert!(!called.get());
    assert!(result.unwrap_err().contains("unreadable-root"));
}

#[test]
fn deletion_allows_qoder_missing_usage_and_historical_read_warnings() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("qoder.jsonl");
    write(
        &path,
        &[
            family("measured", Some(counters(10, 1))),
            family("missing", None),
        ],
    );
    let mut store = Store::open(None);
    assert!(scan("qoder", dir.path(), &mut store).partial);
    preserve_before_delete_at("qoder", &[dir.path().into()], &mut store, || {
        fs::remove_file(&path).map_err(|e| e.to_string())
    })
    .unwrap();
    assert_eq!(total(&scan("qoder", dir.path(), &mut store)), 11);

    let broken = dir.path().join("broken.jsonl.zst");
    fs::write(&broken, b"invalid zstd").unwrap();
    assert!(scan("codex", dir.path(), &mut store).partial);
    fs::remove_file(broken).unwrap();
    let called = std::cell::Cell::new(false);
    preserve_before_delete_at("codex", &[dir.path().into()], &mut store, || {
        called.set(true);
        Ok(())
    })
    .unwrap();
    assert!(called.get());
}

#[test]
fn codex_parent_deletion_and_later_response_records_keep_one_historical_request() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(None);
    let parent = dir.path().join("parent.jsonl");
    let mut request = counter(100, 10, 1);
    request["payload"]["info"]["last_token_usage"] = counters(100, 10);
    write(&parent, &[meta("parent"), request.clone()]);
    let mut child = meta("child");
    child["payload"]["forked_from_id"] = json!("parent");
    child["timestamp"] = json!("2026-10-20T00:00:10Z");
    let mut own = counter(150, 15, 12);
    own["payload"]["info"]["last_token_usage"] = counters(50, 5);
    write(&dir.path().join("child.jsonl"), &[child, request, own]);
    assert_eq!(total(&scan("codex", dir.path(), &mut store)), 165);
    fs::remove_file(&parent).unwrap();
    assert_eq!(total(&scan("codex", dir.path(), &mut store)), 165);
    append(
        &dir.path().join("child.jsonl"),
        detail("child-response", 50, 5, 150, 15),
    );
    assert_eq!(total(&scan("codex", dir.path(), &mut store)), 165);
}

#[test]
fn a_reused_codex_log_path_starts_a_new_counter_series_without_erasing_the_old_one() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(None);
    let path = dir.path().join("a.jsonl");
    write(&path, &[meta("first-session"), counter(100, 10, 1)]);
    assert_eq!(total(&scan("codex", dir.path(), &mut store)), 110);
    write(&path, &[meta("second-session"), counter(50, 5, 1)]);
    assert_eq!(total(&scan("codex", dir.path(), &mut store)), 165);
    fs::remove_file(path).unwrap();
    assert_eq!(total(&scan("codex", dir.path(), &mut store)), 165);
}

#[test]
fn qoder_and_pi_deletions_preserve_usage_and_restored_copies_do_not_add_it_again() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(None);
    for agent in ["qoder", "pi"] {
        let logs = dir.path().join(agent);
        let path = logs.join("session.jsonl");
        let values = if agent == "pi" {
            vec![
                json!({"type":"session","id":"stable-pi-session"}),
                json!({"type":"message","id":"one","timestamp":"2026-10-20T00:00:00Z","message":{"role":"assistant","model":"test","usage":{"input":10,"output":1}}}),
            ]
        } else {
            vec![family("msg_retained", Some(counters(10, 1)))]
        };
        write(&path, &values);
        assert_eq!(total(&scan(agent, &logs, &mut store)), 11);
        fs::remove_file(&path).unwrap();
        assert_eq!(total(&scan(agent, &logs, &mut store)), 11);
        write(&logs.join("restored.jsonl"), &values);
        assert_eq!(total(&scan(agent, &logs, &mut store)), 11);
    }
}

#[test]
fn two_collectors_merge_stale_source_snapshots_without_overwriting_recorded_calls() {
    let dir = tempfile::tempdir().unwrap();
    let logs = dir.path().join("logs");
    let path = logs.join("session.jsonl");
    let db = dir.path().join("ledger.sqlite");
    let mut first = ledger::Ledger::open(Some(&db)).unwrap();
    let mut second = ledger::Ledger::open(Some(&db)).unwrap();
    let mut a = BTreeMap::new();
    let mut b = BTreeMap::new();
    let mut a_changed = BTreeSet::new();
    let mut b_changed = BTreeSet::new();
    write(&path, &[family("msg_a", Some(counters(10, 1)))]);
    let source_a = cache::read_file(&path, "claude", None, false, &|| false)
        .unwrap()
        .0;
    ledger::absorb(
        "claude",
        &mut a,
        &BTreeMap::from([(path.clone(), source_a)]),
        &mut a_changed,
    );
    write(&path, &[family("msg_b", Some(counters(20, 2)))]);
    let source_b = cache::read_file(&path, "claude", None, false, &|| false)
        .unwrap()
        .0;
    ledger::absorb(
        "claude",
        &mut b,
        &BTreeMap::from([(path, source_b)]),
        &mut b_changed,
    );
    first.commit("claude", &a, &a_changed, 1).unwrap();
    second.commit("claude", &b, &b_changed, 2).unwrap();
    let (records, _, _) = normalize::records("claude", &first.load("claude").unwrap());
    assert_eq!(records.iter().map(|r| r.tokens.total()).sum::<u64>(), 33);
    assert!(!fs::read(&db)
        .unwrap()
        .windows(b"PRIVATE_BODY".len())
        .any(|w| w == b"PRIVATE_BODY"));
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        fs::metadata(&db).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[test]
fn a_future_ledger_schema_is_rejected_without_replacing_user_data() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("future.sqlite");
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch("CREATE TABLE future_data(value INTEGER);INSERT INTO future_data VALUES(123);PRAGMA user_version=5;").unwrap();
    drop(db);
    assert!(ledger::Ledger::open(Some(&path)).is_err());
    let db = rusqlite::Connection::open(&path).unwrap();
    assert_eq!(
        db.query_row("SELECT value FROM future_data", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        123
    );
    assert_eq!(
        db.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
            .unwrap(),
        5
    );
}

#[test]
fn source_specific_cache_and_reasoning_rules_never_double_count() {
    for (agent, u, input, output, total, partial) in [
        (
            "claude",
            json!({"input_tokens":10,"output_tokens":20,"cache_read_input_tokens":30,"cache_creation_input_tokens":40,"cache_creation":{"ephemeral_5m_input_tokens":40}}),
            80,
            20,
            100,
            false,
        ),
        (
            "codex",
            json!({"input_tokens":80,"cached_input_tokens":30,"cache_write_input_tokens":40,"output_tokens":20,"reasoning_output_tokens":5,"total_tokens":100}),
            80,
            20,
            100,
            false,
        ),
        (
            "opencode",
            json!({"input":10,"output":15,"reasoning":5,"cache":{"read":30,"write":40},"total":100}),
            80,
            20,
            100,
            false,
        ),
        (
            "pi",
            json!({"input":10,"output":20,"reasoning":5,"cacheRead":30,"cacheWrite":40,"totalTokens":100}),
            80,
            20,
            100,
            false,
        ),
        (
            "qoder",
            json!({"input_tokens":80,"output_tokens":20,"cache_read_input_tokens":30,"cache_creation_input_tokens":40}),
            80,
            20,
            100,
            true,
        ),
    ] {
        let (t, p) = parse::normalize(&u, agent).unwrap();
        assert_eq!(
            (t.input_tokens, t.output_tokens, t.total(), p),
            (input, output, total, partial),
            "{agent}"
        );
    }
    assert!(parse::normalize(&json!({}), "claude").is_none());
    assert_eq!(
        parse::normalize(&json!({"input_tokens":0,"output_tokens":0}), "claude")
            .unwrap()
            .0
            .total(),
        0
    );
}

#[test]
fn input_output_availability_distinguishes_unknown_zero_and_partial_for_every_agent() {
    for agent in AGENTS {
        let (input, output, total_key) = match agent {
            "opencode" => ("input", "output", "total"),
            "pi" => ("input", "output", "totalTokens"),
            _ => ("input_tokens", "output_tokens", "total_tokens"),
        };
        let only_total = json!({total_key:30});
        assert_eq!(parse::normalize(&only_total, agent).unwrap().0.total(), 30);
        let details = serde_json::to_value(parse::recorded_fields(&only_total, agent)).unwrap();
        assert_eq!(
            details["inputRecorded"], false,
            "{agent}: missing input must remain unknown"
        );
        assert_eq!(
            details["outputRecorded"], false,
            "{agent}: missing output must remain unknown"
        );
        let zero = json!({input:0,output:0});
        let details = serde_json::to_value(parse::recorded_fields(&zero, agent)).unwrap();
        assert_eq!(
            details["inputRecorded"], true,
            "{agent}: explicit zero is measured"
        );
        assert_eq!(
            details["outputRecorded"], true,
            "{agent}: explicit zero is measured"
        );
    }
}

#[test]
fn claude_and_qoder_forks_keep_global_message_and_request_ids_without_merging_new_calls() {
    for agent in ["claude", "qoder"] {
        for id in [
            "msg_global",
            "chatcmpl-global",
            "abc-global",
            "4a085be6-6235-47a7-abde-889dfebd8a13",
        ] {
            let dir = tempfile::tempdir().unwrap();
            let mut parent = family(id, Some(counters(100, 10)));
            if id == "abc-global" {
                parent["requestId"] = json!("request-global");
            }
            let mut child = parent.clone();
            child["sessionId"] = json!("fork");
            let new = family("msg_new", Some(counters(100, 10)));
            write(&dir.path().join("original.jsonl"), &[parent]);
            write(&dir.path().join("fork.jsonl"), &[child, new]);
            let mut store = Store::open(None);
            assert_eq!(
                total(&scan(agent, dir.path(), &mut store)),
                220,
                "{agent}: {id}"
            );
            fs::remove_file(dir.path().join("original.jsonl")).unwrap();
            assert_eq!(total(&scan(agent, dir.path(), &mut store)), 220);
        }
    }
}

#[test]
fn mixed_availability_survives_aggregation_stream_completion_and_legacy_ledger_loading() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("log.jsonl");
    let ledger = dir.path().join("durable.sqlite");
    write(
        &path,
        &[
            family("msg_total", Some(json!({"total_tokens":30}))),
            family(
                "msg_zero",
                Some(json!({"input_tokens":0,"output_tokens":0})),
            ),
        ],
    );
    let mut store = Store::open_with_ledger(None, Some(&ledger));
    let report = scan("claude", dir.path(), &mut store);
    assert_eq!(total(&report), 30);
    assert!(report.rows[0].details.input_recorded);
    assert!(report.rows[0].details.input_incomplete);
    append(
        &path,
        family(
            "msg_total",
            Some(json!({"input_tokens":20,"output_tokens":10,"total_tokens":30})),
        ),
    );
    let complete = scan("claude", dir.path(), &mut store);
    assert!(!complete.rows[0].details.input_incomplete);
    assert!(!complete.rows[0].details.output_incomplete);
    fs::remove_file(path).unwrap();
    drop(store);
    // Old persisted observations have no availability marker. Explicit all-zero complete calls
    // remain measured zero; total-only partial observations remain unknown after upgrade.
    let db = rusqlite::Connection::open(&ledger).unwrap();
    let raw: String = db
        .query_row("SELECT data FROM usage_ledger_sources LIMIT 1", [], |r| {
            r.get(0)
        })
        .unwrap();
    let mut file: Value = serde_json::from_str(&raw).unwrap();
    for sample in file["samples"].as_array_mut().unwrap() {
        for key in [
            "availabilityKnown",
            "inputRecorded",
            "outputRecorded",
            "inputIncomplete",
            "outputIncomplete",
        ] {
            sample["details"].as_object_mut().unwrap().remove(key);
        }
    }
    db.execute(
        "UPDATE usage_ledger_sources SET data=?1",
        [file.to_string()],
    )
    .unwrap();
    drop(db);
    let mut store = Store::open_with_ledger(None, Some(&ledger));
    let report = scan("claude", dir.path(), &mut store);
    assert_eq!(total(&report), 30);
    assert!(report.rows[0].details.input_recorded);
    assert!(report.rows[0].details.output_recorded);
}

#[test]
fn incremental_collection_is_durable_without_building_or_loading_a_report() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("logs");
    let path = root.join("a.jsonl");
    let cache = dir.path().join("cache.sqlite");
    let ledger = dir.path().join("ledger.sqlite");
    write(&path, &[family("msg_first", Some(counters(10, 1)))]);
    let mut collector = Store::open_with_ledger(Some(&cache), Some(&ledger));
    let roots = vec![root.clone()];
    assert_eq!(
        ingest::collect("claude", &roots, &mut collector, None, &|| false)
            .unwrap()
            .files,
        1
    );
    assert_eq!(collector.report_builds, 0);
    append(&path, family("msg_next", Some(counters(20, 2))));
    let result = ingest::collect(
        "claude",
        &roots,
        &mut collector,
        Some(&BTreeSet::from([path.clone()])),
        &|| false,
    )
    .unwrap();
    assert!(result.bytes_read < fs::metadata(&path).unwrap().len());
    assert_eq!(collector.report_builds, 0);
    fs::remove_file(&path).unwrap();
    drop(collector);
    let mut gui = Store::open_with_ledger(Some(&cache), Some(&ledger));
    assert_eq!(
        total(&scan("claude", &root, &mut gui)),
        33,
        "the GUI can start after logs have already been deleted"
    );
}

#[test]
fn cached_read_before_refresh_cannot_bypass_extraction_upgrade_for_unchanged_logs() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("logs");
    let path = root.join("a.jsonl");
    write(
        &path,
        &[family(
            "msg_zero",
            Some(json!({"input_tokens":0,"output_tokens":1})),
        )],
    );
    let cache_path = dir.path().join("cache.sqlite");
    let mut writer = Store::open(Some(&cache_path));
    let mut old = cache::read_file(&path, "claude", None, false, &|| false)
        .unwrap()
        .0;
    old.parser_version = 0;
    old.samples[0].details = RecordedFields::default();
    old.samples[0].partial = true;
    writer.commit(
        &scope("claude", &[root.clone()]),
        &BTreeMap::from([(path.clone(), old)]),
        &BTreeSet::from([path]),
        &[],
        1,
    );
    drop(writer);
    let mut store = Store::open(Some(&cache_path));
    let cached = read_at("claude", &[root.clone()], &mut store, false, false, &|| {
        false
    })
    .unwrap()
    .unwrap();
    assert!(!cached.rows[0].details.input_recorded);
    let refreshed = scan("claude", &root, &mut store);
    assert!(
        refreshed.bytes_read > 0,
        "an old extraction version must be re-read despite unchanged file stamps"
    );
    assert!(refreshed.rows[0].details.input_recorded);
    assert!(!refreshed.rows[0].details.input_incomplete);
}

#[test]
fn opencode_fork_with_new_ids_does_not_rebill_history_but_keeps_real_new_calls() {
    let dir = tempfile::tempdir().unwrap();
    let conn = rusqlite::Connection::open(dir.path().join("opencode.db")).unwrap();
    conn.execute_batch("CREATE TABLE session(id TEXT PRIMARY KEY,time_created INTEGER); CREATE TABLE message(id TEXT,session_id TEXT,time_created INTEGER,data TEXT);").unwrap();
    let time = 1792454400000i64;
    conn.execute(
        "INSERT INTO session VALUES ('original',?1),('fork',?2),('independent',?1)",
        [time - 1000, time + 10000],
    )
    .unwrap();
    let data = json!({"role":"assistant","parentID":"user-original","modelID":"m","providerID":"p","time":{"created":time,"completed":time+100},"tokens":{"input":100,"output":10}});
    conn.execute(
        "INSERT INTO message VALUES ('a','original',?1,?2)",
        rusqlite::params![time, data.to_string()],
    )
    .unwrap();
    let ledger = dir.path().join("durable.sqlite");
    let mut store = Store::open_with_ledger(None, Some(&ledger));
    assert_eq!(total(&scan("opencode", dir.path(), &mut store)), 110);
    let mut copied = data.clone();
    copied["parentID"] = json!("user-fork");
    conn.execute(
        "INSERT INTO message VALUES ('copy','fork',?1,?2)",
        rusqlite::params![time, copied.to_string()],
    )
    .unwrap();
    assert_eq!(
        total(&scan("opencode", dir.path(), &mut store)),
        110,
        "a fork copies usage with fresh message IDs, without inference"
    );
    // Released ledgers have fresh copied IDs but no replay metadata. Re-reading still-existing
    // messages must enrich those observations rather than retaining their old "original" status.
    drop(store);
    let legacy = rusqlite::Connection::open(&ledger).unwrap();
    let raw: String = legacy
        .query_row("SELECT data FROM usage_ledger_sources LIMIT 1", [], |r| {
            r.get(0)
        })
        .unwrap();
    let mut file: Value = serde_json::from_str(&raw).unwrap();
    file["parser_version"] = json!(0);
    for sample in file["samples"].as_array_mut().unwrap() {
        sample["copy_fingerprint"] = Value::Null;
        sample["replayed"] = json!(false);
    }
    legacy
        .execute(
            "UPDATE usage_ledger_sources SET data=?1",
            [file.to_string()],
        )
        .unwrap();
    drop(legacy);
    let mut store = Store::open_with_ledger(None, Some(&ledger));
    assert_eq!(
        total(&scan("opencode", dir.path(), &mut store)),
        110,
        "legacy copied IDs must be repaired when source metadata is available"
    );
    // Same counters and timestamps are not sufficient to merge independent requests.
    conn.execute(
        "INSERT INTO message VALUES ('real','independent',?1,?2)",
        rusqlite::params![time, data.to_string()],
    )
    .unwrap();
    let mut next = copied;
    next["time"]["created"] = json!(time + 20000);
    next["time"]["completed"] = json!(time + 20100);
    conn.execute(
        "INSERT INTO message VALUES ('new','fork',?1,?2)",
        rusqlite::params![time + 20000, next.to_string()],
    )
    .unwrap();
    assert_eq!(total(&scan("opencode", dir.path(), &mut store)), 330);
    conn.execute(
        "DELETE FROM message WHERE session_id IN ('original','fork')",
        [],
    )
    .unwrap();
    drop(store);
    let mut store = Store::open_with_ledger(None, Some(&ledger));
    assert_eq!(
        total(&scan("opencode", dir.path(), &mut store)),
        330,
        "deletion and restart must preserve corrected lifetime accounting"
    );
}

#[test]
fn opencode_v2_usage_handles_nested_models_compaction_forks_and_persistent_history() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("opencode.db");
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../tests/fixtures/opencode-v2.sql"
    )))
    .unwrap();
    db.execute_batch("PRAGMA journal_mode=WAL;").unwrap();
    let time = 1792454400000i64;
    for (id, created) in [("ses_original", time - 1000), ("ses_fork", time + 1000)] {
        db.execute("INSERT INTO session_v2(id,directory,version,time_created,time_updated) VALUES (?1,'/tmp/v2','2.0.15',?2,?2)",rusqlite::params![id,created]).unwrap();
    }
    let usage = json!({"input":100,"output":10,"reasoning":3,"cache":{"read":20,"write":5}});
    let data = json!({"model":{"id":"v2-model","providerID":"provider"},"agent":"build","time":{"created":time},"tokens":usage,
        "content":[{"type":"text","text":"PRIVATE_V2_BODY_NOT_IN_USAGE"}]});
    let insert = |id: &str, session: &str, kind: &str, seq: i64, data: &Value| {
        db.execute(
            "INSERT INTO session_message VALUES (?1,?2,?3,?4,?5,?5,?6)",
            rusqlite::params![id, session, kind, seq, time, data.to_string()],
        )
        .unwrap();
    };
    insert("msg_original", "ses_original", "assistant", 0, &data);
    insert("msg_copy", "ses_fork", "assistant", 0, &data);
    insert(
        "msg_compact",
        "ses_original",
        "compaction",
        1,
        &json!({"model":{"id":"summary","providerID":"provider"},"time":{"created":time+1},"tokens":{"input":10,"output":2,"reasoning":0,"cache":{"read":0,"write":0}},"status":"completed","summary":"PRIVATE_V2_SUMMARY"}),
    );
    insert(
        "msg_user",
        "ses_original",
        "user",
        2,
        &json!({"text":"PRIVATE_V2_PROMPT","tokens":usage}),
    );
    insert(
        "msg_switch",
        "ses_original",
        "model-switched",
        3,
        &json!({"model":{"id":"unused","providerID":"provider"},"tokens":usage}),
    );
    let ledger = dir.path().join("ledger.sqlite");
    let mut store = Store::open_with_ledger(None, Some(&ledger));
    let report = scan("opencode", dir.path(), &mut store);
    assert_eq!(total(&report), 150); // 100 + 20 + 5 + 10 + 3, plus compaction 12; fork is a copy.
    assert!(report.rows.iter().any(|r| r.model == "provider/v2-model"));
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert_eq!(scan("opencode", dir.path(), &mut store).bytes_read, 0);
    let (cached, _) = opencode::read(&path, None, false, &|| false).unwrap();
    assert_eq!(cached.parser_version, 4);
    assert!(!serde_json::to_string(&cached)
        .unwrap()
        .contains("PRIVATE_V2"));
    let mut legacy_cache = cached.clone();
    legacy_cache.parser_version = 2;
    legacy_cache.samples.clear();
    assert!(
        !opencode::read(&path, Some(&legacy_cache), false, &|| false)
            .unwrap()
            .0
            .samples
            .is_empty()
    );
    db.execute("UPDATE session_message SET data=json_set(data,'$.tokens.output',20) WHERE id='msg_original'",[]).unwrap();
    assert_eq!(total(&scan("opencode", dir.path(), &mut store)), 160);
    let mut new_call = data.clone();
    new_call["time"]["created"] = json!(time + 2000);
    insert("msg_new", "ses_fork", "assistant", 1, &new_call);
    assert_eq!(total(&scan("opencode", dir.path(), &mut store)), 298);
    db.execute("DELETE FROM session_message", []).unwrap();
    drop(store);
    let mut store = Store::open_with_ledger(None, Some(&ledger));
    assert_eq!(total(&scan("opencode", dir.path(), &mut store)), 298);
}

#[test]
fn opencode_v2_missing_and_malformed_usage_never_become_complete_zero_counts() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("opencode.db");
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../tests/fixtures/opencode-v2.sql"
    )))
    .unwrap();
    db.execute_batch("INSERT INTO session_v2(id,directory,version,time_created,time_updated) VALUES ('ses_v2','/tmp/v2','2.0.15',1,1);
        INSERT INTO session_message VALUES ('missing','ses_v2','assistant',0,2,2,'{\"content\":[]}');
        INSERT INTO session_message VALUES ('broken','ses_v2','assistant',1,3,3,'broken');").unwrap();
    let (file, _) = opencode::read(&path, None, false, &|| false).unwrap();
    assert_eq!(file.state.missing_usage, 1);
    assert_eq!(file.state.bad_lines, 1);
    let mut store = Store::open(None);
    assert!(!scan("opencode", dir.path(), &mut store).warnings.is_empty());
}

#[test]
fn absent_detail_fields_differ_from_explicit_zero_and_timestamp_units_are_normalized() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(None);
    let p = dir.path().join("a.jsonl");
    write(
        &p,
        &[family(
            "msg_zero",
            Some(json!({"input_tokens":0,"output_tokens":0})),
        )],
    );
    let first = scan("claude", dir.path(), &mut store);
    assert!(first.has_usage);
    assert_eq!(total(&first), 0);
    assert!(!first.rows[0].details.cache_read_recorded);
    assert!(!first.rows[0].details.reasoning_recorded);
    append(
        &p,
        family(
            "msg_known_zero",
            Some(
                json!({"input_tokens":0,"output_tokens":0,"cache_read_input_tokens":0,"cache_creation_input_tokens":0,"output_tokens_details":{"reasoning_tokens":0}}),
            ),
        ),
    );
    let known = scan("claude", dir.path(), &mut store);
    assert!(known.rows[0].details.reasoning_recorded);
    assert!(known.rows[0].details.cache_read_recorded);
    assert_eq!(total(&known), 0);
    let encoded = serde_json::to_value(known).unwrap();
    assert_eq!(encoded["rows"][0]["cacheReadRecorded"], true);
    assert_eq!(
        parse::timestamp(Some(&json!(1700000000))),
        Some(1700000000000)
    );
    assert_eq!(
        parse::timestamp(Some(&json!(1700000000000i64))),
        Some(1700000000000)
    );
    assert_eq!(
        parse::timestamp(Some(&json!("2023-11-14T22:13:20Z"))),
        Some(1700000000000)
    );
}

#[test]
fn equal_length_middle_rewrite_invalidates_even_when_append_anchors_match() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(None);
    let p = dir.path().join("a.jsonl");
    let mut a = family("msg_a", Some(counters(1, 1)));
    a["message"]["content"] = json!("x".repeat(5000));
    let mut c = family("msg_c", Some(counters(1, 1)));
    c["message"]["content"] = json!("z".repeat(5000));
    let mut values = vec![a, family("msg_b", Some(counters(10, 1))), c];
    write(&p, &values);
    let len = fs::metadata(&p).unwrap().len();
    assert_eq!(total(&scan("claude", dir.path(), &mut store)), 15);
    values[1]["message"]["usage"]["input_tokens"] = json!(20);
    values[1]["message"]["usage"]["total_tokens"] = json!(21);
    write(&p, &values);
    assert_eq!(fs::metadata(&p).unwrap().len(), len);
    assert_eq!(total(&scan("claude", dir.path(), &mut store)), 25);
}

#[test]
fn cancelling_after_one_file_was_parsed_leaves_the_previous_snapshot_atomic() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(None);
    for name in ["a", "b"] {
        write(
            &dir.path().join(format!("{name}.jsonl")),
            &[family(&format!("msg_{name}"), Some(counters(10, 1)))],
        );
    }
    assert_eq!(total(&scan("claude", dir.path(), &mut store)), 22);
    for name in ["a", "b"] {
        append(
            &dir.path().join(format!("{name}.jsonl")),
            family(&format!("msg_{name}_new"), Some(counters(10, 1))),
        );
    }
    let checks = std::cell::Cell::new(0);
    let cancel = || {
        checks.set(checks.get() + 1);
        checks.get() >= 7
    };
    assert!(read_at(
        "claude",
        &[dir.path().into()],
        &mut store,
        true,
        false,
        &cancel
    )
    .is_err());
    assert_eq!(
        total(
            &read_at(
                "claude",
                &[dir.path().into()],
                &mut store,
                false,
                false,
                &|| false
            )
            .unwrap()
            .unwrap()
        ),
        22
    );
}

#[test]
fn claude_cache_ttl_fallback_and_chatcmpl_streaming_keep_one_complete_request() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(None);
    let mut first = family(
        "chatcmpl-one",
        Some(
            json!({"input_tokens":2,"output_tokens":3,"cache_creation_input_tokens":0,"cache_creation":{"ephemeral_5m_input_tokens":10,"ephemeral_1h_input_tokens":20}}),
        ),
    );
    let mut last = first.clone();
    last["timestamp"] = json!("2026-10-20T23:00:05Z");
    last["message"]["usage"]["output_tokens"] = json!(8);
    first["message"]["stop_reason"] = Value::Null;
    write(&dir.path().join("a.jsonl"), &[first, last.clone(), last]);
    let report = scan("claude", dir.path(), &mut store);
    assert_eq!(total(&report), 40);
    assert_eq!(report.rows[0].tokens.cache_write_tokens, 30);
}

#[test]
fn claude_advisor_calls_are_separate_but_regular_iteration_breakdowns_are_not() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(None);
    let usage = json!({"input_tokens":10,"output_tokens":5,"iterations":[
        {"type":"message","input_tokens":10,"output_tokens":5},
        {"type":"advisor_message","model":"advisor-model","input_tokens":2,"output_tokens":3,"cache_read_input_tokens":4}
    ]});
    let value = family("msg_advisor", Some(usage));
    let mut later = value.clone();
    later["message"]["usage"]["iterations"]
        .as_array_mut()
        .unwrap()
        .insert(
            0,
            json!({"type":"message","input_tokens":10,"output_tokens":5}),
        );
    write(&dir.path().join("a.jsonl"), &[value, later]);
    let report = scan("claude", dir.path(), &mut store);
    assert_eq!(total(&report), 24);
    assert_eq!(
        report
            .rows
            .iter()
            .find(|r| r.model == "advisor-model")
            .unwrap()
            .total_tokens,
        9
    );
}

#[test]
fn claude_fragments_replays_and_final_usage_are_counted_once_without_body_cache() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("project/session.jsonl");
    let db = dir.path().join("cache.sqlite");
    let mut store = Store::open(Some(&db));
    let early = family(
        "msg_unique",
        Some(json!({"input_tokens":5,"output_tokens":2})),
    );
    let final_value = family(
        "msg_unique",
        Some(json!({"input_tokens":5,"output_tokens":20})),
    );
    write(
        &path,
        &[
            family("msg_unique", None),
            early,
            final_value.clone(),
            family(
                "msg_other",
                Some(json!({"input_tokens":5,"output_tokens":20})),
            ),
        ],
    );
    write(
        &dir.path().join("project/subagents/copy.jsonl"),
        &[final_value],
    );
    let report = scan("claude", dir.path(), &mut store);
    assert_eq!(total(&report), 50);
    assert!(!report.partial);
    let cached = rusqlite::Connection::open(&db).unwrap();
    let data: String = cached
        .query_row("SELECT group_concat(data) FROM usage_files", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert!(!data.contains("PRIVATE_BODY"));
    assert!(!data.contains("content"));
}

#[test]
fn incremental_append_restart_partial_tail_rewrite_replacement_and_delete() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.jsonl");
    let db = dir.path().join("cache.sqlite");
    let mut store = Store::open(Some(&db));
    write(&path, &[family("msg_1", Some(counters(100, 10)))]);
    let first = scan("claude", dir.path(), &mut store);
    assert_eq!(total(&first), 110);
    assert!(first.bytes_read > 0);
    assert_eq!(scan("claude", dir.path(), &mut store).bytes_read, 0);
    drop(store);
    let mut store = Store::open(Some(&db));
    let cached = read_at(
        "claude",
        &[dir.path().into()],
        &mut store,
        false,
        false,
        &|| false,
    )
    .unwrap()
    .unwrap();
    assert_eq!(total(&cached), 110);
    assert_eq!(cached.bytes_read, 0);
    let value = family("msg_2", Some(counters(50, 5))).to_string();
    let half = value.len() / 2;
    write!(
        fs::OpenOptions::new().append(true).open(&path).unwrap(),
        "{}",
        &value[..half]
    )
    .unwrap();
    let pending = scan("claude", dir.path(), &mut store);
    assert_eq!(total(&pending), 110);
    assert!(pending.partial);
    writeln!(
        fs::OpenOptions::new().append(true).open(&path).unwrap(),
        "{}",
        &value[half..]
    )
    .unwrap();
    let next = scan("claude", dir.path(), &mut store);
    assert_eq!(total(&next), 165);
    assert!(next.bytes_read < fs::metadata(&path).unwrap().len());
    write(&path, &[family("msg_3", Some(counters(7, 3)))]);
    assert_eq!(total(&scan("claude", dir.path(), &mut store)), 175);
    let replacement = dir.path().join("replacement");
    write(&replacement, &[family("msg_4", Some(counters(8, 4)))]);
    fs::rename(replacement, &path).unwrap();
    assert_eq!(total(&scan("claude", dir.path(), &mut store)), 187);
    fs::remove_file(path).unwrap();
    let empty = scan("claude", dir.path(), &mut store);
    assert!(empty.has_usage);
    assert_eq!(total(&empty), 187);
}

#[test]
fn codex_cumulative_full_and_tail_only_copies_stay_at_150_after_restart() {
    let dir = tempfile::tempdir().unwrap();
    let logs = dir.path().join("logs");
    let db = dir.path().join("cache.sqlite");
    let original = [meta("original"), counter(100, 0, 1), counter(150, 0, 2)];
    write(&logs.join("original.jsonl"), &original);
    let mut store = Store::open(Some(&db));
    assert_eq!(total(&scan("codex", &logs, &mut store)), 150);
    write(&logs.join("full-copy.jsonl"), &original);
    assert_eq!(total(&scan("codex", &logs, &mut store)), 150);
    write(
        &logs.join("tail-copy.jsonl"),
        &[meta("original"), counter(150, 0, 2)],
    );
    assert_eq!(total(&scan("codex", &logs, &mut store)), 150);
    drop(store);
    let mut store = Store::open(Some(&db));
    assert_eq!(total(&scan("codex", &logs, &mut store)), 150);
    fs::remove_dir_all(&logs).unwrap();
    assert_eq!(total(&scan("codex", &logs, &mut store)), 150);
    write(
        &logs.join("continued-copy.jsonl"),
        &[meta("original"), counter(150, 0, 2), counter(200, 0, 3)],
    );
    assert_eq!(total(&scan("codex", &logs, &mut store)), 200);
}

#[test]
fn codex_legacy_ledger_parents_do_not_shadow_newer_page_baselines() {
    for (fork, byte_boundary) in [(false, false), (false, true), (true, false)] {
        let dir = tempfile::tempdir().unwrap();
        let logs = dir.path().join("logs");
        let id = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
        let parent = logs.join(format!("rollout-{id}.jsonl"));
        write(&parent, &[meta("original"), counter(100, 0, 1)]);
        let mut store = Store::open(None);
        let old = cache::read_file(&parent, "codex", None, false, &|| false)
            .unwrap()
            .0;
        let legacy = parent
            .join(format!("ledger-{:x}", Sha256::digest(b"original")))
            .join(parent.file_name().unwrap());
        store
            .ledger()
            .unwrap()
            .commit(
                "codex",
                &BTreeMap::from([(legacy.clone(), old)]),
                &BTreeSet::from([legacy]),
                1,
            )
            .unwrap();
        append(&parent, counter(200, 0, 2));
        let mut page = meta(if fork { "child" } else { "original" });
        if fork {
            page["payload"]["forked_from_id"] = json!("original");
            page["timestamp"] = json!("2026-10-20T00:00:03Z");
        } else {
            page["payload"]["history_base"] = if byte_boundary {
                json!({"thread_id":id,"end_byte_offset":parent.metadata().unwrap().len()})
            } else {
                json!({"thread_id":id,"end_ordinal_exclusive":3})
            };
        }
        write(&logs.join("page.jsonl"), &[page, counter(250, 0, 4)]);
        let report = scan("codex", &logs, &mut store);
        assert_eq!(
            total(&report),
            250,
            "fork={fork}, byte_boundary={byte_boundary}"
        );
        assert!(!report.partial, "{:?}", report.warnings);
    }
}

#[test]
fn codex_equal_increments_in_distinct_pages_and_forks_are_separate_calls() {
    for fork in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(None);
        let parent = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
        write(
            &dir.path().join(format!("rollout-{parent}.jsonl")),
            &[meta("parent"), counter(100, 0, 1), counter(200, 0, 3)],
        );
        for (index, base, value) in [(1, 2, 150), (2, 4, 250)] {
            let mut header = meta(if fork { "child" } else { "parent" });
            if fork {
                header["payload"]["forked_from_id"] = json!("parent");
                header["payload"]["subagent_history_start_ordinal"] = json!(base);
                header["timestamp"] = json!("2026-10-20T00:00:04Z");
            } else {
                header["payload"]["history_base"] =
                    json!({"thread_id":parent,"end_ordinal_exclusive":base});
            }
            write(
                &dir.path().join(format!("page-{index}.jsonl")),
                &[header, counter(value, 0, 6)],
            );
        }
        assert_eq!(
            total(&scan("codex", dir.path(), &mut store)),
            300,
            "fork={fork}"
        );
    }
}

#[test]
fn codex_response_coverage_does_not_cross_distinct_page_boundaries() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(None);
    let id = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
    write(
        &dir.path().join(format!("rollout-{id}.jsonl")),
        &[meta("root"), counter(100, 0, 1), counter(200, 0, 3)],
    );
    for (index, base, value) in [(1, 2, 150), (2, 4, 250)] {
        let mut header = meta("root");
        header["payload"]["history_base"] = json!({"thread_id":id,"end_ordinal_exclusive":base});
        let mut count = counter(value, 0, 6);
        count["payload"]["info"]["last_token_usage"] = counters(50, 0);
        let mut values = vec![header, count];
        if index == 1 {
            values.push(detail("page-one-response", 50, 0, 150, 0));
        }
        write(&dir.path().join(format!("page-{index}.jsonl")), &values);
    }
    assert_eq!(total(&scan("codex", dir.path(), &mut store)), 300);
}

#[test]
fn codex_reused_path_with_a_new_history_base_keeps_both_segments() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(None);
    let id = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
    write(
        &dir.path().join(format!("rollout-{id}.jsonl")),
        &[meta("root"), counter(100, 0, 1), counter(300, 0, 3)],
    );
    let page = dir.path().join("page.jsonl");
    let mut header = meta("root");
    header["payload"]["history_base"] = json!({"thread_id":id,"end_ordinal_exclusive":2});
    write(&page, &[header.clone(), counter(150, 0, 5)]);
    assert_eq!(total(&scan("codex", dir.path(), &mut store)), 350);
    header["payload"]["history_base"]["end_ordinal_exclusive"] = json!(4);
    write(&page, &[header, counter(350, 0, 6)]);
    assert_eq!(total(&scan("codex", dir.path(), &mut store)), 400);
}

#[test]
fn codex_repeated_snapshots_and_per_response_records_do_not_add_twice() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(None);
    let mut copy = counter(150, 15, 5);
    copy["timestamp"] = json!("2026-10-20T00:00:06Z");
    let values = vec![
        meta("root"),
        json!({"type":"turn_context","payload":{"model":"gpt-example"}}),
        counter(100, 10, 2),
        detail("response-a", 100, 10, 100, 10),
        counter(100, 10, 3),
        detail("response-b", 50, 5, 150, 15),
        counter(150, 15, 5),
        copy,
    ];
    write(&dir.path().join("a.jsonl"), &values);
    write(&dir.path().join("b.jsonl"), &values);
    let report = scan("codex", dir.path(), &mut store);
    assert_eq!(total(&report), 165);
    assert!(!report.partial);
}

#[test]
fn codex_legacy_history_and_new_detailed_calls_fill_only_missing_intervals() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(None);
    write(
        &dir.path().join("a.jsonl"),
        &[
            meta("root"),
            counter(100, 10, 1),
            detail("next", 50, 5, 150, 15),
            counter(150, 15, 3),
        ],
    );
    assert_eq!(total(&scan("codex", dir.path(), &mut store)), 165);
}

#[test]
fn codex_prefers_explicit_request_usage_over_cumulative_scope_jumps() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(None);
    let mut a = counter(1000, 100, 2);
    a["payload"]["info"]["last_token_usage"] = counters(100, 10);
    let mut b = counter(2500, 200, 4);
    b["payload"]["info"]["last_token_usage"] = counters(50, 5);
    let mut repeated = b.clone();
    repeated["timestamp"] = json!("2026-10-20T00:00:05Z");
    write(&dir.path().join("a.jsonl"), &[meta("root"), a, b, repeated]);
    assert_eq!(total(&scan("codex", dir.path(), &mut store)), 165);
}

#[test]
fn codex_request_ledger_covers_counters_with_a_different_aggregate_scope_and_copied_notices() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(None);
    let mut notification = counter(500, 50, 2);
    notification["payload"]["info"]["last_token_usage"] = counters(100, 10);
    write(
        &dir.path().join("a.jsonl"),
        &[
            meta("root"),
            notification.clone(),
            detail("actual-response", 100, 10, 100, 10),
        ],
    );
    write(
        &dir.path().join("copy.jsonl"),
        &[meta("copy"), notification],
    );
    assert_eq!(total(&scan("codex", dir.path(), &mut store)), 110);
}

#[test]
fn codex_same_millisecond_equal_requests_keep_their_multiplicity() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(None);
    let mut a = counter(100, 10, 2);
    a["payload"]["info"]["last_token_usage"] = counters(100, 10);
    let mut b = counter(200, 20, 3);
    b["timestamp"] = a["timestamp"].clone();
    b["payload"]["info"]["last_token_usage"] = counters(100, 10);
    write(&dir.path().join("a.jsonl"), &[meta("root"), a, b]);
    assert_eq!(total(&scan("codex", dir.path(), &mut store)), 220);
}

#[test]
fn codex_counter_reset_never_matches_future_details_to_an_older_epoch() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(None);
    write(
        &dir.path().join("a.jsonl"),
        &[
            meta("root"),
            json!({"type":"turn_context","payload":{"model":"old-model"}}),
            counter(1000, 100, 1),
            json!({"type":"turn_context","payload":{"model":"new-model"}}),
            detail("after-reset", 50, 5, 50, 5),
            counter(50, 5, 3),
            counter(50, 5, 4),
        ],
    );
    let report = scan("codex", dir.path(), &mut store);
    assert_eq!(total(&report), 1155);
    assert!(report.partial);
    assert_eq!(
        report
            .rows
            .iter()
            .filter(|r| r.model == "old-model")
            .map(|r| r.total_tokens)
            .sum::<u64>(),
        1100
    );
    assert_eq!(
        report
            .rows
            .iter()
            .filter(|r| r.model == "new-model")
            .map(|r| r.total_tokens)
            .sum::<u64>(),
        55
    );
}

#[test]
fn codex_page_baseline_preserves_real_calls_on_both_branches() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(None);
    let id = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
    write(
        &dir.path().join(format!("rollout-{id}.jsonl")),
        &[meta("root"), counter(100, 10, 2), counter(150, 15, 5)],
    );
    let mut header = meta("root");
    header["payload"]["history_base"] = json!({"thread_id":id,"end_ordinal_exclusive":4});
    write(
        &dir.path().join("page-two.jsonl"),
        &[header, counter(120, 12, 4)],
    );
    assert_eq!(total(&scan("codex", dir.path(), &mut store)), 187);
}

#[test]
fn codex_byte_boundary_works_when_older_parent_records_have_no_ordinal() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(None);
    let id = "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb";
    let parent = dir.path().join(format!("rollout-{id}.jsonl"));
    let mut old = counter(100, 10, 2);
    old.as_object_mut().unwrap().remove("ordinal");
    write(&parent, &[meta("root"), old]);
    let mut header = meta("root");
    header["payload"]["history_base"] = json!({"thread_id":id,"end_byte_offset":fs::metadata(&parent).unwrap().len(),"end_ordinal_exclusive":10});
    write(
        &dir.path().join("child.jsonl"),
        &[header, counter(150, 15, 12)],
    );
    assert_eq!(total(&scan("codex", dir.path(), &mut store)), 165);
}

#[test]
fn codex_fast_new_page_requests_do_not_look_like_a_fork_replay() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(None);
    let id = "cccccccc-cccc-cccc-cccc-cccccccccccc";
    let parent = dir.path().join(format!("rollout-{id}.jsonl"));
    write(&parent, &[meta("root"), counter(100, 10, 1)]);
    let mut header = meta("root");
    header["payload"]["history_base"] =
        json!({"thread_id":id,"end_byte_offset":fs::metadata(&parent).unwrap().len()});
    header["timestamp"] = json!("2026-10-20T00:00:10Z");
    let mut a = counter(110, 11, 2);
    a["timestamp"] = json!("2026-10-20T00:00:10Z");
    let mut b = counter(120, 12, 3);
    b["timestamp"] = json!("2026-10-20T00:00:10.001Z");
    write(&dir.path().join("page.jsonl"), &[header, a, b]);
    assert_eq!(total(&scan("codex", dir.path(), &mut store)), 132);
}

#[test]
fn codex_fork_drops_inherited_prefix_but_keeps_distinct_child_calls() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(None);
    write(
        &dir.path().join("parent.jsonl"),
        &[meta("parent"), counter(100, 10, 2)],
    );
    let mut header = meta("child");
    header["payload"]["forked_from_id"] = json!("parent");
    header["payload"]["subagent_history_start_ordinal"] = json!(50);
    write(
        &dir.path().join("child.jsonl"),
        &[header, counter(100, 10, 2), counter(150, 15, 51)],
    );
    assert_eq!(total(&scan("codex", dir.path(), &mut store)), 165);
}

#[test]
fn codex_advanced_subagent_history_marker_does_not_hide_new_child_requests() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(None);
    let mut a = counter(100, 10, 1);
    a["payload"]["info"]["last_token_usage"] = counters(100, 10);
    let mut b = counter(150, 15, 2);
    b["payload"]["info"]["last_token_usage"] = counters(50, 5);
    write(
        &dir.path().join("parent.jsonl"),
        &[meta("parent"), a.clone(), b.clone()],
    );
    let mut header = meta("child");
    header["payload"]["forked_from_id"] = json!("parent");
    header["payload"]["subagent_history_start_ordinal"] = json!(100);
    header["timestamp"] = json!("2026-10-20T00:00:10Z");
    a["timestamp"] = json!("2026-10-20T00:00:10Z");
    b["timestamp"] = json!("2026-10-20T00:00:10.001Z");
    let mut own = counter(170, 17, 10);
    own["timestamp"] = json!("2026-10-20T00:00:20Z");
    own["payload"]["info"]["last_token_usage"] = counters(20, 2);
    // 旧版头部的 marker 已推进到本次调用之后，只有前两条是复制的父历史。
    write(&dir.path().join("child.jsonl"), &[header, a, b, own]);
    assert_eq!(total(&scan("codex", dir.path(), &mut store)), 187);
}

#[test]
fn codex_rewritten_initial_burst_stops_before_new_calls_even_when_parent_counters_changed() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(None);
    write(
        &dir.path().join("parent.jsonl"),
        &[meta("parent"), counter(100, 10, 2)],
    );
    let mut header = meta("child");
    header["payload"]["forked_from_id"] = json!("parent");
    header["timestamp"] = json!("2026-10-20T00:00:10Z");
    let mut copied_a = counter(80, 8, 1);
    copied_a["timestamp"] = json!("2026-10-20T00:00:10Z");
    copied_a["payload"]["info"]["last_token_usage"] = counters(80, 8);
    let mut copied_b = counter(100, 10, 2);
    copied_b["timestamp"] = json!("2026-10-20T00:00:10.001Z");
    copied_b["payload"]["info"]["last_token_usage"] = counters(20, 2);
    let mut own = counter(105, 15, 3);
    own["timestamp"] = json!("2026-10-20T00:00:15Z");
    own["payload"]["info"]["last_token_usage"] = counters(5, 5);
    write(
        &dir.path().join("child.jsonl"),
        &[header, copied_a, copied_b, own],
    );
    assert_eq!(total(&scan("codex", dir.path(), &mut store)), 120);
}

#[test]
fn codex_fork_without_copied_prefix_starts_at_parent_counter() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(None);
    write(
        &dir.path().join("parent.jsonl"),
        &[meta("parent"), counter(100, 10, 2)],
    );
    let mut header = meta("child");
    header["payload"]["forked_from_id"] = json!("parent");
    header["payload"]["timestamp"] = json!("2026-10-20T00:00:03Z");
    header["timestamp"] = json!("2026-10-20T00:00:03Z");
    let mut own = counter(150, 15, 4);
    own["payload"]["info"]["last_token_usage"] = counters(50, 5);
    write(&dir.path().join("child.jsonl"), &[header, own]);
    assert_eq!(total(&scan("codex", dir.path(), &mut store)), 165);
}

#[test]
fn codex_fork_uses_envelope_creation_time_instead_of_inherited_session_start() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(None);
    write(
        &dir.path().join("parent.jsonl"),
        &[meta("parent"), counter(100, 10, 1)],
    );
    let mut header = meta("child");
    header["payload"]["forked_from_id"] = json!("parent");
    header["timestamp"] = json!("2026-10-20T00:00:10Z");
    let mut copied = counter(999, 90, 1);
    copied["payload"]["info"]["last_token_usage"] = counters(100, 10);
    let mut own = counter(1000, 100, 11);
    own["payload"]["info"]["last_token_usage"] = counters(5, 5);
    write(&dir.path().join("child.jsonl"), &[header, copied, own]);
    assert_eq!(total(&scan("codex", dir.path(), &mut store)), 120);
}

#[test]
fn codex_retains_recorded_prefix_when_the_parent_log_is_not_available() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(None);
    let mut header = meta("child");
    header["payload"]["forked_from_id"] = json!("missing");
    header["payload"]["subagent_history_start_ordinal"] = json!(100);
    header["timestamp"] = json!("2026-10-20T00:00:10Z");
    let mut a = counter(100, 10, 1);
    a["payload"]["info"]["last_token_usage"] = counters(100, 10);
    let mut b = counter(150, 25, 2);
    b["payload"]["info"]["last_token_usage"] = counters(50, 15);
    write(&dir.path().join("child.jsonl"), &[header, a, b]);
    let report = scan("codex", dir.path(), &mut store);
    assert_eq!(total(&report), 175);
    assert!(report.partial);
}

#[test]
fn codex_legacy_fork_replayed_with_new_timestamps_does_not_rebill_parent_requests() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(None);
    let mut a = counter(100, 10, 1);
    a["payload"]["info"]["last_token_usage"] = counters(100, 10);
    let mut b = counter(150, 15, 2);
    b["payload"]["info"]["last_token_usage"] = counters(50, 5);
    let mut c = counter(170, 17, 3);
    c["payload"]["info"]["last_token_usage"] = counters(20, 2);
    write(
        &dir.path().join("parent.jsonl"),
        &[meta("parent"), a.clone(), b.clone(), c],
    );
    let mut header = meta("child");
    header["payload"]["forked_from_id"] = json!("parent");
    header["timestamp"] = json!("2026-10-20T00:00:10Z");
    a["timestamp"] = json!("2026-10-20T00:00:20Z");
    b["timestamp"] = json!("2026-10-20T00:00:21Z");
    let mut own = counter(175, 20, 22);
    own["payload"]["info"]["last_token_usage"] = counters(25, 5);
    write(&dir.path().join("child.jsonl"), &[header, a, b, own]);
    assert_eq!(total(&scan("codex", dir.path(), &mut store)), 217);
}

#[test]
fn missing_codex_base_uses_last_request_and_reports_partial_instead_of_inherited_total() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(None);
    let mut header = meta("root");
    header["payload"]["history_base"] = json!({"thread_id":"missing","end_ordinal_exclusive":10});
    let mut value = counter(1000, 100, 12);
    value["payload"]["info"]["last_token_usage"] = counters(10, 1);
    write(&dir.path().join("child.jsonl"), &[header, value]);
    let report = scan("codex", dir.path(), &mut store);
    assert_eq!(total(&report), 11);
    assert!(report.partial);
}

#[test]
fn pi_counts_all_usage_kinds_and_dedupes_fork_lineage_without_merging_unrelated_ids() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(None);
    let time = "2026-10-20T00:00:00Z";
    let assistant = json!({"type":"message","id":"a","timestamp":time,"message":{"role":"assistant","provider":"p","model":"m","usage":{"input":2,"output":3,"cacheRead":4,"cacheWrite":5,"reasoning":2}}});
    let values = vec![
        json!({"type":"session","id":"parent"}),
        assistant.clone(),
        json!({"type":"usage","id":"b","timestamp":time,"provider":"p","model":"m","usage":{"input":1,"output":0,"cacheRead":2}}),
        json!({"type":"compaction","id":"c","timestamp":time,"usage":{"input":7,"output":8}}),
        json!({"type":"branch_summary","id":"d","timestamp":time,"usage":{"input":1,"output":2}}),
        json!({"type":"message","id":"e","timestamp":time,"message":{"role":"toolResult","usage":{"input":1,"output":1}}}),
    ];
    write(&dir.path().join("parent.jsonl"), &values);
    write(
        &dir.path().join("fork.jsonl"),
        &[
            json!({"type":"session","id":"child","parentSession":"parent.jsonl"}),
            assistant.clone(),
            json!({"type":"usage","id":"new","timestamp":time,"usage":{"input":1,"output":1}}),
        ],
    );
    write(
        &dir.path().join("unrelated.jsonl"),
        &[json!({"type":"session","id":"other"}), assistant],
    );
    assert_eq!(total(&scan("pi", dir.path(), &mut store)), 53);
}

#[test]
fn qoder_keeps_recorded_lower_bound_without_adding_ambiguous_cache() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(None);
    write(
        &dir.path().join("a.jsonl"),
        &[
            family(
                "msg_a",
                Some(json!({"input_tokens":100,"output_tokens":10,"cache_read_input_tokens":80})),
            ),
            family("msg_missing", None),
        ],
    );
    let report = scan("qoder", dir.path(), &mut store);
    assert_eq!(total(&report), 110);
    assert!(report.partial);
    assert_eq!(report.rows[0].tokens.cache_read_tokens, 80);
}

#[test]
fn opencode_wal_updates_existing_assistant_without_counting_user_or_duplicate_steps() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("opencode.db");
    let c = rusqlite::Connection::open(&path).unwrap();
    c.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0; CREATE TABLE message(id TEXT,session_id TEXT,time_created INTEGER,time_updated INTEGER,data TEXT); CREATE TABLE part(id TEXT,data TEXT);").unwrap();
    let m = json!({"role":"assistant","modelID":"m","providerID":"p","tokens":{"input":10,"output":15,"reasoning":5,"cache":{"read":30,"write":40},"total":100}});
    c.execute(
        "INSERT INTO message VALUES ('a','s',1792454400000,1,?1)",
        [m.to_string()],
    )
    .unwrap();
    c.execute(
        "INSERT INTO message VALUES ('user','s',1792454400000,1,?1)",
        [json!({"role":"user","tokens":{"input":999,"output":999}}).to_string()],
    )
    .unwrap();
    c.execute(
        "INSERT INTO part VALUES ('step',?1)",
        [json!({"type":"step-finish","tokens":m["tokens"]}).to_string()],
    )
    .unwrap();
    let mut store = Store::open(None);
    assert_eq!(total(&scan("opencode", dir.path(), &mut store)), 100);
    assert_eq!(scan("opencode", dir.path(), &mut store).bytes_read, 0);
    let mut updated = m;
    updated["tokens"]["output"] = json!(25);
    updated["tokens"]["total"] = json!(110);
    c.execute(
        "UPDATE message SET data=?1,time_updated=2 WHERE id='a'",
        [updated.to_string()],
    )
    .unwrap();
    assert_eq!(total(&scan("opencode", dir.path(), &mut store)), 110);
    c.execute("DELETE FROM message WHERE id='a'", []).unwrap();
    assert_eq!(total(&scan("opencode", dir.path(), &mut store)), 110);
    c.execute(
        "INSERT INTO message VALUES ('a','s',1792454400000,3,?1)",
        [updated.to_string()],
    )
    .unwrap();
    assert_eq!(total(&scan("opencode", dir.path(), &mut store)), 110);
}

#[test]
fn opencode_completed_fork_copy_can_fill_the_last_unseen_stream_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let db = rusqlite::Connection::open(dir.path().join("opencode.db")).unwrap();
    db.execute_batch("CREATE TABLE session(id TEXT,time_created INTEGER);CREATE TABLE message(id TEXT,session_id TEXT,time_created INTEGER,data TEXT);INSERT INTO session VALUES ('original',1792454399000),('fork',1792454410000);").unwrap();
    let mut reply = json!({"role":"assistant","time":{"created":1792454400000i64},"modelID":"m","tokens":{"input":100,"output":0}});
    db.execute(
        "INSERT INTO message VALUES ('original-message','original',1792454400000,?1)",
        [reply.to_string()],
    )
    .unwrap();
    let mut store = Store::open(None);
    assert_eq!(total(&scan("opencode", dir.path(), &mut store)), 100);
    // 原文件末帧尚未采集就被外部删除，但分叉已经复制了那份完整计量。
    reply["tokens"]["output"] = json!(10);
    db.execute(
        "INSERT INTO message VALUES ('copied-message','fork',1792454400000,?1)",
        [reply.to_string()],
    )
    .unwrap();
    db.execute("DELETE FROM message WHERE session_id='original'", [])
        .unwrap();
    assert_eq!(total(&scan("opencode", dir.path(), &mut store)), 110);
}

#[test]
fn compressed_codex_logs_and_scope_isolation_survive_cache_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let db = dir.path().join("cache.sqlite");
    let content = format!("{}\n{}\n", meta("compressed"), counter(10, 1, 2));
    fs::write(
        dir.path().join("log.jsonl.zst"),
        zstd::stream::encode_all(content.as_bytes(), 1).unwrap(),
    )
    .unwrap();
    let mut store = Store::open(Some(&db));
    assert_eq!(total(&scan("codex", dir.path(), &mut store)), 11);
    assert_eq!(scan("codex", dir.path(), &mut store).bytes_read, 0);
    assert_eq!(total(&scan("codex", other.path(), &mut store)), 11);
    assert!(!scan("claude", other.path(), &mut store).has_usage);
    drop(store);
    let mut store = Store::open(Some(&db));
    assert_eq!(total(&scan("codex", dir.path(), &mut store)), 11);
}

#[test]
fn cancellation_does_not_publish_partial_cache_and_corrupt_cache_falls_back() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("cache.sqlite");
    let path = dir.path().join("a.jsonl");
    write(&path, &[family("msg_one", Some(counters(10, 1)))]);
    let mut store = Store::open(Some(&db));
    scan("claude", dir.path(), &mut store);
    append(&path, family("msg_two", Some(counters(10, 1))));
    assert!(read_at(
        "claude",
        &[dir.path().into()],
        &mut store,
        true,
        false,
        &|| true
    )
    .is_err());
    assert_eq!(
        total(
            &read_at(
                "claude",
                &[dir.path().into()],
                &mut store,
                false,
                false,
                &|| false
            )
            .unwrap()
            .unwrap()
        ),
        11
    );
    drop(store);
    fs::write(&db, "BROKEN CACHE").unwrap();
    let mut store = Store::open(Some(&db));
    assert_eq!(total(&scan("claude", dir.path(), &mut store)), 22);
}

#[test]
fn export_usage_fixture_for_browser_if_requested() {
    let Ok(path) = std::env::var("ASM_USAGE_EXPORT") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(None);
    let mut reports = Vec::new();
    for (agent, input, output) in [("qoder", 100, 10), ("claude", 200, 20)] {
        let root = dir.path().join(agent);
        let mut value = family(
            &format!("msg_{agent}"),
            Some(
                json!({"input_tokens":input,"output_tokens":output,"cache_read_input_tokens":if agent=="qoder" {80}else{0}}),
            ),
        );
        value["message"]["model"] = json!(format!("{agent}-model"));
        write(&root.join("a.jsonl"), &[value]);
        reports.push(scan(agent, &root, &mut store));
    }
    let root = dir.path().join("codex");
    write(
        &root.join("a.jsonl"),
        &[
            meta("root"),
            json!({"type":"turn_context","payload":{"model":"gpt-example"}}),
            counter(300, 30, 2),
        ],
    );
    reports.push(scan("codex", &root, &mut store));
    let root = dir.path().join("opencode");
    fs::create_dir_all(&root).unwrap();
    let c = rusqlite::Connection::open(root.join("opencode.db")).unwrap();
    c.execute_batch(
        "CREATE TABLE message(id TEXT,session_id TEXT,time_created INTEGER,data TEXT);",
    )
    .unwrap();
    c.execute("INSERT INTO message VALUES ('a','s',1792454400000,?1)",[json!({"role":"assistant","modelID":"opencode-model","providerID":"test","tokens":{"input":400,"output":40,"cache":{"read":0,"write":0}}}).to_string()]).unwrap();
    reports.push(scan("opencode", &root, &mut store));
    reports.push(scan("pi", &dir.path().join("pi"), &mut store));
    use std::os::unix::fs::OpenOptionsExt;
    let mut out = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .unwrap();
    out.write_all(&serde_json::to_vec(&reports).unwrap())
        .unwrap();
}
