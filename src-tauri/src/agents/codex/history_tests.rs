use super::*;

fn page(
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

fn message(ordinal: u64, role: &str, text: &str) -> Value {
    json!({"timestamp":format!("2099-12-31T00:00:{ordinal:02}Z"), "ordinal":ordinal,
        "type":"response_item", "payload":{"type":"message", "role":role,
            "id":format!("message-{ordinal}"), "content":[{"type":"text","text":text}]}})
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
        first.metadata().unwrap().len() + second.metadata().unwrap().len()
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
fn confirmed_paginated_deletion_receives_every_file_once_and_keeps_other_threads() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("sessions");
    let first = page(
        &root,
        "thread",
        "thread",
        0,
        None,
        vec![message(1, "user", "first")],
    );
    let second = page(
        &root,
        "thread_next",
        "thread",
        1,
        Some(json!({"thread_id":"thread","end_byte_offset":first.metadata().unwrap().len()})),
        vec![message(2, "user", "last")],
    );
    let other = page(
        &root,
        "other",
        "other",
        0,
        None,
        vec![message(1, "user", "other")],
    );
    let calls = std::cell::Cell::new(0);
    delete_session_with(&root, &escape_cwd("/tmp/paged"), "thread", |paths| {
        calls.set(calls.get() + 1);
        assert_eq!(paths.len(), 2);
        assert!(paths.contains(&first.canonicalize().unwrap()));
        assert!(paths.contains(&second.canonicalize().unwrap()));
        Ok(()) // 注入移除器只记录目标，不操作回收站或真实数据。
    })
    .unwrap();
    assert_eq!(calls.get(), 1);
    assert!(other.exists() && first.exists() && second.exists());
}

#[test]
fn paginated_deletion_refuses_any_corrupt_member_before_calling_the_remover() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("sessions");
    let first = page(
        &root,
        "thread",
        "thread",
        0,
        None,
        vec![message(1, "user", "first")],
    );
    let second = page(
        &root,
        "thread_next",
        "thread",
        1,
        Some(json!({"thread_id":"thread","end_byte_offset":first.metadata().unwrap().len()})),
        vec![message(2, "user", "last")],
    );
    use std::io::Write;
    writeln!(
        fs::OpenOptions::new().append(true).open(&second).unwrap(),
        "invalid JSON"
    )
    .unwrap();
    assert!(
        delete_session_with(&root, &escape_cwd("/tmp/paged"), "thread", |_| panic!(
            "不得开始部分删除"
        ))
        .is_err()
    );
    assert!(first.exists() && second.exists());
}

#[test]
fn paginated_deletion_refuses_unreadable_or_missing_predecessor() {
    for unreadable in [true, false] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("sessions");
        let first = page(&root, "thread", "thread", 0, None, vec![]);
        let second = page(
            &root,
            "thread_next",
            "thread",
            1,
            Some(json!({"thread_id":"thread","end_byte_offset":first.metadata().unwrap().len()})),
            vec![message(2, "user", "last")],
        );
        if unreadable {
            fs::write(&first, b"\xff\xfe\n").unwrap();
        } else {
            fs::remove_file(&first).unwrap();
        }
        let calls = std::cell::Cell::new(0);
        let result = delete_session_with(&root, &escape_cwd("/tmp/paged"), "thread", |_| {
            calls.set(calls.get() + 1);
            Ok(())
        });
        assert!(result.is_err(), "不可把唯一可见的最新页当作完整会话删除");
        assert_eq!(calls.get(), 0);
        assert!(second.exists());
    }
}

#[test]
fn paginated_deletion_refuses_malformed_history_identity() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("sessions");
    let file = page(
        &root,
        "thread",
        "thread",
        0,
        Some(json!({"thread_id":42})),
        vec![],
    );
    assert!(
        delete_session_with(&root, &escape_cwd("/tmp/paged"), "thread", |_| {
            panic!("不可把损坏的历史引用当作无历史引用")
        })
        .is_err()
    );
    assert!(file.exists());
}

#[test]
fn real_paginated_history_is_unique_and_complete_if_present() {
    let root = sessions_root();
    if !root.exists() {
        return;
    }
    let catalog = catalog_in(&root, false).unwrap();
    let ids: std::collections::HashSet<_> =
        catalog.sessions.iter().map(|m| &m.session_id).collect();
    assert_eq!(ids.len(), catalog.sessions.len());
    let requested = std::env::var("ASM_TEST_CODEX_SESSION").ok();
    let Some(meta) = catalog.sessions.iter().find(|m| {
        requested
            .as_ref()
            .map_or(m.source_files.len() > 2, |id| &m.session_id == id)
    }) else {
        return;
    };
    let detail = read_session_in(&root, &meta.session_id).unwrap();
    assert_eq!(detail.meta.real_user_count, meta.real_user_count);
    assert_eq!(detail.meta.assistant_text_count, meta.assistant_text_count);
    assert_eq!(detail.meta.source_files, meta.source_files);
    eprintln!(
        "Codex merged history: {} files, {} user messages, {} assistant messages, {} events",
        meta.source_files.len(),
        meta.real_user_count,
        meta.assistant_text_count,
        detail.events.len()
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
    let quick = || catalog_with(&root, false, true, &|| false).unwrap();
    let preview = quick();
    assert!(preview.sessions[0].stats_pending);
    assert!(preview.warnings.is_empty());
    let accurate = catalog_in(&root, false).unwrap();
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
        catalog_in(&root, false).unwrap().sessions[0].real_user_count,
        2
    );
    // 损坏尾部只有后台精确扫描才会接触，不能挡住有有效身份的首屏。
    writeln!(
        fs::OpenOptions::new().append(true).open(&path).unwrap(),
        "broken JSON"
    )
    .unwrap();
    assert!(quick().warnings.is_empty());
    assert!(!catalog_in(&root, false).unwrap().warnings.is_empty());
    assert!(catalog_with(&root, false, false, &|| true).is_err());
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
        catalog_with(&root, false, true, &|| false)
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
    let pages = history::headers(vec![path.clone()], &mut Vec::new());
    let mut old = to_meta(&pages[0].header, None);
    old.real_user_count = 1;
    old.source_files = vec![path.clone()];
    super::super::catalog_cache::summary("codex-history", &[path], || Ok((old, vec![]))).unwrap();
    let quick = catalog_with(&root, false, true, &|| false).unwrap();
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
    let warm = catalog_with(&root, false, true, &|| false).unwrap();
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
        let pages = history::headers(vec![parent.clone(), child.clone()], &mut Vec::new());
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
        "payload":{"id":"out","type":"function_call_output","call_id":"a","output":"{\"output\":\"FAILED\",\"metadata\":{\"status\":\"error\"}}"}});
    let path = page(
        &root,
        "view-thread",
        "view-thread",
        0,
        None,
        vec![message(1, "user", "hello"), call, output],
    );
    let full = read_session_in(&root, "view-thread").unwrap();
    let view = read_session_mode(&root, "view-thread", ReadMode::View).unwrap();
    assert_eq!(full.events.len(), view.events.len());
    assert_eq!(full.meta.assistant_count, view.meta.assistant_count);
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
    assert_eq!(tokens.len(), 2);
    writeln!(
        fs::OpenOptions::new().append(true).open(&path).unwrap(),
        "{}",
        message(4, "assistant", "later")
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
