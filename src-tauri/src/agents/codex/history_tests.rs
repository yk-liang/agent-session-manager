use super::*;

pub(super) fn page(
    root: &Path,
    storage: &str,
    id: &str,
    minute: u32,
    base: Option<Value>,
    events: Vec<Value>,
) -> PathBuf {
    let dir = root.join("2099/12/31");
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!(
        "rollout-2099-12-31T00-{minute:02}-00-{storage}.jsonl"
    ));
    let mut rows = vec![json!({
        "timestamp": format!("2099-12-31T00:{minute:02}:00Z"), "type": "session_meta",
        "payload": { "id": id, "cwd": "/tmp/paged", "source": "vscode", "thread_source": "user",
            "timestamp": format!("2099-12-31T00:{minute:02}:00Z"),
            "history_mode": "paginated", "history_base": base }
    })];
    rows.extend(events);
    fs::write(
        &path,
        rows.into_iter()
            .map(|v| v.to_string() + "\n")
            .collect::<String>(),
    )
    .unwrap();
    path
}

pub(super) fn message(ordinal: u64, role: &str, text: &str) -> Value {
    json!({"timestamp":format!("2099-12-31T00:00:{ordinal:02}Z"), "ordinal":ordinal,
        "type":"response_item", "payload":{"type":"message", "role":role,
            "id":format!("message-{ordinal}"), "content":[{"type":"text","text":text}]}})
}

#[test]
fn compressed_rollout_keeps_interactive_session_visible() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("sessions");
    let path = page(
        &root,
        "compressed-storage",
        "compressed-session",
        0,
        None,
        vec![message(1, "user", "A synthetic user request")],
    );
    assert_eq!(catalog_in(&root, false).unwrap().sessions.len(), 1);

    // 仅改变同一日志的存储格式，模拟 Codex 对历史日志的压缩，保持会话内容不变。
    let bytes = fs::read(&path).unwrap();
    let compressed = path.with_extension("jsonl.zst");
    fs::write(&compressed, zstd::stream::encode_all(bytes.as_slice(), 0).unwrap()).unwrap();
    fs::remove_file(&path).unwrap();

    let catalog = catalog_in(&root, false).unwrap();
    assert_eq!(
        catalog.sessions.len(),
        1,
        "压缩存储不应使非空交互式会话从列表消失；warnings={:?}",
        catalog.warnings
    );
}

#[test]
fn cached_history_tracks_appends_new_pages_boundaries_and_title_index() {
    use std::io::Write;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("sessions");
    let first = page(
        &root,
        "cached-thread",
        "cached-thread",
        0,
        None,
        vec![message(1, "user", "first")],
    );
    assert_eq!(
        catalog_in(&root, false).unwrap().sessions[0].real_user_count,
        1
    );
    let boundary = first.metadata().unwrap().len();
    writeln!(
        fs::OpenOptions::new().append(true).open(&first).unwrap(),
        "{}",
        message(2, "user", "old tail")
    )
    .unwrap();
    assert_eq!(
        catalog_in(&root, false).unwrap().sessions[0].real_user_count,
        2
    );
    let second = page(
        &root,
        "cached-thread_page-two",
        "cached-thread",
        1,
        Some(
            json!({"thread_id":"cached-thread","end_byte_offset":boundary,"end_ordinal_exclusive":2}),
        ),
        vec![message(3, "assistant", "new answer")],
    );
    let check = catalog_in(&root, false).unwrap();
    assert_eq!(check.sessions.len(), 1);
    assert_eq!(
        check.sessions[0].real_user_count, 1,
        "新分页边界必须使旧摘要失效"
    );
    assert_eq!(check.sessions[0].assistant_text_count, 1);
    fs::write(
        temp.path().join("session_index.jsonl"),
        json!({"id":"cached-thread","thread_name":"renamed"}).to_string(),
    )
    .unwrap();
    assert_eq!(
        catalog_in(&root, false).unwrap().sessions[0]
            .title
            .as_deref(),
        Some("renamed")
    );
    fs::remove_file(second).unwrap();
    assert_eq!(
        catalog_in(&root, false).unwrap().sessions[0].real_user_count,
        2
    );
}

#[test]
fn paginated_history_is_one_session_with_bounded_ordered_detail_and_search() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("sessions");
    let first = page(
        &root,
        "paged-thread",
        "paged-thread",
        0,
        None,
        vec![
            message(1, "user", "earliest question"),
            message(2, "assistant", "earliest answer"),
        ],
    );
    let boundary = first.metadata().unwrap().len();
    // 旧页在分段后仍可能补写记录；新页指定的边界之后已不属于当前历史。
    use std::io::Write;
    writeln!(
        fs::OpenOptions::new().append(true).open(&first).unwrap(),
        "{}",
        message(3, "user", "abandoned tail")
    )
    .unwrap();
    let second = page(
        &root,
        "paged-thread_page-two",
        "paged-thread",
        1,
        Some(
            json!({"thread_id":"paged-thread","end_byte_offset":boundary,"end_ordinal_exclusive":3}),
        ),
        vec![message(4, "assistant", "latest answer only in second page")],
    );
    fs::write(
        temp.path().join("session_index.jsonl"),
        json!({"id":"paged-thread","thread_name":"same title"}).to_string(),
    )
    .unwrap();

    let catalog = catalog_in(&root, false).unwrap();
    assert!(catalog.warnings.is_empty(), "{:?}", catalog.warnings);
    assert_eq!(catalog.sessions.len(), 1, "日志分段不能重复计为会话");
    assert_eq!(catalog.projects[0].session_count, 1);
    let meta = &catalog.sessions[0];
    assert_eq!(meta.title.as_deref(), Some("same title"));
    assert_eq!(meta.source_files.len(), 2, "助手消息独占的分段也必须保留");
    assert_eq!(meta.real_user_count, 1);
    assert_eq!(meta.assistant_text_count, 2);
    assert_eq!(
        meta.size_bytes,
        Some(first.metadata().unwrap().len() + second.metadata().unwrap().len())
    );
    let detail = read_session_in(&root, "paged-thread").unwrap();
    assert_eq!(detail.events.len(), 3);
    let texts: Vec<_> = detail
        .events
        .iter()
        .map(|e| e["message"]["content"].as_str().unwrap())
        .collect();
    assert_eq!(
        texts,
        [
            "earliest question",
            "earliest answer",
            "latest answer only in second page"
        ]
    );
    assert_eq!(
        CodexSource.search_events(meta).unwrap(),
        detail.events,
        "搜索与详情必须使用同一份合并历史"
    );
}

#[test]
fn same_title_with_different_ids_remains_two_sessions() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("sessions");
    for id in ["thread-a", "thread-b"] {
        page(
            &root,
            id,
            id,
            0,
            None,
            vec![message(1, "user", "same title")],
        );
    }
    let catalog = catalog_in(&root, false).unwrap();
    assert_eq!(catalog.sessions.len(), 2);
    assert_ne!(
        catalog.sessions[0].session_id,
        catalog.sessions[1].session_id
    );
}

#[test]
fn user_pages_group_by_id_and_respect_ordinal_boundary() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("sessions");
    page(
        &root,
        "thread",
        "thread",
        0,
        None,
        vec![
            message(1, "user", "first question"),
            message(2, "assistant", "first answer"),
            message(3, "user", "discard this tail"),
        ],
    );
    page(
        &root,
        "thread_next",
        "thread",
        1,
        Some(json!({"thread_id":"thread","end_ordinal_exclusive":3})),
        vec![
            message(3, "user", "second question"),
            message(4, "assistant", "second answer"),
        ],
    );
    let catalog = catalog_in(&root, false).unwrap();
    assert_eq!(
        catalog.sessions.len(),
        1,
        "不能按用户消息所在文件重复生成卡片"
    );
    assert_eq!(catalog.sessions[0].real_user_count, 2);
    assert_eq!(catalog.sessions[0].assistant_text_count, 2);
    let detail = read_session_in(&root, "thread").unwrap();
    assert_eq!(detail.events.len(), 4);
    assert!(detail
        .events
        .iter()
        .all(|e| e["message"]["content"] != "discard this tail"));
}

#[test]
fn missing_cyclic_or_truncated_history_is_reported_and_not_searched_as_complete() {
    for base in [
        json!({"thread_id":"missing","end_byte_offset":100}),
        json!({"thread_id":"next","end_byte_offset":100}),
        json!({"thread_id":"thread","end_byte_offset":u64::MAX}),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("sessions");
        page(
            &root,
            "thread",
            "thread",
            0,
            None,
            vec![message(1, "user", "first")],
        );
        page(
            &root,
            "thread_next",
            "thread",
            1,
            Some(base),
            vec![message(2, "user", "last")],
        );
        let catalog = catalog_in(&root, false).unwrap();
        assert!(catalog.warnings.is_empty());
        assert_eq!(catalog.sessions.len(), 1);
        assert!(!read_session_in(&root, "thread")
            .unwrap()
            .warnings
            .is_empty());
        assert!(CodexSource.search_events(&catalog.sessions[0]).is_err());
    }
}

#[test]
fn replayed_legacy_prefix_is_deduplicated_without_losing_repeated_messages_in_one_file() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("sessions");
    let repeated = message(1, "user", "repeated within one source");
    for (storage, minute, events) in [
        ("thread", 0, vec![repeated.clone(), repeated.clone()]),
        (
            "thread_copy",
            1,
            vec![
                repeated.clone(),
                repeated,
                message(2, "assistant", "new suffix"),
            ],
        ),
    ] {
        let p = page(&root, storage, "thread", minute, None, events);
        let text = fs::read_to_string(&p).unwrap();
        let mut rows: Vec<Value> = text
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        rows[0]["payload"]
            .as_object_mut()
            .unwrap()
            .remove("history_mode");
        for row in &mut rows[1..] {
            row["payload"].as_object_mut().unwrap().remove("id");
        }
        fs::write(
            &p,
            rows.into_iter()
                .map(|v| v.to_string() + "\n")
                .collect::<String>(),
        )
        .unwrap();
    }
    let catalog = catalog_in(&root, false).unwrap();
    assert_eq!(catalog.sessions.len(), 1);
    assert_eq!(catalog.sessions[0].real_user_count, 2);
    assert_eq!(read_session_in(&root, "thread").unwrap().events.len(), 3);
}

#[test]
fn real_paginated_history_is_unique_and_complete_if_present() {
    let requested = std::env::var("ASM_TEST_CODEX_SESSION").ok();
    let root = sessions_root();
    if !root.exists() {
        assert!(
            requested.is_none(),
            "Requested Codex session root does not exist"
        );
        return;
    }
    let catalog = catalog_in(&root, false).unwrap();
    let ids: std::collections::HashSet<_> =
        catalog.sessions.iter().map(|m| &m.session_id).collect();
    assert_eq!(ids.len(), catalog.sessions.len());
    let Some(meta) = catalog.sessions.iter().find(|m| {
        requested
            .as_ref()
            .map_or(m.source_files.len() > 2, |id| &m.session_id == id)
    }) else {
        assert!(
            requested.is_none(),
            "Requested Codex session was not found in the catalog"
        );
        return;
    };
    let detail = read_session_in(&root, &meta.session_id).unwrap();
    if requested.is_some() {
        assert!(
            detail.warnings.is_empty(),
            "History is incomplete: {:?}",
            detail.warnings
        );
    }
    assert_eq!(detail.meta.real_user_count, meta.real_user_count);
    assert_eq!(detail.meta.assistant_text_count, meta.assistant_text_count);
    assert_eq!(detail.meta.source_files, meta.source_files);
    eprintln!(
        "Codex merged history: {} files, {} user messages, {} assistant messages, {} events, {} history warnings",
        meta.source_files.len(),
        meta.real_user_count,
        meta.assistant_text_count,
        detail.events.len(),
        detail.warnings.len()
    );
    // 显式指定私有临时输出路径时，供本地 WebKit 回归读取；默认不导出真实对话。
    if let Ok(path) = std::env::var("ASM_TEST_CODEX_EXPORT") {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .unwrap();
        file.write_all(&serde_json::to_vec(&detail).unwrap())
            .unwrap();
    }
}

#[test]
fn first_screen_never_scans_body_and_refinement_invalidates_after_append() {
    use std::io::Write;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("sessions");
    let path = page(
        &root,
        "fast-thread",
        "fast-thread",
        0,
        None,
        vec![message(1, "user", "hello")],
    );
    let scan = catalog::Scan::new(&root, CatalogFilter::default(), &|| false).unwrap();
    let quick = || scan.read(true, &|| false).unwrap();
    let preview = quick();
    assert!(preview.sessions[0].stats_pending);
    assert!(preview.warnings.is_empty());
    let accurate = scan.read(false, &|| false).unwrap();
    assert!(!accurate.sessions[0].stats_pending);
    assert_eq!(quick().sessions[0].real_user_count, 1);
    writeln!(
        fs::OpenOptions::new().append(true).open(&path).unwrap(),
        "{}",
        message(2, "user", "new")
    )
    .unwrap();
    assert!(quick().sessions[0].stats_pending);
    assert_eq!(
        scan.read(false, &|| false).unwrap().sessions[0].real_user_count,
        2
    );
    // 损坏尾部只有后台精确扫描才会接触，不能挡住有有效身份的首屏。
    writeln!(
        fs::OpenOptions::new().append(true).open(&path).unwrap(),
        "broken JSON"
    )
    .unwrap();
    assert!(quick().warnings.is_empty());
    assert!(!scan.read(false, &|| false).unwrap().warnings.is_empty());
    assert!(catalog::Scan::new(&root, CatalogFilter::default(), &|| true).is_err());
}

#[test]
fn historical_model_reaches_refined_list_detail_and_cached_first_screen() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("sessions");
    page(
        &root,
        "model-thread",
        "model-thread",
        0,
        None,
        vec![
            json!({"type":"turn_context","ordinal":1,"payload":{"model":"historical-model-a"}}),
            message(2, "user", "hello"),
            message(3, "assistant", "first answer"),
            json!({"type":"turn_context","ordinal":4,"payload":{"model":"historical-model-b"}}),
            message(5, "user", "continue"),
            message(6, "assistant", "second answer"),
        ],
    );
    assert_eq!(
        catalog_in(&root, false).unwrap().sessions[0]
            .model
            .as_deref(),
        Some("historical-model-b")
    );
    assert_eq!(
        read_session_in(&root, "model-thread")
            .unwrap()
            .meta
            .model
            .as_deref(),
        Some("historical-model-b")
    );
    assert_eq!(
        read_session_mode(&root, "model-thread", ReadMode::View)
            .unwrap()
            .meta
            .model
            .as_deref(),
        Some("historical-model-b")
    );
    assert_eq!(
        catalog::Scan::new(&root, CatalogFilter::default(), &|| false).unwrap().read(true, &|| false)
            .unwrap()
            .sessions[0]
            .model
            .as_deref(),
        Some("historical-model-b")
    );
}

#[test]
fn old_empty_model_summaries_are_invalidated_without_scanning_the_first_screen_body() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("sessions");
    let path = page(
        &root,
        "cached-model",
        "cached-model",
        0,
        None,
        vec![
            json!({"type":"turn_context","payload":{"model":"logged-model"}}),
            message(1, "user", "hello"),
        ],
    );
    let pages = history::read_headers(std::slice::from_ref(&path), &mut Vec::new(), &|| false).unwrap();
    let mut old = to_meta(&pages[0].header, None);
    old.real_user_count = 1;
    old.source_files = vec![path.clone()];
    super::super::catalog_cache::summary("codex-history", &[path], || Ok((old, vec![]))).unwrap();
    let quick = catalog::Scan::new(&root, CatalogFilter::default(), &|| false).unwrap().read(true, &|| false).unwrap();
    assert!(
        quick.sessions[0].stats_pending,
        "an old empty-model cache cannot count as a complete new summary"
    );
    assert_eq!(
        catalog_in(&root, false).unwrap().sessions[0]
            .model
            .as_deref(),
        Some("logged-model")
    );
    let warm = catalog::Scan::new(&root, CatalogFilter::default(), &|| false).unwrap().read(true, &|| false).unwrap();
    assert!(!warm.sessions[0].stats_pending);
    assert_eq!(warm.sessions[0].model.as_deref(), Some("logged-model"));
}

#[test]
fn model_follows_effective_page_boundaries_and_new_calls_but_not_blank_metadata() {
    use std::io::Write;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("sessions");
    let parent = page(
        &root,
        "paged-model",
        "paged-model",
        0,
        None,
        vec![
            json!({"type":"turn_context","ordinal":1,"payload":{"model":"inherited-model"}}),
            message(2, "user", "first"),
            message(3, "assistant", "answer"),
            json!({"type":"turn_context","ordinal":4,"payload":{"model":"discarded-model"}}),
        ],
    );
    let child = page(
        &root,
        "paged-model_page2",
        "paged-model",
        1,
        Some(json!({"thread_id":"paged-model","end_ordinal_exclusive":4})),
        vec![message(5, "user", "continue")],
    );
    assert_eq!(
        catalog_in(&root, false).unwrap().sessions[0]
            .model
            .as_deref(),
        Some("inherited-model")
    );
    for payload in [
        json!({"model":"used-model"}),
        json!({"model":"  "}),
        json!({"model":null}),
        json!({"model":42}),
        json!({"model_provider":"provider-is-not-a-model"}),
    ] {
        writeln!(
            fs::OpenOptions::new().append(true).open(&child).unwrap(),
            "{}",
            json!({"type":"token_usage_record","payload":payload})
        )
        .unwrap();
    }
    for mode in [ReadMode::Summary, ReadMode::Events, ReadMode::View] {
        let pages = history::read_headers(&[parent.clone(), child.clone()], &mut Vec::new(), &|| false).unwrap();
        let (detail, _) = history::read(&pages, mode).unwrap();
        assert_eq!(detail.meta.model.as_deref(), Some("used-model"));
    }
    assert_eq!(
        catalog_in(&root, false).unwrap().sessions[0]
            .model
            .as_deref(),
        Some("used-model")
    );
}

#[test]
fn deferred_tool_view_keeps_order_ids_and_counts_and_checks_the_source() {
    use std::io::Write;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("sessions");
    let call = json!({"type":"response_item","ordinal":2,"timestamp":"2099-12-31T00:00:02Z",
        "payload":{"id":"call","type":"function_call","name":"shell","call_id":"a","arguments":"{\"cmd\":\"hello\"}"}});
    let output = json!({"type":"response_item","ordinal":3,"timestamp":"2099-12-31T00:00:03Z",
        "payload":{"id":"out","type":"function_call_output","call_id":"a","output":"{\"output\":\"FAILED\",\"metadata\":{\"status\":\"error\"},\"chunk_id\":\"test\",\"exit_code\":2}"}});
    let search = json!({"type":"response_item","ordinal":4,"timestamp":"2099-12-31T00:00:04Z",
        "payload":{"type":"function_call","name":"exec_command","call_id":"search","arguments":"{\"cmd\":\"rg -n needle src\"}"}});
    let no_matches = json!({"type":"response_item","ordinal":5,"timestamp":"2099-12-31T00:00:05Z",
        "payload":{"type":"function_call_output","call_id":"search","output":"Chunk ID: test\nWall time: 0.01 seconds\nProcess exited with code 1\nFinal output:\n"}});
    let path = page(
        &root,
        "view-thread",
        "view-thread",
        0,
        None,
        vec![message(1, "user", "hello"), call, output, search, no_matches],
    );
    let full = read_session_in(&root, "view-thread").unwrap();
    let view = read_session_mode(&root, "view-thread", ReadMode::View).unwrap();
    assert_eq!(full.events.len(), view.events.len());
    assert_eq!(full.meta.assistant_count, view.meta.assistant_count);
    let result = &view.events[2]["message"]["content"][0];
    assert_eq!(result["is_error"], true);
    assert_eq!(result["exit_code"], 2);
    assert!(result.get("command_output_kind").is_none());
    assert!(result.get("content").is_none());
    let search = &view.events[3]["message"]["content"][0];
    assert_eq!(search["ops"][0]["exit_one"], "no_match");
    let no_matches = &view.events[4]["message"]["content"][0];
    assert_eq!(no_matches["exit_code"], 1);
    assert_eq!(no_matches["command_output_kind"], "empty");
    assert!(no_matches.get("content").is_none());
    let mut tokens = vec![];
    for (original, light) in full.events.iter().zip(&view.events) {
        assert_eq!(original["uuid"], light["uuid"]);
        if let Some(token) = light
            .pointer("/message/content/0/deferred")
            .and_then(Value::as_str)
        {
            assert_eq!(
                crate::agents::read_tool_payload(token).unwrap(),
                original["message"]["content"][0]
            );
            tokens.push(token.to_string());
        } else {
            assert_eq!(original, light);
        }
    }
    assert_eq!(tokens.len(), 4);
    writeln!(
        fs::OpenOptions::new().append(true).open(&path).unwrap(),
        "{}",
        message(6, "assistant", "later")
    )
    .unwrap();
    assert!(
        crate::agents::read_tool_payload(&tokens[0]).is_ok(),
        "追加不使已打开的工具卡片过期"
    );
    let bytes = fs::read_to_string(&path)
        .unwrap()
        .replace("hello\\\"", "other\\\"");
    fs::write(&path, bytes).unwrap();
    assert!(
        crate::agents::read_tool_payload(&tokens[0]).is_err(),
        "同长度原位改写必须拒绝旧引用"
    );
}

#[test]
fn exec_children_on_a_later_page_keep_deferred_payloads_and_script_output() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("sessions");
    let origin = chrono::DateTime::parse_from_rfc3339("2099-12-31T00:00:00Z").unwrap().timestamp_millis();
    let script = |id: &str, turn: &str, at: u64, input: &str| json!({"type":"response_item", "ordinal":at, "timestamp":format!("2099-12-31T00:00:{at:02}Z"),
        "payload":{"type":"custom_tool_call","name":"exec","call_id":id,"input":input, "internal_chat_message_metadata_passthrough":{"turn_id":turn}}});
    let output = |id: &str, at: u64, body: &str| json!({"type":"response_item", "ordinal":at, "timestamp":format!("2099-12-31T00:00:{at:02}Z"),
        "payload":{"type":"custom_tool_call_output","call_id":id,"output":body}});
    let first = page(&root, "script-thread", "script-thread", 0, None,
        vec![message(1, "user", "Run a command"), script("parent", "one", 2, "await tools.exec_command({cmd:'printf child'})")]);
    let command = json!({"type":"event_msg", "ordinal":3, "timestamp":"2099-12-31T00:00:03Z", "payload":{
        "type":"item_completed","turn_id":"one","started_at_ms":origin+2500,"item":{"type":"CommandExecution", "id":"child", "command":["/bin/zsh","-lc","printf child"], "status":"completed", "exit_code":0,"aggregated_output":"child"}}});
    let unrelated = json!({"type":"event_msg", "ordinal":6, "timestamp":"2099-12-31T00:00:06Z", "payload":{
        "type":"item_completed","turn_id":"two","started_at_ms":origin+5500,"item":{"type":"ImageView", "id":"image", "path":"/tmp/fixture.png"}}});
    let failed = json!({"type":"event_msg", "ordinal":9, "timestamp":"2099-12-31T00:00:09Z", "payload":{
        "type":"item_completed","turn_id":"four","started_at_ms":origin+8500,"item":{"type":"ImageView", "id":"failed-child", "path":"/tmp/fixture.png"}}});
    let mixed = json!({"type":"event_msg", "ordinal":12, "timestamp":"2099-12-31T00:00:12Z", "payload":{
        "type":"item_completed","turn_id":"mixed","started_at_ms":origin+11500,"item":{"type":"CommandExecution", "id":"mixed-child", "command":"false", "status":"completed", "exit_code":2, "aggregated_output":"fixture failure"}}});
    let file_change = json!({"type":"event_msg", "ordinal":15,"timestamp":"2099-12-31T00:00:15Z", "payload":{
        "type":"item_completed","turn_id":"patch","started_at_ms":origin+14500,
        "item":{"type":"FileChange","id":"patch-child","status":"completed","changes":{"src/new.ts":{"type":"add","content":"export {};"}}}}});
    let comparison = json!({"type":"event_msg", "ordinal":21, "timestamp":"2099-12-31T00:00:21Z", "payload":{
        "type":"item_completed","turn_id":"compare",
        "item":{"type":"CommandExecution","id":"compare","command":"diff -u old new","status":"failed","exit_code":1,
            "aggregated_output":"--- old\n+++ new\n@@ -1 +1 @@\n-before\n+after\n"}}});
    let multi_web = json!({"type":"event_msg", "ordinal":23, "timestamp":"2099-12-31T00:00:23Z", "payload":{
        "type":"item_completed","turn_id":"web","started_at_ms":origin+22500,
        "item":{"type":"Extension","kind":"web.search","id":"web-child","status":"completed","action":{"type":"search","queries":["alpha"]}}}});
    let mismatched_web = json!({"type":"event_msg", "ordinal":26, "timestamp":"2099-12-31T00:00:26Z", "payload":{
        "type":"item_completed","turn_id":"web-mismatch","started_at_ms":origin+25500,
        "item":{"type":"Extension","kind":"web.search","id":"web-beta","status":"completed","action":{"type":"search","queries":["beta"]}}}});
    let mismatched_patch = json!({"type":"event_msg", "ordinal":29, "timestamp":"2099-12-31T00:00:29Z", "payload":{
        "type":"item_completed","turn_id":"patch-mismatch","started_at_ms":origin+28500,
        "item":{"type":"FileChange","id":"patch-section-b","status":"completed","changes":{"src/same.ts":{"type":"update","unified_diff":"@@ -10,3 +10,3 @@\n fn second() {\n-  x = 1;\n+  x = 2;\n }\n"}}}}});
    let standalone_patch = format!("text(await tools.apply_patch({}));", json!("*** Begin Patch\n*** Add File: src/new.ts\n+export {};\n*** End Patch"));
    let section_a_patch = format!("text(await tools.apply_patch({}));", json!("*** Begin Patch\n*** Update File: src/same.ts\n@@\n fn first() {\n-  x = 1;\n+  x = 2;\n }\n*** End Patch"));
    let batch = r#"const patch = String.raw`*** Begin Patch
*** Update File: src/fixture.ts
@@
-const previous = 1;
+const next = 2;
*** End Patch`;
const apply = tools.apply_patch;
text(await apply(patch));
const cmd = "cat " + "skills/fixture/SKILL.md";
const r = await Promise.allSettled([tools['exec_command']({max_output_tokens: 60, cmd}), tools.exec_command({cmd:'git status --short --branch'})]);
for (const result of r) text(result);
text('tools.exec_command({cmd:"do not parse"})');"#;
    page(&root, "script-thread_followup", "script-thread", 1, Some(json!({"thread_id":"script-thread",
        "end_byte_offset":first.metadata().unwrap().len(), "end_ordinal_exclusive":3})), vec![
        command, output("parent", 4, "Script completed\nOutput:\nextra log"),
        script("unrelated", "two", 5, "await tools.view_image({path:'/tmp/different.png'})"), unrelated, output("unrelated", 7, "Script completed\nOutput:\nstandalone log"),
        script("failed", "four", 8, "await tools.view_image({path:'/tmp/fixture.png'})"), failed, output("failed", 10, "Script failed: fixture"),
        script("mixed", "mixed", 11, "await tools.exec_command({cmd:'false'}); await tools.update_plan({})"), mixed, output("mixed", 13, "Script completed"),
        script("patch", "patch", 14, &standalone_patch), file_change, output("patch", 16, "Script completed\nOutput:\nDone"),
        script("batch", "batch", 17, batch), output("batch", 18, "Script completed\nOutput:\nCombined stdout"),
        script("running", "async", 19, "text(await tools.write_stdin({session_id:42,chars:''}));"),
        output("running", 20, "Script running with cell ID g25"), comparison,
        script("multi-web", "web", 22, r#"text(await tools.web__run({search_query:[{q:"alpha"}],time:[{utc_offset:"+00:00"}]}))"#),
        multi_web, output("multi-web", 24, "Script completed\nOutput:\nok"),
        script("web-alpha", "web-mismatch", 25, r#"text(await tools.web__run({search_query:[{q:"alpha"}]}))"#),
        mismatched_web, output("web-alpha", 27, "Script completed\nOutput:\nok"),
        script("patch-section-a", "patch-mismatch", 28, &section_a_patch),
        mismatched_patch, output("patch-section-a", 30, "Script completed\nOutput:\nok"),
    ]);
    let view = read_session_mode(&root, "script-thread", ReadMode::View).unwrap();
    let blocks: Vec<_> = view.events.iter().filter_map(|event| event.pointer("/message/content/0")).collect();
    let load = |block: &Value| crate::agents::read_tool_payload(block["deferred"].as_str().unwrap()).unwrap();
    assert!(!blocks.iter().any(|block| block["id"] == "parent" || block["tool_use_id"] == "parent"));
    let child = blocks.iter().copied().find(|block| block["id"] == "child").unwrap();
    assert_eq!(load(child)["input"]["cmd"], "printf child");
    assert_eq!(load(&child["script_result"])["content"], "Script completed\nOutput:\nextra log");
    let stdout = blocks.iter().copied().find(|block| block["tool_use_id"] == "child").unwrap();
    assert_eq!(load(stdout)["content"], "child");
    assert!(blocks.iter().any(|block| block["id"] == "unrelated"));
    assert!(blocks.iter().any(|block| block["tool_use_id"] == "failed" && block["is_error"] == true));
    assert!(blocks.iter().any(|block| block["id"] == "mixed" && block["name"] == "exec"));
    let mixed_result = blocks.iter().find(|block| block["tool_use_id"] == "mixed-child").unwrap();
    assert_eq!(mixed_result["error_summary"], "fixture failure");
    let compare = blocks.iter().find(|block| block["id"] == "compare").unwrap();
    assert_eq!(compare["ops"][0]["outcome"], "differences");
    assert_eq!(compare["status"], "error");
    assert!(!blocks.iter().any(|block| block["id"] == "patch"));
    let patch = blocks.iter().find(|block| block["id"] == "patch-child").unwrap();
    assert_eq!(patch["ops"][0]["kind"], "write");
    assert_eq!(load(&patch["script_result"])["content"], "Script completed\nOutput:\nDone");
    let wrapper = blocks.iter().find(|block| block["id"] == "batch").unwrap();
    let calls = wrapper["ops"][0]["calls"].as_array().unwrap();
    assert_eq!(calls.len(), 3);
    assert_eq!(calls[0]["ops"][0]["path"], "src/fixture.ts");
    assert!(calls[0]["ops"][0].get("hunks").is_none());
    assert_eq!(calls[1]["ops"][0]["steps"][0]["path"], "skills/fixture/SKILL.md");
    assert_eq!(calls[2]["ops"][0]["head"], "git status --short --branch");
    assert_eq!(wrapper["ops"][0]["lead"], json!([0, 0]));
    assert_eq!(wrapper["ops"][0]["features"], json!(["parallel"]));
    assert_eq!(load(wrapper)["ops"][0]["calls"][0]["ops"][0]["hunks"][0]["lines"][1], "+const next = 2;");
    let running = blocks.iter().copied().find(|block| block["tool_use_id"] == "running").unwrap();
    assert_eq!(running["status"], "running");
    assert_eq!(load(running)["status"], "running");
    assert!(blocks.iter().any(|block| block["id"] == "multi-web" && block["name"] == "exec"));
    assert!(blocks.iter().any(|block| block["id"] == "web-alpha" && block["name"] == "exec"));
    assert!(blocks.iter().any(|block| block["id"] == "web-beta" && block.get("script_result").is_none()));
    assert!(blocks.iter().any(|block| block["id"] == "patch-section-a" && block["name"] == "exec"));
    assert!(blocks.iter().any(|block| block["id"] == "patch-section-b" && block.get("script_result").is_none()));
}
