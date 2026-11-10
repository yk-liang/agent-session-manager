use super::{
    history_tests::{message, page},
    *,
};

fn compress(path: &Path) -> PathBuf {
    let compressed = path.with_extension("jsonl.zst");
    fs::write(
        &compressed,
        zstd::stream::encode_all(fs::read(path).unwrap().as_slice(), 0).unwrap(),
    )
    .unwrap();
    fs::remove_file(path).unwrap();
    compressed
}

fn set_fork_parent(path: &Path, parent: &str) {
    let text = fs::read_to_string(path).unwrap();
    let mut rows: Vec<Value> = text
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    rows[0]["payload"]["forked_from_id"] = json!(parent);
    fs::write(
        path,
        rows.into_iter()
            .map(|row| row.to_string() + "\n")
            .collect::<String>(),
    )
    .unwrap();
}

#[test]
fn fork_root_can_reference_a_parents_compressed_physical_page() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("sessions");
    let parent = page(
        &root,
        "parent",
        "parent",
        0,
        None,
        vec![message(1, "user", "parent request")],
    );
    let parent_page = page(
        &root,
        "parent_page2",
        "parent",
        1,
        Some(json!({"thread_id":"parent","end_byte_offset":parent.metadata().unwrap().len()})),
        vec![message(2, "assistant", "parent answer")],
    );
    let child = page(
        &root,
        "child",
        "child",
        2,
        Some(
            json!({"thread_id":"page2","end_byte_offset":parent_page.metadata().unwrap().len(),"end_ordinal_exclusive":3}),
        ),
        vec![message(3, "user", "fork request")],
    );
    set_fork_parent(&child, "parent");
    compress(&parent_page);
    let child = compress(&child);
    let detail = read_session_in(&root, "child").unwrap();
    assert!(detail.warnings.is_empty(), "{:?}", detail.warnings);
    assert_eq!(detail.events.len(), 1);
    assert_eq!(detail.events[0]["message"]["content"], "fork request");
    assert_eq!(
        CodexSource.search_events(&detail.meta).unwrap(),
        detail.events
    );
    delete_session_with(&root, &escape_cwd("/tmp/paged"), "child", |paths| {
        assert_eq!(paths, vec![child.canonicalize().unwrap()]);
        Ok(())
    })
    .unwrap();
}

#[test]
fn fork_later_page_still_reports_a_missing_page_of_its_own_thread() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("sessions");
    let child = page(
        &root,
        "child_later",
        "child",
        2,
        Some(json!({"thread_id":"missing-child-page","end_ordinal_exclusive":5})),
        vec![message(5, "user", "later request")],
    );
    set_fork_parent(&child, "parent");
    compress(&child);
    let detail = read_session_in(&root, "child").unwrap();
    assert!(detail
        .warnings
        .iter()
        .any(|warning| warning.contains("缺少历史分页")));
    assert!(CodexSource.search_events(&detail.meta).is_err());
    assert!(
        delete_session_with(&root, &escape_cwd("/tmp/paged"), "child", |_| {
            panic!("A fork marker cannot authorize partial deletion of its own pages")
        })
        .is_err()
    );
}

#[test]
fn mixed_pages_use_decoded_boundaries_and_keep_search_and_deletion_consistent() {
    check_mixed_pages(true);
}

#[test]
fn compressed_first_page_is_restored_before_two_plain_continuations() {
    // 首段 ID 与逻辑会话 ID 相同；漏扫压缩首段会误报“会话缺少自身 ID 的分页”。
    check_mixed_pages(false);
}

fn check_mixed_pages(compress_middle: bool) {
    use std::io::Write;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("sessions");
    let first = page(
        &root,
        "thread",
        "thread",
        0,
        None,
        vec![message(1, "user", &"first request ".repeat(2000))],
    );
    let boundary = first.metadata().unwrap().len();
    writeln!(
        fs::OpenOptions::new().append(true).open(&first).unwrap(),
        "{}",
        message(2, "user", "discarded tail")
    )
    .unwrap();
    let second = page(
        &root,
        "thread_page2",
        "thread",
        1,
        Some(json!({"thread_id":"thread","end_byte_offset":boundary,"end_ordinal_exclusive":2})),
        vec![message(3, "assistant", "second page answer")],
    );
    let second_boundary = second.metadata().unwrap().len();
    let third = page(
        &root,
        "thread_page3",
        "thread",
        2,
        Some(json!({"thread_id":"page2","end_byte_offset":second_boundary})),
        vec![message(4, "user", "last request")],
    );
    let first = compress(&first);
    let second = if compress_middle {
        compress(&second)
    } else {
        second
    };
    assert!(first.metadata().unwrap().len() < boundary);

    let quick = catalog_with(&root, false, true, &|| false).unwrap();
    assert_eq!(quick.sessions.len(), 1);
    let catalog = catalog_in(&root, false).unwrap();
    assert!(catalog.warnings.is_empty(), "{:?}", catalog.warnings);
    let meta = &catalog.sessions[0];
    assert_eq!((meta.real_user_count, meta.assistant_text_count), (2, 1));
    assert_eq!(meta.source_files.len(), 3);
    assert_eq!(
        meta.size_bytes,
        [&first, &second, &third]
            .iter()
            .map(|p| p.metadata().unwrap().len())
            .sum::<u64>()
    );
    let detail = read_session_in(&root, "thread").unwrap();
    assert!(detail.warnings.is_empty(), "{:?}", detail.warnings);
    assert_eq!(detail.events.len(), 3);
    assert!(!serde_json::to_string(&detail.events)
        .unwrap()
        .contains("discarded tail"));
    assert_eq!(CodexSource.search_events(meta).unwrap(), detail.events);
    let warm = catalog_with(&root, false, true, &|| false).unwrap();
    assert!(!warm.sessions[0].stats_pending);
    assert_eq!(warm.sessions[0].real_user_count, 2);
    delete_session_with(&root, &escape_cwd("/tmp/paged"), "thread", |paths| {
        assert_eq!(paths.len(), 3);
        for path in [&first, &second, &third] {
            assert!(paths.contains(&path.canonicalize().unwrap()));
        }
        Ok(())
    })
    .unwrap();
}

#[test]
fn compression_transition_does_not_duplicate_messages_or_delete_only_one_copy() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("sessions");
    let plain = page(
        &root,
        "thread",
        "thread",
        0,
        None,
        vec![message(1, "user", "request")],
    );
    let compressed = plain.with_extension("jsonl.zst");
    fs::write(
        &compressed,
        zstd::stream::encode_all(fs::read(&plain).unwrap().as_slice(), 0).unwrap(),
    )
    .unwrap();
    let catalog = catalog_in(&root, false).unwrap();
    assert!(catalog.warnings.is_empty());
    assert_eq!(catalog.sessions[0].source_files, vec![plain.clone()]);
    assert_eq!(read_session_in(&root, "thread").unwrap().events.len(), 1);
    assert!(
        delete_session_with(&root, &escape_cwd("/tmp/paged"), "thread", |_| {
            panic!("Both storage variants must be resolved before deletion")
        })
        .is_err()
    );
    fs::remove_file(&plain).unwrap();
    let after = catalog_in(&root, false).unwrap();
    assert_eq!(after.sessions.len(), 1);
    assert_eq!(after.sessions[0].source_files, vec![compressed.clone()]);
    delete_session_with(&root, &escape_cwd("/tmp/paged"), "thread", |paths| {
        assert_eq!(paths, vec![compressed.canonicalize().unwrap()]);
        Ok(())
    })
    .unwrap();
}

#[test]
fn compressed_tools_use_decoded_offsets_validate_rewrites_and_allow_appended_frames() {
    use std::io::Write;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("sessions");
    let call = json!({"type":"response_item","ordinal":2,"payload":{
        "type":"function_call","id":"call","call_id":"c1","name":"shell","arguments":"{\"cmd\":\"initial-tool\"}"}});
    let output = json!({"type":"response_item","ordinal":3,"payload":{
        "type":"function_call_output","id":"result","call_id":"c1","output":"{\"output\":\"tool result\"}"}});
    let plain = page(
        &root,
        "thread",
        "thread",
        0,
        None,
        vec![message(1, "user", &"prefix ".repeat(20000)), call, output],
    );
    let original = fs::read_to_string(&plain).unwrap();
    let compressed = compress(&plain);
    let full = read_session_in(&root, "thread").unwrap();
    let view = read_session_mode(&root, "thread", ReadMode::View).unwrap();
    assert!(view.warnings.is_empty());
    assert_eq!(view.events.len(), full.events.len());
    let mut tokens = Vec::new();
    for (event, expected) in view.events.iter().zip(&full.events) {
        if let Some(token) = event
            .pointer("/message/content/0/deferred")
            .and_then(Value::as_str)
        {
            assert_eq!(
                crate::agents::read_tool_payload(token).unwrap(),
                expected["message"]["content"][0]
            );
            tokens.push(token.to_string());
        }
    }
    assert_eq!(tokens.len(), 2);
    let appended = message(4, "assistant", "later answer").to_string() + "\n";
    fs::OpenOptions::new()
        .append(true)
        .open(&compressed)
        .unwrap()
        .write_all(&zstd::stream::encode_all(appended.as_bytes(), 0).unwrap())
        .unwrap();
    assert_eq!(read_session_in(&root, "thread").unwrap().events.len(), 4);
    assert!(crate::agents::read_tool_payload(&tokens[0]).is_ok());
    let modified = original.replace("initial-tool", "changed-tool") + &appended;
    assert_eq!(modified.len(), original.len() + appended.len());
    fs::write(
        &compressed,
        zstd::stream::encode_all(modified.as_bytes(), 0).unwrap(),
    )
    .unwrap();
    assert!(crate::agents::read_tool_payload(&tokens[0]).is_err());
}

#[test]
fn compressed_truncated_history_is_reported_and_never_partially_deleted() {
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
    let invalid_boundary = first.metadata().unwrap().len() + 1;
    compress(&first);
    page(
        &root,
        "thread_page2",
        "thread",
        1,
        Some(json!({"thread_id":"thread","end_byte_offset":invalid_boundary})),
        vec![message(1, "user", "first"), message(2, "user", "last")],
    );
    let detail = read_session_in(&root, "thread").unwrap();
    assert!(
        detail.warnings.iter().any(|w| w.contains("截断")),
        "{:?}",
        detail.warnings
    );
    assert_eq!(
        detail.events.len(),
        2,
        "A failed page must not consume the replayed message"
    );
    let meta = catalog_in(&root, false).unwrap().sessions.remove(0);
    assert!(CodexSource.search_events(&meta).is_err());
    assert!(
        delete_session_with(&root, &escape_cwd("/tmp/paged"), "thread", |_| {
            panic!("Truncated history must not reach the remover")
        })
        .is_err()
    );
}

#[test]
fn invalid_or_truncated_zstd_is_not_silently_ignored() {
    for truncate in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("sessions");
        let path = page(
            &root,
            "broken",
            "broken",
            0,
            None,
            vec![message(1, "user", "request")],
        );
        let compressed = compress(&path);
        if truncate {
            let bytes = fs::read(&compressed).unwrap();
            fs::write(&compressed, &bytes[..bytes.len() - 1]).unwrap();
        } else {
            fs::write(&compressed, b"not a zstd stream").unwrap();
        }
        let catalog = catalog_in(&root, false).unwrap();
        assert!(
            !catalog.warnings.is_empty(),
            "Unreadable compressed logs must produce a warning"
        );
        assert!(
            delete_session_with(&root, &escape_cwd("/tmp/paged"), "broken", |_| {
                panic!("Unreadable compressed logs must not reach the remover")
            })
            .is_err()
        );
    }
}

#[test]
fn compressed_empty_sessions_still_obey_the_empty_filter() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("sessions");
    compress(&page(&root, "empty", "empty", 0, None, vec![]));
    assert!(catalog_in(&root, false).unwrap().sessions.is_empty());
    assert_eq!(catalog_in(&root, true).unwrap().sessions.len(), 1);
}

#[test]
fn compressed_symlink_is_readable_but_deletion_still_refuses_links() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("sessions");
    let compressed = compress(&page(
        &root,
        "thread",
        "thread",
        0,
        None,
        vec![message(1, "user", "request")],
    ));
    let actual = compressed.with_file_name("stored-transcript.bin");
    fs::rename(&compressed, &actual).unwrap();
    std::os::unix::fs::symlink(&actual, &compressed).unwrap();
    assert_eq!(catalog_in(&root, false).unwrap().sessions.len(), 1);
    assert_eq!(read_session_in(&root, "thread").unwrap().events.len(), 1);
    assert!(
        delete_session_with(&root, &escape_cwd("/tmp/paged"), "thread", |_| {
            panic!("Readable symlinks must still be rejected by deletion preflight")
        })
        .is_err()
    );
}

#[test]
fn compressed_fifo_is_rejected_without_blocking_the_catalog() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("sessions");
    let path = root.join("2099/12/31/rollout-fifo.jsonl.zst");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let cpath = std::ffi::CString::new(path.to_str().unwrap()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(cpath.as_ptr(), 0o600) }, 0);
    let (send, receive) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = send.send(catalog_in(&root, false));
    });
    let catalog = receive
        .recv_timeout(std::time::Duration::from_secs(1))
        .expect("FIFO scan blocked")
        .unwrap();
    assert!(catalog.sessions.is_empty());
    assert!(!catalog.warnings.is_empty());
}
