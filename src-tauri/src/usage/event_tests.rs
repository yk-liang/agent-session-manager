use super::*;
use events::{EventPage, EventQuery};
use request_parse::RequestOutcome;
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
    // 测试先完成同步索引，再验证只读分页；生产查询不会替采集器写账本。
    for agent in AGENTS {
        events::refresh(&mut store.ledger().unwrap().db, agent, &|| false).unwrap();
    }
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
    let samples = normalize::records(agent, &files);
    let expected: BTreeMap<_, _> = normalize::unique(
        agent,
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
        let result = ingest::sync_at(agent, &[logs], &mut store, &|| false).unwrap();
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
    let mut first_chunk = claude("a", t + 1000, 10, false);
    first_chunk["message"]["usage"]["cache_creation"] =
        json!({"ephemeral_5m_input_tokens":20,"ephemeral_1h_input_tokens":30});
    write(&path, &[start("a", t), first_chunk]);
    let mut store = Store::open_with_ledger(None, Some(&ledger));
    scan("claude", &logs, &mut store);
    let first = page(&mut store, EventQuery::default());
    assert_eq!(first.total, 1);
    assert_eq!(first.events[0].output_tps, Some(10.0));
    assert_eq!(first.events[0].status, RequestOutcome::Success);
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
    assert_eq!(e.status, RequestOutcome::Success);
    assert_eq!(e.tokens.input_tokens, 550);
    assert_eq!(e.tokens.cache_write_1h_tokens, 30);
    assert_eq!(e.output_tps, Some(50.0));
    assert_eq!(e.request.latency_ms, Some(2000));
    assert_eq!(e.request.timing.as_deref(), Some("observed"));
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
    lines[0]["payload"]["originator"] = json!("codex-tui");
    lines[3]["payload"]["thread_settings"]["model"] = json!("gpt-from-settings");
    lines.splice(
        4..4,
        [
            codex(t + 300, "event_msg", json!({"type":"item_completed","item":{"type":"UserMessage"},"completed_at_ms":t+300})),
            codex(t + 1800, "event_msg", json!({"type":"item_completed","item":{"type":"Reasoning"},"completed_at_ms":t+1800})),
            codex(t + 1990, "event_msg", json!({"type":"item_completed","item":{"type":"FileChange"},"completed_at_ms":t+1990})),
        ],
    );
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
    assert_eq!(e.model, "gpt-from-settings");
    assert_eq!(e.request.client.as_deref(), Some("codex-tui"));
    assert_eq!(e.request.kind.as_deref(), Some("cli"));
    assert_eq!(e.request.started_at, Some(t + 300));
    assert_eq!(e.request.latency_ms, Some(1700));
    assert_eq!(
        e.request.timing.as_deref(),
        Some(codex::BOUNDARY_ESTIMATE)
    );
    assert_eq!(e.request.ttft_ms, Some(100));
    assert_eq!(e.request.ttft_scope.as_deref(), Some("request"));
    assert_eq!(e.request.tier.as_deref(), Some("priority"));
    append(
        &p,
        codex(
            t + 3000,
            "event_msg",
            json!({"type":"error","message":"Connection closed before the response finished"}),
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
    assert_eq!(aggregate.request.latency_ms, None);
    assert_eq!(aggregate.status, RequestOutcome::Success);
    let failed = page(
        &mut store,
        EventQuery {
            status: Some("failed".into()),
            ..Default::default()
        },
    );
    assert_eq!(failed.total, 1);
    assert_eq!(failed.events[0].request.scope.as_deref(), Some("error"));
    assert_eq!(
        failed.events[0].request.error.as_ref().unwrap().message.as_deref(),
        Some("Connection closed before the response finished")
    );

    let ledger = store.ledger().unwrap();
    let changed = ledger
        .db
        .execute(
            "UPDATE usage_request_events SET data=json_set(data,'$.request.kind','vscode')
         WHERE id=?1 AND json_extract(data,'$.request.kind')='cli'",
            [&e.id],
        )
        .unwrap();
    assert_eq!(changed, 1);
    let old_page = page(&mut store, EventQuery::default());
    let indexed = old_page
        .events
        .iter()
        .find(|event| event.id == e.id)
        .unwrap();
    assert_eq!(indexed.request.kind.as_deref(), Some("cli"));
    append(
        &p,
        codex(
            t + 5000,
            "compacted",
            json!({"compaction_response_id":"response-one"}),
        ),
    );
    scan("codex", dir.path(), &mut store);
    let compacted = page(&mut store, EventQuery::default());
    assert_eq!(compacted.total, result.total);
    let response = compacted
        .events
        .iter()
        .find(|event| event.id == e.id)
        .unwrap();
    assert_eq!(response.request.kind.as_deref(), Some("compaction"));
    assert_eq!(response.request.latency_ms, None);
    assert_eq!(response.request.ttft_ms, None);
}

#[test]
fn request_events_codex_ignores_unchanged_counters_and_context_snapshots_for_timing() {
    let dir = tempfile::tempdir().unwrap();
    let t = 1_790_000_000_000i64;
    let counter = |time, input, output, total| {
        let usage = json!({"input_tokens":input,"output_tokens":output,"total_tokens":total});
        codex(time, "event_msg", json!({"type":"token_count","info":{
            "total_token_usage":usage,"last_token_usage":usage
        }}))
    };
    let path = dir.path().join("rollout.jsonl");
    let mut baseline = counter(t - 1000, 600, 40, 640);
    baseline["payload"]["info"]["last_token_usage"] =
        json!({"input_tokens":0,"output_tokens":0,"total_tokens":16497});
    write(&path, &[
        codex(t - 10_000, "session_meta", json!({"id":"codex-session"})),
        baseline,
        counter(t - 300, 50, 10, 60),
        codex(t, "event_msg", json!({"type":"task_started","turn_id":"new-turn"})),
        counter(t + 1000, 50, 10, 60),
        counter(t + 1200, 0, 0, 16497),
        counter(t + 4000, 100, 25, 125),
        codex(t + 5000, "event_msg", json!({"type":"task_complete","turn_id":"new-turn","time_to_first_token_ms":2000})),
    ]);
    let mut store = Store::open(None);
    scan("codex", dir.path(), &mut store);
    let page = page(&mut store, EventQuery::default());
    assert_eq!(page.total, 2);
    let latest = &page.events[0];
    assert_eq!(latest.request.latency_ms, Some(4000));
    assert_eq!(latest.request.ttft_ms, Some(2000));
    assert_eq!(latest.request.ttft_scope.as_deref(), Some("request"));
    assert_eq!(latest.tokens.unclassified_tokens, 0);
}

#[test]
fn request_events_replaces_bad_codex_timing_in_existing_ledger() {
    let dir = tempfile::tempdir().unwrap();
    let logs = dir.path().join("logs");
    let path = logs.join("rollout.jsonl");
    let t = 1_790_000_000_000i64;
    let usage = json!({"input_tokens":1000,"output_tokens":100});
    let mut lines = codex_lines(t);
    lines.pop();
    lines.extend([
        codex(t + 13_990, "event_msg", json!({"type":"item_completed","item":{"type":"CommandExecution"}})),
        codex(t + 14_000, "token_usage_record", json!({"turn_id":"turn-one","response_id":"response-two","usage":usage})),
        codex(t + 14_500, "event_msg", json!({"type":"task_complete","turn_id":"turn-one","time_to_first_token_ms":400})),
    ]);
    write(&path, &lines);
    let ledger = dir.path().join("ledger.sqlite");
    let cache_path = dir.path().join("cache.sqlite");
    let mut store = Store::open_with_ledger(Some(&cache_path), Some(&ledger));
    let (mut old, _) = cache::read_file(&path, "codex", None, false, &|| false).unwrap();
    old.parser_version -= 1;

    let bad_timing = |request: &mut request_parse::RequestMeta| {
        if let Some(end) = request.finished_at {
            request.started_at = Some(end - 10);
            request.latency_ms = Some(10);
            request.timing = Some("estimated".into());
        }
    };
    for sample in &mut old.samples {
        bad_timing(&mut sample.request);
        sample.request.tier = Some("stale-tier".into());
    }
    let mut archive = old.clone();
    let archive_path = logs.join("removed-rollout.jsonl");
    archive.origin_path = Some(archive_path.clone());
    old.state.session = "copied-parent".into();
    let old_store = store.ledger().unwrap();
    old_store
        .commit(
            "codex",
            &BTreeMap::from([(path.clone(), old), (archive_path.clone(), archive)]),
            &BTreeSet::from([path, archive_path]),
            t,
        )
        .unwrap();
    events::refresh(&mut old_store.db, "codex", &|| false).unwrap();
    old_store.db.execute(
        "UPDATE usage_request_events SET data=json_set(data,'$.request.timing','estimated','$.request.latencyMs',10,'$.outputTps',10000)",
        [],
    ).unwrap();
    let before = page(&mut store, EventQuery::default());
    assert_eq!(before.total, 2);
    assert!(before
        .events
        .iter()
        .all(|e| e.request.latency_ms.is_none() && e.output_tps.is_none()));
    let summary = summary::read_from(&mut store, Default::default(), &|| false).unwrap();
    assert!(summary
        .rows
        .iter()
        .all(|row| row.metrics.latency_count == 0 && row.metrics.tps_count == 0));
    drop(store);

    let mut store = Store::open_with_ledger(Some(&cache_path), Some(&ledger));
    scan("codex", &logs, &mut store);
    let recalculated = page(&mut store, EventQuery::default());
    assert_eq!(recalculated.total, 2);
    assert_eq!(recalculated.events[0].request.latency_ms, Some(11_950));
    for event in recalculated.events {
        assert_eq!(event.request.tier.as_deref(), Some("priority"));
        assert_eq!(event.session, "codex-session");
        let previous = before
            .events
            .iter()
            .find(|previous| previous.id == event.id)
            .unwrap();
        assert_eq!(event.tokens, previous.tokens);
    }
}

#[test]
fn request_events_numbered_pages_filters_and_refresh_anchor() {
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
    store
        .ledger()
        .unwrap()
        .db
        .execute(
            "UPDATE usage_request_events SET data=json_set(data,'$.modelName','Old display name')",
            [],
        )
        .unwrap();
    let second = page(
        &mut store,
        EventQuery {
            page: Some(2),
            model: Some("claude-sonnet".into()),
            ..Default::default()
        },
    );
    assert_eq!(second.events.len(), 50);
    assert!(second
        .events
        .iter()
        .all(|event| event.model == "claude-sonnet"));
    let third = page(
        &mut store,
        EventQuery {
            page: Some(3),
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
    assert_eq!(
        (first.page, second.page, third.page, third.page_size),
        (1, 2, 3, 50)
    );
    let last = page(
        &mut store,
        EventQuery {
            page: Some(u64::MAX),
            ..Default::default()
        },
    );
    assert_eq!(last.page, 3);
    assert_eq!(last.events[0].id, third.events[0].id);
    let large = page(
        &mut store,
        EventQuery {
            page: Some(0),
            limit: Some(1000),
            ..Default::default()
        },
    );
    assert_eq!(
        (large.page, large.page_size, large.events.len()),
        (1, 200, 105)
    );
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
        let empty = page(
            &mut store,
            EventQuery {
                page: Some(10),
                ..query
            },
        );
        assert_eq!((empty.total, empty.page, empty.events.len()), (0, 1, 0));
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
    // 新增记录让原第 2 页末尾跨到第 3 页；按可见请求定位，不能回到最新一页。
    let anchor = second.events.last().unwrap().id.clone();
    let refreshed = page(
        &mut store,
        EventQuery {
            page: Some(2),
            anchor: Some(anchor.clone()),
            ..Default::default()
        },
    );
    assert_eq!((refreshed.total, refreshed.page), (106, 3));
    assert_eq!(refreshed.events[0].id, anchor);
    assert_ne!(refreshed.revision, first.revision);
    let latest = page(&mut store, EventQuery::default());
    assert_eq!(latest.events[0].time, t + 1000);
    let missing = page(
        &mut store,
        EventQuery {
            page: Some(2),
            anchor: Some("removed".into()),
            ..Default::default()
        },
    );
    assert_eq!(missing.page, 2);
    let resized = page(
        &mut store,
        EventQuery {
            page: Some(2),
            limit: Some(100),
            ..Default::default()
        },
    );
    assert_eq!(
        (resized.page, resized.page_size, resized.events.len()),
        (2, 100, 6)
    );
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
    error["error"] = json!("rate_limit");
    error["message"]["content"] = json!([
        {"type":"text","text":"Rate limit exceeded."},
        {"type":"tool_use","input":{"text":"PRIVATE_TOOL_ARGUMENTS"}}
    ]);
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
        .find(|e| e.status == RequestOutcome::Success)
        .unwrap();
    assert_eq!(success.session, "session-one");
    assert_eq!(success.output_tps, Some(100.0));
    let failed = result
        .events
        .iter()
        .find(|e| e.status == RequestOutcome::Failed)
        .unwrap();
    assert_eq!(failed.request.http_status, Some(429));
    let error = failed.request.error.as_ref().unwrap();
    assert_eq!(error.kind.as_deref(), Some("rate_limit"));
    assert_eq!(error.message.as_deref(), Some("Rate limit exceeded."));
    assert!(success.request.error.is_none());
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
    assert_eq!(result.events[0].status, RequestOutcome::Success);
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
    assert!(
        events::refresh(&mut store.ledger().unwrap().db, "claude", &|| {
            calls.set(calls.get() + 1);
            calls.get() > 20
        })
        .is_err()
    );
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
        let id = i.to_string();
        let mut response = claude(&id, time + 2000, 100 + i as u64, true);
        if i == 107 {
            response["message"]["model"] = json!("demo:free");
            response["message"]["usage"]["service_tier"] = json!("priority");
        }
        lines.extend([start(&id, time), response]);
    }
    let mut failure = claude("failed", t + 3000, 0, true);
    failure["isApiErrorMessage"] = json!(true);
    failure["apiErrorStatus"] = json!(503);
    failure["error"] = json!("overloaded");
    failure["message"]["content"] = json!([
        {"type":"text","text":"The provider is temporarily unavailable.\nTry again in a moment."}
    ]);
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
    if agent == "opencode" {
        let projects = result
            .events
            .iter()
            .filter(|event| event.request.project.is_some())
            .count();
        let efforts = result
            .events
            .iter()
            .filter(|event| event.request.effort.is_some())
            .count();
        let tiers = result
            .events
            .iter()
            .filter(|event| event.request.tier.is_some())
            .count();
        assert!(projects > 0 && efforts > 0);
        println!("OpenCode page: {projects} with project, {efforts} with reasoning effort, {tiers} with service tier");
    }
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
            page: Some(2),
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
fn request_events_opencode_v2_keeps_timing_and_error_without_body() {
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
    let data = json!({"model":{"id":"live","providerID":"local","variant":"high","serviceTier":"flex"},"agent":"build","time":{"created":t,"firstToken":t+450,"streamed":t+2000,"completed":t+6000},"tokens":{"input":100,"output":100,"reasoning":20,"cache":{"read":50,"write":0}},"finish":"stop","content":[{"type":"text","text":"PRIVATE_V2_BODY"}]});
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
    assert_eq!(e.status, RequestOutcome::Success);
    assert_eq!(e.request.project.as_deref(), Some("/fixture"));
    assert_eq!(e.request.effort.as_deref(), Some("high"));
    assert_eq!(e.request.tier.as_deref(), Some("flex"));
    assert_eq!(e.request.version.as_deref(), Some("2.2.0"));
    assert_eq!(e.output_tps, Some(60.0));
    assert_eq!(e.request.ttft_ms, Some(450));
    assert!(result.has_service_tiers);
    assert_eq!(e.request.timing.as_deref(), Some("recorded"));
    assert!(!serde_json::to_string(&result)
        .unwrap()
        .contains("PRIVATE_V2_BODY"));
    let mut failure = data;
    failure["error"] = json!({"name":"APIError","data":{
        "message":"Provider unavailable",
        "statusCode":503,
        "responseBody":"PRIVATE_RESPONSE_BODY"
    }});
    failure["tokens"] = Value::Null;
    failure["time"]["created"] = json!(t + 3000);
    failure["time"]["completed"] = json!(t + 4000);
    db.execute(
        "INSERT INTO session_message VALUES ('msg_b','ses_a','compaction',1,?1,?1,?2)",
        rusqlite::params![t + 3000, failure.to_string()],
    )
    .unwrap();
    scan("opencode", dir.path(), &mut store);
    let failed = page(
        &mut store,
        EventQuery {
            status: Some("failed".into()),
            project: Some("/fixture".into()),
            include_filters: true,
            ..Default::default()
        },
    );
    assert_eq!(failed.total, 1);
    let options = failed.filters.as_ref().unwrap();
    assert_eq!(options.projects["/fixture"], 1);
    assert_eq!(options.results["success"], 1);
    assert_eq!(options.results["failed"], 1);
    assert!(!failed.events[0].details.input_recorded);
    assert_eq!(failed.events[0].output_tps, None);
    assert_eq!(failed.events[0].request.latency_ms, Some(1000));
    assert_eq!(failed.events[0].request.ttft_ms, None);
    assert_eq!(failed.events[0].request.http_status, Some(503));
    assert_eq!(failed.events[0].request.kind.as_deref(), Some("compaction"));
    let error = failed.events[0].request.error.as_ref().unwrap();
    assert_eq!(error.kind.as_deref(), Some("APIError"));
    assert_eq!(error.message.as_deref(), Some("Provider unavailable"));
    let serialized = serde_json::to_string(&failed).unwrap();
    assert!(!serialized.contains("PRIVATE_RESPONSE_BODY"));
    assert!(!serialized.contains("PRIVATE_V2_BODY"));
}

#[test]
fn usage_excludes_a_legacy_open_code_mirror_when_native_claude_usage_is_available() {
    let dir = tempfile::tempdir().unwrap();
    let t = 1_790_000_000_000i64;
    let file = dir.path().join("storage/message/open-code/msg_mirror.json");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, json!({
        "id":"msg_mirror","role":"assistant","sessionID":"open-code",
        "modelID":"claude-agent/claude","providerID":"claude-agent",
        "time":{"created":t,"completed":t+1000},
        "tokens":{"input":10,"output":10},
        "content":[{"type":"native","native":{"sessionID":"native"}}]
    }).to_string()).unwrap();
    let mut store = Store::open(None);
    scan("opencode", dir.path(), &mut store);
    let before = page(&mut store, EventQuery::default());
    assert_eq!(before.total, 1);
    assert_eq!(before.events[0].model, "claude-agent/claude");

    let mut native = claude("native-usage", t, 10, true);
    native["sessionId"] = json!("native");
    write(&dir.path().join("native.jsonl"), &[native]);
    scan("claude", dir.path(), &mut store);
    let result = page(&mut store, EventQuery {include_filters: true, ..Default::default()});
    assert_eq!(result.total, 1);
    assert_eq!(result.events[0].agent, "claude");
    let report = summary::read_from(&mut store, Default::default(), &|| false).unwrap();
    assert_eq!(report.rows.iter().map(|row| row.record_count).sum::<u64>(), 1);
    let prices = super::pricing::Pricer::open(None, None)
        .list_model_prices_from_db(&store.ledger().unwrap().db, None)
        .unwrap();
    assert_eq!(prices.models.len(), 1);
    assert_eq!(prices.models[0].agents, ["claude"]);
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

/// 时间分布覆盖整个筛选结果（不止当前页）：按状态计数、缺时间的记录单列，
/// 且“某时段之后的记录数”正好是按时间倒序跳到该时段最新一条的分页偏移。
#[test]
fn request_timeline_counts_filtered_ledger_and_maps_buckets_to_pages() {
    let dir = tempfile::tempdir().unwrap();
    let logs = dir.path().join("logs");
    let ledger = dir.path().join("ledger.sqlite");
    let midnight = Local
        .with_ymd_and_hms(2026, 3, 10, 0, 0, 0)
        .earliest()
        .unwrap()
        .timestamp_millis();
    let t = midnight + 10 * 3_600_000;
    let mut line = claude("a", t, 10, true);
    line["message"]["usage"]["service_tier"] = json!("priority");
    line["message"]["usage"]["cache_creation"] =
        json!({"ephemeral_5m_input_tokens": 20, "ephemeral_1h_input_tokens": 30});
    write(&logs.join("a.jsonl"), &[line]);
    let mut store = Store::open_with_ledger(None, Some(&ledger));
    scan("claude", &logs, &mut store);
    let first = page(&mut store, EventQuery::default());
    assert_eq!(first.events[0].tokens.cache_write_tokens, 50);
    assert_eq!(first.events[0].tokens.cache_write_1h_tokens, 30);
    for (id, time, status) in [
        ("b", t + 3_600_000, "failed"),
        ("c", t + 7_200_000, "success"),
        ("d", t + 7_260_000, "unknown"),
        ("e", 0, "unknown"),
    ] {
        store
            .ledger()
            .unwrap()
            .db
            .execute(
                "INSERT INTO usage_request_events(id,agent,time,model,status,data,source)
                 SELECT ?1,agent,?2,model,?3,
                    json_set(data,'$.id',?1,'$.time',?2,'$.status',?3,'$.cacheWriteTokens',0,'$.cacheWrite1hTokens',0,'$.request.tier',NULL),source
                 FROM usage_request_events LIMIT 1",
                rusqlite::params![id, time, status],
            )
            .unwrap();
    }
    let day = EventQuery {
        from: Some(midnight),
        to: Some(midnight + 86_400_000),
        limit: Some(1),
        ..Default::default()
    };
    let bounded = page(&mut store, day.clone());
    assert!(bounded.has_cache_writes);
    assert!(bounded.has_service_tiers);
    assert_eq!(bounded.events[0].request.tier, None);
    assert_eq!(bounded.events[0].tokens.cache_write_tokens, 0);
    let timeline = &bounded.timeline;
    assert_eq!(
        (timeline.start, timeline.step, timeline.success.len()),
        (midnight, 15 * 60_000, 96)
    );
    assert_eq!(timeline.success.iter().sum::<u64>(), 3);
    assert_eq!(
        timeline.failed[44], 1,
        "11:00 falls in the 45th quarter hour"
    );
    assert_eq!(
        timeline.untimed.success, 0,
        "a bounded range excludes untimed records"
    );
    assert_eq!(bounded.total, 4);
    // 跳到 11:00 时段：之后的记录数即偏移，limit=1 时该页正是失败请求。
    let offset: u64 = (45..96).map(|i| timeline.success[i] + timeline.failed[i]).sum();
    let jumped = page(
        &mut store,
        EventQuery {
            page: Some(offset + 1),
            ..day
        },
    );
    assert_eq!(jumped.events[0].status, RequestOutcome::Failed);

    let all = page(&mut store, EventQuery::default());
    assert_eq!(all.timeline.untimed.success, 1);
    assert_eq!(all.total, 5);
    let successful = page(
        &mut store,
        EventQuery {
            status: Some("success".into()),
            include_filters: true,
            ..Default::default()
        },
    );
    assert_eq!(successful.total, 4);
    assert_eq!(
        successful.filters.unwrap().results,
        BTreeMap::from([("failed".into(), 1), ("success".into(), 4)])
    );
    assert!(successful.events.iter().all(|event| event.status == RequestOutcome::Success));
    let summary = summary::read_from(&mut store, summary::SummaryQuery::default(), &|| false)
        .unwrap();
    assert_eq!(
        summary.rows.iter().map(|row| row.metrics.success_count).sum::<u64>(),
        4
    );
    assert_eq!(
        summary.rows.iter().map(|row| row.metrics.failed_count).sum::<u64>(),
        1
    );
    let failed = page(
        &mut store,
        EventQuery {
            status: Some("failed".into()),
            ..Default::default()
        },
    );
    assert_eq!(failed.timeline.success.iter().sum::<u64>(), 0);
    assert_eq!(failed.timeline.failed.iter().sum::<u64>(), 1);
    assert!(!failed.has_cache_writes);
    assert!(!failed.has_service_tiers);
}
