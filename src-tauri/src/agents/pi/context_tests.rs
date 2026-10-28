use super::*;
use serde_json::json;

#[test]
fn context_visibility_pi_tree_summaries_and_extensions() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    fs::create_dir(&project).unwrap();
    let file = project.join("fixture_pi-context.jsonl");
    let records = [
        json!({"type":"session","version":3,"id":"pi-context","cwd":"/tmp/context","timestamp":"2026-10-27T00:00:00Z"}),
        json!({"type":"message","id":"user","parentId":null,"message":{"role":"user","content":"genuine request"}}),
        json!({"type":"custom_message","id":"abandoned","parentId":"user","content":"Abandoned context","customType":"plugin","display":false}),
        json!({"type":"custom_message","id":"injected","parentId":"user","content":"context-secret extension","customType":"plugin","display":false}),
        json!({"type":"compaction","id":"compact","parentId":"injected","summary":"context-secret compacted history","firstKeptEntryId":"user","tokensBefore":4000}),
        json!({"type":"branch_summary","id":"branch","parentId":"compact","summary":"context-secret previous branch","fromId":"abandoned"}),
        json!({"type":"message","id":"skill","parentId":"branch","message":{"role":"user","content":"<skill name=\"demo\" location=\"/tmp/demo/SKILL.md\">\ncontext-secret skill instructions\n</skill>\n\nuser skill request"}}),
        json!({"type":"message","id":"answer","parentId":"skill","message":{"role":"assistant","content":[{"type":"text","text":"visible-answer"}]}}),
        json!({"type":"custom","id":"state","parentId":"answer","data":{"internal":"not LLM context"}}),
    ];
    fs::write(
        &file,
        records.iter().map(|v| format!("{v}\n")).collect::<String>(),
    )
    .unwrap();
    let before = fs::read(&file).unwrap();
    let (summary, warnings) = parse_pi_session(&file, false).split();
    assert!(warnings.is_empty());
    let summary = summary.unwrap();
    assert_eq!(summary.real_user_count, 2);
    assert_eq!(summary.assistant_text_count, 1);
    let detail = read_session_in(dir.path(), "project", "pi-context").unwrap();
    assert_eq!(
        detail
            .events
            .iter()
            .filter(|e| e["type"] == "context")
            .count(),
        4
    );
    assert!(!detail
        .events
        .iter()
        .any(|e| e["uuid"] == "abandoned" || e["uuid"] == "state"));
    assert_eq!(
        detail.events.iter().find(|e| e["uuid"] == "skill").unwrap()["message"]["content"],
        "/skill:demo\nuser skill request"
    );
    assert_eq!(fs::read(&file).unwrap(), before);
    super::super::context_test_support::check_and_export(
        "pi",
        detail,
        "context-secret",
        "visible-answer",
    );
}

#[test]
fn context_visibility_actual_pi_log_if_requested() {
    let Ok(file) = std::env::var("ASM_PI_CONTEXT_LIVE") else {
        return;
    };
    let path = Path::new(&file);
    let before = fs::read(path).unwrap();
    let (parsed, warnings) = parse_pi_session(path, true).split();
    assert!(warnings.is_empty());
    let parsed = parsed.unwrap();
    assert_eq!(parsed.real_user_count, 1);
    assert_eq!(parsed.assistant_text_count, 1);
    assert!(parsed
        .events
        .iter()
        .any(|e| e["context"]["kind"] == "extension"));
    assert!(parsed
        .events
        .iter()
        .any(|e| e["context"]["kind"] == "compaction"));
    assert_eq!(fs::read(path).unwrap(), before);
    let detail = SessionDetail {
        meta: to_meta(&parsed),
        events: parsed.events,
        warnings: vec![],
    };
    super::super::context_test_support::check_and_export(
        "pi-live",
        detail,
        "ASM_PI_CONTEXT",
        "ASM actual Pi answer",
    );
}
