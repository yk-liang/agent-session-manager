use super::*;
use std::io::Write;

fn message(ordinal: u64, role: &str, text: &str) -> Value {
    json!({"type":"response_item","ordinal":ordinal,"timestamp":"2026-10-27T00:00:01Z",
        "payload":{"type":"message","id":format!("msg-{ordinal}"),"role":role,"content":[{"type":"input_text","text":text}]}})
}

fn fixture() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("sessions");
    fs::create_dir_all(root.join("2026/10/27")).unwrap();
    for (id, records) in [
        ("context-main", vec![
            message(1, "developer", "Runtime instructions: runtime-only-needle"),
            message(2, "developer", "<app-context>Application context instructions</app-context>"),
            message(3, "system", "System instructions"),
            message(4, "user", "<external_codex_apps_open_page>{\"page_id\":null}</external_codex_apps_open_page>"),
            message(5, "developer", "<codex_apps_open_page_instructions>Interpret the current page state.</codex_apps_open_page_instructions>"),
            message(6, "user", "<environment_context>cwd: /tmp/context</environment_context>\nFirst actual request"),
            message(7, "assistant", "First actual answer"),
            message(8, "user", "<external_codex_apps_open_page>{\"page_id\":\"fixture-page\"}</external_codex_apps_open_page>"),
            message(9, "user", "Second actual request: explain <external_codex_apps_open_page>{\"page_id\":null}</external_codex_apps_open_page>"),
            message(10, "assistant", "Last answer with visible needle"),
            message(11, "user", "<external_codex_apps_open_page>{\"page_id\":null}</external_codex_apps_open_page>"),
        ]),
        ("context-only", vec![
            message(1, "developer", "Context without a user request"),
            message(2, "user", "<external_codex_apps_open_page>{\"page_id\":null}</external_codex_apps_open_page>"),
        ]),
    ] {
        let path = root.join(format!("2026/10/27/rollout-{id}.jsonl"));
        let mut file = fs::File::create(path).unwrap();
        writeln!(file, "{}", json!({"type":"session_meta","timestamp":"2026-10-27T00:00:00Z","payload":{
            "id":id,"cwd":"/tmp/context","source":"cli","cli_version":"fixture"}})).unwrap();
        for record in records { writeln!(file, "{record}").unwrap(); }
    }
    (dir, root)
}

#[test]
fn context_is_preserved_without_polluting_counts_titles_search_or_event_identity() {
    let (_dir, root) = fixture();
    let catalog = catalog_in(&root, false).unwrap();
    assert!(catalog.warnings.is_empty());
    assert_eq!(catalog.sessions.len(), 1);
    let meta = &catalog.sessions[0];
    assert_eq!(meta.title.as_deref(), Some("First actual request"));
    assert_eq!((meta.real_user_count, meta.assistant_text_count), (2, 2));
    let full = read_session_in(&root, &meta.session_id).unwrap();
    let view = read_session_mode(&root, &meta.session_id, ReadMode::View).unwrap();
    assert_eq!(full.events, view.events);
    assert_eq!(CodexSource.search_events(meta).unwrap(), view.events);
    let contexts: Vec<_> = view
        .events
        .iter()
        .filter(|e| e["type"] == "context")
        .collect();
    assert_eq!(contexts.len(), 8);
    assert!(contexts
        .iter()
        .any(|e| e["context"]["summary"] == "未打开页面"));
    assert!(contexts
        .iter()
        .any(|e| e["context"]["summary"] == "已关联页面"));
    assert!(contexts
        .iter()
        .any(|e| e["message"]["content"] == "Runtime instructions: runtime-only-needle"));
    let ids: std::collections::HashSet<_> = view
        .events
        .iter()
        .map(|e| e["uuid"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids.len(),
        view.events.len(),
        "split context and request must have distinct identities"
    );

    let source = CodexSource;
    super::super::search::register(&source, &catalog);
    let target = super::super::search::SessionRef::from(meta);
    let search = |query| {
        super::super::search::search(
            &source,
            &[target.clone()],
            query,
            super::super::search::start(),
        )
        .unwrap()
    };
    assert!(search("runtime-only-needle").hits.is_empty());
    let hit = search("visible needle");
    assert_eq!(hit.hits.len(), 1);
    assert_eq!(
        view.events[hit.hits[0].event_index]["uuid"].as_str(),
        hit.hits[0].event_id.as_deref()
    );
    assert_eq!(
        search("page_id").hits.len(),
        1,
        "a real user discussing the envelope remains searchable"
    );
    let all = catalog_in(&root, true).unwrap();
    assert_eq!(all.sessions.len(), 2);
    assert_eq!(
        all.sessions
            .iter()
            .find(|s| s.session_id == "context-only")
            .unwrap()
            .real_user_count,
        0
    );

    if let Ok(destination) = std::env::var("ASM_CODEX_CONTEXT_EXPORT") {
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(destination)
            .unwrap();
        let only = read_session_mode(&root, "context-only", ReadMode::View).unwrap();
        file.write_all(
            &serde_json::to_vec(
                &json!({"catalog":catalog,"all":all,"details":[view,only],"search":hit}),
            )
            .unwrap(),
        )
        .unwrap();
    }
}

#[test]
fn actual_context_log_is_read_only_if_requested() {
    let Ok(path) = std::env::var("ASM_CODEX_CONTEXT_REAL") else {
        return;
    };
    let path = Path::new(&path);
    let before = fs::read(path).unwrap();
    let (parsed, warnings) = read_rollout(path, ReadMode::View, ReadLimit::default(), None).split();
    let parsed = parsed.unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    assert!(parsed
        .events
        .iter()
        .any(|e| e["context"]["label"] == "开发者指令"));
    assert!(parsed.events.iter().any(|e| e["context"]["kind"] == "page"));
    assert!(parsed.events.iter().all(|e| e["type"] != "developer"));
    assert!(parsed
        .events
        .iter()
        .filter(|e| e["type"] == "user")
        .all(|e| {
            !e["message"]["content"]
                .as_str()
                .is_some_and(|s| s.starts_with("<external_codex_apps_open_page>"))
        }));
    assert_eq!(fs::read(path).unwrap(), before);
    println!("Actual Codex log: {} user messages, {} assistant text messages, {} context records; source unchanged",
        parsed.real_user_count, parsed.assistant_text_count, parsed.events.iter().filter(|e| e["type"] == "context").count());
}
