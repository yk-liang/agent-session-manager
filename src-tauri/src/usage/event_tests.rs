use super::*;
use events::{EventPage, EventQuery};
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
fn append(path: &Path, value: Value) {
    writeln!(
        fs::OpenOptions::new().append(true).open(path).unwrap(),
        "{value}"
    )
    .unwrap();
}
fn scan(agent: &str, path: &Path, store: &mut Store) {
    ingest::collect(agent, &[path.into()], store, None, &|| false).unwrap();
}
fn page(store: &mut Store, query: EventQuery) -> EventPage {
    events::page(&mut store.ledger().unwrap().db, query, &|| false).unwrap()
}

/// 对照旧版完整归并结果，验证缩小更新范围没有遗漏跨来源的兼容/去重关系。
pub(super) fn assert_index_matches_full(store: &mut Store, agent: &str) {
    events::refresh(&mut store.ledger().unwrap().db, agent, &|| false).unwrap();
    let mut files = store.ledger().unwrap().load(agent).unwrap();
    for file in files.values_mut() {
        for sample in file.samples.iter_mut().chain(&mut file.failures) {
            if sample.request.source.is_none() {
                sample.request.source = file
                    .origin_path
                    .as_ref()
                    .map(|p| p.to_string_lossy().into());
            }
        }
    }
    let (samples, _, _) = normalize::records(agent, &files);
    let expected: BTreeMap<_, _> = normalize::unique(
        files
            .values()
            .flat_map(|f| f.failures.iter().cloned())
            .chain(samples),
    )
    .into_iter()
    .filter(|s| {
        s.model != "<synthetic>"
            && !(s.request.scope.as_deref() == Some("usage_delta") && s.tokens.total() == 0)
    })
    .map(|s| {
        let e = events::from_sample(agent, s);
        // 与 SQLite 中 JSON 的序列化/解析路径相同，避免 f64 最末位舍入差异。
        (
            e.id.clone(),
            serde_json::from_str::<Value>(&serde_json::to_string(&e).unwrap()).unwrap(),
        )
    })
    .collect();
    let mut stmt = store
        .ledger()
        .unwrap()
        .db
        .prepare("SELECT id,data FROM usage_request_events WHERE agent=?1")
        .unwrap();
    let actual: BTreeMap<_, Value> = stmt
        .query_map([agent], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })
        .unwrap()
        .map(|r| {
            let (id, data) = r.unwrap();
            (id, serde_json::from_str(&data).unwrap())
        })
        .collect();
    assert_eq!(
        actual, expected,
        "incremental/full normalization differ for {agent}"
    );
}

#[test]
fn request_updates_do_not_decode_or_rewrite_unrelated_history_after_restart() {
    for agent in ["claude", "codex"] {
        let dir = tempfile::tempdir().unwrap();
        let logs = dir.path().join("logs");
        let ledger = dir.path().join("ledger.sqlite");
        let cache = dir.path().join("cache.sqlite");
        let t = 1_790_000_000_000;
        let lines = |id: &str| {
            if agent == "codex" {
                let mut lines = codex_lines(t);
                lines[0]["payload"]["id"] = json!(id);
                lines[4]["payload"]["response_id"] = json!(id);
                // 不同请求也使用不同时间，避免将完全相同的累计通知视为复制历史。
                if id == "other" {
                    for line in &mut lines {
                        line["timestamp"] = json!(t + 10000);
                    }
                }
                lines
            } else {
                let mut line = claude(id, t, 10, true);
                line["sessionId"] = json!(id);
                vec![line]
            }
        };
        write(&logs.join("active.jsonl"), &lines("active"));
        write(&logs.join("other.jsonl"), &lines("other"));
        let mut store = Store::open_with_ledger(Some(&cache), Some(&ledger));
        scan(agent, &logs, &mut store);
        assert_index_matches_full(&mut store, agent);
        assert_eq!(page(&mut store, EventQuery::default()).total, 2);
        // 破坏无关来源的观测并禁止改写它的事件：仅查 stamps / 已存事件仍然可以同步。
        store.ledger().unwrap().db.execute(
            "UPDATE usage_ledger_sources SET data='must not be decoded' WHERE path LIKE '%/other.jsonl/%'", []
        ).unwrap();
        store
            .ledger()
            .unwrap()
            .db
            .execute_batch(
                "CREATE TRIGGER protect_old_event BEFORE UPDATE ON usage_request_events
             WHEN json_extract(OLD.data,'$.session')='other'
             BEGIN SELECT RAISE(ABORT,'unrelated event rewritten'); END;",
            )
            .unwrap();
        drop(store);
        let mut store = Store::open_with_ledger(Some(&cache), Some(&ledger));
        let line = if agent == "codex" {
            codex(
                t + 20000,
                "token_usage_record",
                json!({"response_id":"new","usage":{"input_tokens":20,"output_tokens":2}}),
            )
        } else {
            claude("new", t + 20000, 20, true)
        };
        append(&logs.join("active.jsonl"), line);
        let result = summary::sync_at(agent, &[logs], &mut store, &|| false).unwrap();
        assert_eq!(result.changed_files, 1);
        assert_eq!(page(&mut store, EventQuery::default()).total, 3);
    }
}
fn claude(id: &str, time: i64, output: u64, done: bool) -> Value {
    json!({"type":"assistant","sessionId":"session-one","timestamp":time,"requestRef":format!("req_{id}"),"cwd":"/fixture/project","version":"2.1.309","entrypoint":"cli","effort":"high",
        "message":{"id":format!("msg_{id}"),"model":"claude-sonnet","stop_reason":if done {json!("end_turn")} else {Value::Null},
            "content":[{"type":"text","text":"PRIVATE_RESPONSE_BODY"}],"usage":{"input_tokens":100,"output_tokens":output,"cache_read_input_tokens":400,"cache_creation_input_tokens":50,"service_tier":"standard"}}})
}
fn start(id: &str, time: i64) -> Value {
    json!({"type":"api-request","id":format!("req_{id}"),"timestamp":time,"querySource":"repl_main_thread","params":{"model":"claude-sonnet","messages":["PRIVATE_PROMPT"],"authorization":"SECRET_API_TOKEN"}})
}

#[test]
fn request_events_append_refine_restart_and_delete_preserve_one_response() {
    let dir = tempfile::tempdir().unwrap();
    let logs = dir.path().join("logs");
    let path = logs.join("a.jsonl");
    let ledger = dir.path().join("ledger.sqlite");
    let t = 1_790_000_000_000i64;
    write(&path, &[start("a", t), claude("a", t + 1000, 10, false)]);
    let mut store = Store::open_with_ledger(None, Some(&ledger));
    scan("claude", &logs, &mut store);
    let first = page(&mut store, EventQuery::default());
    assert_eq!(first.total, 1);
    assert_eq!(first.events[0].output_tps, Some(10.0));
    assert_eq!(first.events[0].status, "unknown");
    let end = claude("a", t + 2000, 100, true).to_string();
    fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(end.as_bytes())
        .unwrap();
    scan("claude", &logs, &mut store);
    assert_eq!(
        page(&mut store, EventQuery::default()).events[0]
            .tokens
            .output_tokens,
        10
    );
    fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(b"\n")
        .unwrap();
    scan("claude", &logs, &mut store);
    let final_page = page(&mut store, EventQuery::default());
    let e = &final_page.events[0];
    assert_eq!(final_page.total, 1);
    assert_eq!(e.id, first.events[0].id);
    assert_eq!(e.status, "success");
    assert_eq!(e.tokens.input_tokens, 550);
    assert_eq!(e.output_tps, Some(50.0));
    assert_eq!(e.request.latency_ms, Some(2000));
    assert_eq!(e.request.timing.as_deref(), Some("observed"));
    assert_eq!(e.request.cost_usd, None);
    let before = store.ledger().unwrap().db.total_changes();
    page(&mut store, EventQuery::default());
    assert_eq!(
        before,
        store.ledger().unwrap().db.total_changes(),
        "unchanged pages must not rewrite index"
    );
    drop(store);
    fs::remove_file(path).unwrap();
    let mut store = Store::open_with_ledger(None, Some(&ledger));
    scan("claude", &logs, &mut store);
    assert_eq!(
        page(&mut store, EventQuery::default()).events[0].output_tps,
        Some(50.0)
    );
    let bytes = fs::read(ledger).unwrap();
    for secret in [
        "PRIVATE_PROMPT",
        "PRIVATE_RESPONSE_BODY",
        "SECRET_API_TOKEN",
    ] {
        assert!(!bytes.windows(secret.len()).any(|b| b == secret.as_bytes()));
    }
}

fn codex(time: i64, kind: &str, payload: Value) -> Value {
    json!({"timestamp":time,"type":kind,"payload":payload})
}
fn codex_lines(t: i64) -> Vec<Value> {
    let usage = json!({"input_tokens":1000,"cached_input_tokens":800,"output_tokens":100,"reasoning_output_tokens":40});
    vec![
        codex(
            t,
            "session_meta",
            json!({"id":"codex-session","cwd":"/fixture/codex","originator":"Codex Desktop","cli_version":"0.99.0","model_provider":"openai","source":"vscode"}),
        ),
        codex(
            t,
            "event_msg",
            json!({"type":"task_started","turn_id":"turn-one"}),
        ),
        codex(
            t,
            "turn_context",
            json!({"model":"gpt-codex","effort":"high"}),
        ),
        codex(
            t,
            "event_msg",
            json!({"type":"thread_settings_applied","thread_settings":{"model_provider_id":"openai","service_tier":"priority"}}),
        ),
        codex(
            t + 2000,
            "token_usage_record",
            json!({"response_id":"response-one","turn_id":"turn-one","usage":usage,"thread_token_usage":usage}),
        ),
        codex(
            t + 2050,
            "event_msg",
            json!({"type":"token_count","info":{"total_token_usage":usage,"last_token_usage":usage}}),
        ),
        codex(
            t + 2200,
            "event_msg",
            json!({"type":"task_complete","turn_id":"turn-one","time_to_first_token_ms":400}),
        ),
    ]
}

#[test]
fn request_events_codex_dedup_turn_timing_and_aggregate_are_explicit() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("rollout.jsonl");
    let t = 1_790_000_000_000i64;
    let mut lines = codex_lines(t);
    let completed = lines.pop().unwrap();
    write(&p, &lines);
    let mut store = Store::open(None);
    scan("codex", dir.path(), &mut store);
    let first = page(&mut store, EventQuery::default());
    assert_eq!(first.total, 1);
    assert_eq!(first.events[0].request.ttft_ms, None);
    append(&p, completed);
    scan("codex", dir.path(), &mut store);
    let result = page(&mut store, EventQuery::default());
    let e = &result.events[0];
    assert_eq!(e.id, first.events[0].id);
    assert_eq!(e.output_tps, Some(50.0));
    assert_eq!(e.request.timing.as_deref(), Some("estimated"));
    assert_eq!(e.request.ttft_ms, Some(400));
    assert_eq!(e.request.ttft_scope.as_deref(), Some("turn"));
    assert_eq!(e.request.tier.as_deref(), Some("priority"));
    append(
        &p,
        codex(
            t + 3000,
            "event_msg",
            json!({"type":"error","message":"SECRET_RESPONSE_ERROR"}),
        ),
    );
    append(
        &p,
        codex(
            t + 4000,
            "event_msg",
            json!({"type":"token_count","info":{"total_token_usage":{"input_tokens":2000,"output_tokens":250}}}),
        ),
    );
    scan("codex", dir.path(), &mut store);
    let result = page(&mut store, EventQuery::default());
    assert_eq!(result.total, 3);
    let aggregate = result
        .events
        .iter()
        .find(|e| e.request.scope.as_deref() == Some("usage_delta"))
        .unwrap();
    assert_eq!(aggregate.output_tps, None);
    let failed = page(
        &mut store,
        EventQuery {
            status: Some("failed".into()),
            ..Default::default()
        },
    );
    assert_eq!(failed.total, 1);
    assert_eq!(failed.events[0].request.scope.as_deref(), Some("error"));
    assert!(!serde_json::to_string(&failed)
        .unwrap()
        .contains("SECRET_RESPONSE_ERROR"));
}

#[test]
fn request_events_filter_keyset_paging_and_new_data_revision() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("a.jsonl");
    let t = 1_790_000_000_000i64;
    write(
        &p,
        &(0..105)
            .map(|i| claude(&i.to_string(), t, 10, true))
            .collect::<Vec<_>>(),
    );
    let mut store = Store::open(None);
    scan("claude", dir.path(), &mut store);
    let first = page(&mut store, EventQuery::default());
    assert_eq!((first.total, first.events.len()), (105, 50));
    let second = page(
        &mut store,
        EventQuery {
            cursor: first.next_cursor.clone(),
            ..Default::default()
        },
    );
    assert_eq!(second.events.len(), 50);
    let third = page(
        &mut store,
        EventQuery {
            cursor: second.next_cursor.clone(),
            ..Default::default()
        },
    );
    assert_eq!(third.events.len(), 5);
    let ids = first
        .events
        .iter()
        .chain(&second.events)
        .chain(&third.events)
        .map(|e| &e.id)
        .collect::<BTreeSet<_>>();
    assert_eq!(ids.len(), 105);
    for query in [
        EventQuery {
            agent: Some("codex".into()),
            ..Default::default()
        },
        EventQuery {
            model: Some("missing".into()),
            ..Default::default()
        },
        EventQuery {
            status: Some("failed".into()),
            ..Default::default()
        },
        EventQuery {
            from: Some(t + 1),
            ..Default::default()
        },
        EventQuery {
            to: Some(t),
            ..Default::default()
        },
    ] {
        assert_eq!(page(&mut store, query).total, 0);
    }
    let same_time = page(
        &mut store,
        EventQuery {
            from: Some(t),
            to: Some(t + 1),
            ..Default::default()
        },
    );
    assert_eq!(same_time.total, 105);
    append(&p, claude("new", t + 1000, 30, true));
    scan("claude", dir.path(), &mut store);
    let reset = page(
        &mut store,
        EventQuery {
            cursor: first.next_cursor,
            ..Default::default()
        },
    );
    assert!(reset.reset);
    assert_eq!(reset.total, 106);
    assert_eq!(reset.events[0].time, t + 1000);
    let invalid = events::page(
        &mut store.ledger().unwrap().db,
        EventQuery {
            agent: Some("bad".into()),
            ..Default::default()
        },
        &|| false,
    );
    assert!(invalid.is_err());
}

#[test]
fn request_events_unmetered_failures_and_replayed_blocks_do_not_duplicate() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("a.jsonl");
    let t = 1_790_000_000_000i64;
    let mut error = claude("failure", t + 4000, 0, true);
    error["isApiErrorMessage"] = json!(true);
    error["apiErrorStatus"] = json!(429);
    error["message"]["usage"] = Value::Null;
    write(
        &p,
        &[start("a", t), claude("a", t + 1000, 100, true), error],
    );
    let mut replay = claude("a", t + 9000, 100, true);
    replay["isSidechain"] = json!(true);
    replay["sessionId"] = json!("fork");
    write(
        &dir.path().join("0copy.jsonl"),
        &[start("a", t + 7000), replay],
    );
    let mut store = Store::open(None);
    scan("claude", dir.path(), &mut store);
    let result = page(&mut store, EventQuery::default());
    assert_eq!(result.total, 2);
    let success = result
        .events
        .iter()
        .find(|e| e.status == "success")
        .unwrap();
    assert_eq!(success.session, "session-one");
    assert_eq!(success.output_tps, Some(100.0));
    let failed = result.events.iter().find(|e| e.status == "failed").unwrap();
    assert_eq!(failed.request.http_status, Some(429));
    assert!(!failed.details.output_recorded);
    assert_eq!(failed.output_tps, None);
}

#[test]
fn request_events_migrate_v1_ledger_even_when_source_is_gone() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("a.jsonl");
    let t = 1_790_000_000_000i64;
    write(&p, &[claude("old", t, 80, true)]);
    let (file, _) = cache::read_file(&p, "claude", None, false, &|| false).unwrap();
    let mut v = serde_json::to_value(file).unwrap();
    v["parser_version"] = json!(2);
    v["samples"][0].as_object_mut().unwrap().remove("request");
    v.as_object_mut().unwrap().remove("failures");
    let ledger = dir.path().join("v1.sqlite");
    let db = rusqlite::Connection::open(&ledger).unwrap();
    db.execute_batch("CREATE TABLE usage_ledger_sources(agent TEXT,path TEXT,data TEXT,PRIMARY KEY(agent,path));CREATE TABLE usage_ledger_updates(agent TEXT PRIMARY KEY,updated INTEGER);PRAGMA user_version=1;").unwrap();
    db.execute(
        "INSERT INTO usage_ledger_sources VALUES ('claude','old',?1)",
        [v.to_string()],
    )
    .unwrap();
    drop(db);
    fs::remove_file(p).unwrap();
    let mut store = Store::open_with_ledger(None, Some(&ledger));
    let result = page(&mut store, EventQuery::default());
    assert_eq!(result.total, 1);
    assert_eq!(result.events[0].tokens.output_tokens, 80);
    assert_eq!(result.events[0].output_tps, None);
    assert_eq!(result.events[0].status, "unknown");
}

#[test]
fn request_events_cancelled_index_update_rolls_back() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("a.jsonl");
    let t = 1_790_000_000_000i64;
    write(&p, &[claude("a", t, 10, true)]);
    let mut store = Store::open(None);
    scan("claude", dir.path(), &mut store);
    page(&mut store, EventQuery::default());
    for i in 0..80 {
        append(&p, claude(&i.to_string(), t, 10, true));
    }
    scan("claude", dir.path(), &mut store);
    let calls = std::cell::Cell::new(0);
    assert!(events::page(
        &mut store.ledger().unwrap().db,
        EventQuery::default(),
        &|| {
            calls.set(calls.get() + 1);
            calls.get() > 20
        }
    )
    .is_err());
    let count: i64 = store
        .ledger()
        .unwrap()
        .db
        .query_row("SELECT count(*) FROM usage_request_events", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(count, 1);
    assert_eq!(page(&mut store, EventQuery::default()).total, 81);
}

#[test]
fn export_request_events_fixture_if_requested() {
    let Ok(export) = std::env::var("ASM_REQUEST_EVENTS_EXPORT") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let t = Utc::now().timestamp_millis() - 60_000;
    let mut lines = Vec::new();
    for i in 0..108 {
        let time = t - i * 10_000;
        lines.extend([
            start(&i.to_string(), time),
            claude(&i.to_string(), time + 2000, 100 + i as u64, true),
        ]);
    }
    let mut failure = claude("failed", t + 3000, 0, true);
    failure["isApiErrorMessage"] = json!(true);
    failure["apiErrorStatus"] = json!(503);
    failure["message"]["usage"] = Value::Null;
    lines.push(failure);
    write(&dir.path().join("claude/a.jsonl"), &lines);
    write(&dir.path().join("codex/a.jsonl"), &codex_lines(t + 5000));
    let mut store = Store::open(None);
    scan("claude", &dir.path().join("claude"), &mut store);
    scan("codex", &dir.path().join("codex"), &mut store);
    let events = page(
        &mut store,
        EventQuery {
            limit: Some(200),
            ..Default::default()
        },
    );
    fs::write(export, serde_json::to_vec(&events).unwrap()).unwrap();
}

#[test]
fn live_request_events_if_requested() {
    let Ok(path) = std::env::var("ASM_REQUEST_LIVE_ROOT") else {
        return;
    };
    let agent = std::env::var("ASM_REQUEST_LIVE_AGENT").unwrap();
    let mut store = Store::open(None);
    scan(&agent, Path::new(&path), &mut store);
    let result = page(
        &mut store,
        EventQuery {
            limit: Some(200),
            ..Default::default()
        },
    );
    assert!(result.total > 0);
    assert!(result.events.iter().any(|e| e.output_tps.is_some()));
    assert!(result.events.iter().all(|e| e.request.source.is_some()));
    println!(
        "actual {agent}: {} retained events, {} with timing, {} with TTFT",
        result.total,
        result
            .events
            .iter()
            .filter(|e| e.output_tps.is_some())
            .count(),
        result
            .events
            .iter()
            .filter(|e| e.request.ttft_ms.is_some())
            .count()
    );
}

#[test]
fn request_events_ledger_copy_performance_if_requested() {
    let Ok(path) = std::env::var("ASM_REQUEST_LEDGER_COPY") else {
        return;
    };
    let mut store = Store::open_with_ledger(None, Some(Path::new(&path)));
    let started = Instant::now();
    let first = page(&mut store, EventQuery::default());
    let cold = started.elapsed();
    let started = Instant::now();
    let next = page(
        &mut store,
        EventQuery {
            cursor: first.next_cursor.clone(),
            ..Default::default()
        },
    );
    let warm = started.elapsed();
    assert!(first.total > 0);
    assert_eq!(next.total, first.total);
    println!(
        "ledger copy: {} events; first index {} ms; next page {} ms",
        first.total,
        cold.as_millis(),
        warm.as_millis()
    );
    assert!(
        warm.as_secs() < 2,
        "indexed pagination must not reload all historical source JSON"
    );
}

#[test]
fn request_events_opencode_v2_keeps_timing_cost_and_error_without_body() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("opencode.db");
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../tests/fixtures/opencode-v2.sql"
    )))
    .unwrap();
    let t = 1_790_000_000_000i64;
    db.execute("INSERT INTO session_v2(id,directory,version,time_created,time_updated) VALUES ('ses_a','/fixture','2.2.0',?1,?1)",[t-1000]).unwrap();
    let data = json!({"model":{"id":"live","providerID":"local"},"agent":"build","time":{"created":t,"completed":t+2000},"tokens":{"input":100,"output":100,"reasoning":20,"cache":{"read":50,"write":0}},"finish":"stop","cost":0.002,"content":[{"type":"text","text":"PRIVATE_V2_BODY"}]});
    db.execute(
        "INSERT INTO session_message VALUES ('msg_a','ses_a','assistant',0,?1,?1,?2)",
        rusqlite::params![t, data.to_string()],
    )
    .unwrap();
    let mut store = Store::open(None);
    scan("opencode", dir.path(), &mut store);
    let result = page(&mut store, EventQuery::default());
    assert_eq!(result.total, 1);
    let e = &result.events[0];
    assert_eq!(e.status, "success");
    assert_eq!(e.output_tps, Some(60.0));
    assert_eq!(e.request.cost_usd, Some(0.002));
    assert_eq!(e.request.timing.as_deref(), Some("recorded"));
    assert!(!serde_json::to_string(&result)
        .unwrap()
        .contains("PRIVATE_V2_BODY"));
    let mut failure = data;
    failure["error"] = json!({"name":"APIError","data":{"message":"PRIVATE_ERROR_BODY"}});
    failure["time"]["created"] = json!(t + 3000);
    failure["time"]["completed"] = json!(t + 4000);
    db.execute(
        "INSERT INTO session_message VALUES ('msg_b','ses_a','assistant',1,?1,?1,?2)",
        rusqlite::params![t + 3000, failure.to_string()],
    )
    .unwrap();
    scan("opencode", dir.path(), &mut store);
    let failed = page(
        &mut store,
        EventQuery {
            status: Some("failed".into()),
            ..Default::default()
        },
    );
    assert_eq!(failed.total, 1);
    assert_eq!(failed.events[0].output_tps, None);
    assert!(!serde_json::to_string(&failed)
        .unwrap()
        .contains("PRIVATE_ERROR_BODY"));
}

#[test]
fn request_events_do_not_present_claude_local_placeholders_as_calls() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("a.jsonl");
    let mut placeholder = claude("placeholder", 1_790_000_000_000, 0, true);
    placeholder["message"]["model"] = json!("<synthetic>");
    write(&p, &[placeholder]);
    let mut store = Store::open(None);
    scan("claude", dir.path(), &mut store);
    assert_eq!(page(&mut store, EventQuery::default()).total, 0);
}

#[test]
fn request_events_codex_copied_response_preserves_original_session_and_timing() {
    let dir = tempfile::tempdir().unwrap();
    let t = 1_790_000_000_000i64;
    let original = codex_lines(t);
    write(&dir.path().join("z-original.jsonl"), &original);
    let child = vec![
        codex(
            t + 5000,
            "session_meta",
            json!({"id":"copied-session","forked_from":"codex-session"}),
        ),
        original[4].clone(),
    ];
    write(&dir.path().join("a-copy.jsonl"), &child);
    let mut store = Store::open(None);
    scan("codex", dir.path(), &mut store);
    let result = page(&mut store, EventQuery::default());
    assert_eq!(result.total, 1);
    assert_eq!(result.events[0].session, "codex-session");
    assert_eq!(result.events[0].output_tps, Some(50.0));
}
