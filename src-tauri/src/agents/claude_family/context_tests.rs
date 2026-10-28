use super::*;
use serde_json::json;

#[test]
fn literal_context_markup_is_never_removed_from_ordinary_prose() {
    for text in [
        "解释 <system-reminder>instructions</system-reminder>",
        "```xml\n<system-reminder>instructions</system-reminder>\n```",
        "<system-reminder>instructions</system-reminder> 这是什么意思？",
        "<system-reminder-example>not metadata</system-reminder-example>",
        "<system-reminder>unclosed",
        "You are a coding agent. 请解释这个提示词。",
    ] {
        let original = json!({"type":"user","message":{"role":"user","content":text}});
        assert_eq!(context::normalize(original.clone()), vec![original]);
        assert_eq!(strip_noise_tags(text), text);
    }
}

#[test]
fn context_visibility_claude_and_qoder() {
    for (name, title_events) in [("claude", false), ("qoder", true)] {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project");
        fs::create_dir(&project).unwrap();
        let records = vec![
            json!({"type":"system","content":"context-secret system instructions"}),
            json!({"type":"developer","message":{"role":"developer","content":"context-secret developer"}}),
            json!({"type":"user","isMeta":true,"message":{"role":"user","content":"context-secret skill"}}),
            json!({"type":"user","isCompactSummary":true,"message":{"role":"user","content":"context-secret summary"}}),
            json!({"type":"attachment","attachment":{"type":"prompt_snapshot","systemPrompt":["context-secret snapshot"]}}),
            json!({"type":"user","message":{"role":"user","content":"<command-message>demo</command-message><command-name>/demo</command-name><command-args>genuine command</command-args>"}}),
            json!({"type":"user","message":{"role":"user","content":[{"type":"text","text":"<system-reminder>context-secret reminder</system-reminder>"},{"type":"text","text":"genuine request"},{"type":"tool_result","tool_use_id":"missing","content":"real tool output"}]}}),
            json!({"type":"assistant","message":{"role":"assistant","content":"Quoted <system-reminder>literal instructions</system-reminder>"}}),
            json!({"type":"user","message":{"role":"user","content":"<loaded_context>literal context</loaded_context> 这段是什么意思？"}}),
            json!({"type":"assistant","message":{"role":"assistant","content":"visible-answer"}}),
            json!({"type":"user","isVisibleInTranscriptOnly":true,"message":{"role":"user","content":"context-secret local output"}}),
            json!({"type":"system","subtype":"api_error","content":"Keep this operational error visible"}),
        ];
        let records: Vec<_> = records
            .into_iter()
            .enumerate()
            .map(|(i, mut v)| {
                v["uuid"] = json!(format!("event-{i}"));
                v["cwd"] = json!("/tmp/context");
                v["timestamp"] = json!("2026-10-27T00:00:00Z");
                v
            })
            .collect();
        let file = project.join("aaaa.jsonl");
        fs::write(
            &file,
            records.iter().map(|v| format!("{v}\n")).collect::<String>(),
        )
        .unwrap();
        fs::write(project.join("bbbb.jsonl"), format!("{}\n", records[2])).unwrap();
        let before = fs::read(&file).unwrap();
        let opts = FamilyOptions {
            title_events,
            filter_non_cli_entrypoint: false,
            filter_qoderwork_cwd: false,
        };
        let catalog = catalog(dir.path(), opts, false).unwrap();
        assert_eq!(catalog.sessions.len(), 1);
        assert_eq!(catalog.sessions[0].real_user_count, 3);
        assert_eq!(catalog.sessions[0].assistant_text_count, 2);
        assert_eq!(
            catalog.sessions[0].title.as_deref(),
            Some("/demo genuine command")
        );
        let detail = read_session(dir.path(), "project", "aaaa", opts).unwrap();
        assert_eq!(
            detail
                .events
                .iter()
                .filter(|e| e["type"] == "context")
                .count(),
            8
        );
        assert!(detail
            .events
            .iter()
            .any(|e| e["type"] == "system" && e["subtype"] == "api_error"));
        assert!(detail.events.iter().any(|e| e["message"]["content"]
            .as_str()
            .is_some_and(|s| s.contains("literal instructions"))
            && e["type"] == "assistant"));
        assert!(detail.events.iter().any(|e| e["message"]["content"]
            .as_array()
            .is_some_and(|parts| parts.iter().any(|b| b["type"] == "tool_result"))));
        assert_eq!(fs::read(&file).unwrap(), before);
        super::super::context_test_support::check_and_export(
            name,
            detail,
            "context-secret",
            "visible-answer",
        );
    }
}

#[test]
fn context_visibility_actual_family_log_if_requested() {
    let Ok(file) = std::env::var("ASM_FAMILY_CONTEXT_LIVE") else {
        return;
    };
    let path = Path::new(&file);
    let project = path.parent().unwrap();
    let opts = FamilyOptions {
        title_events: false,
        filter_non_cli_entrypoint: false,
        filter_qoderwork_cwd: false,
    };
    let before = fs::read(path).unwrap();
    let detail = read_session(
        project.parent().unwrap(),
        project.file_name().unwrap().to_str().unwrap(),
        path.file_stem().unwrap().to_str().unwrap(),
        opts,
    )
    .unwrap();
    assert_eq!(detail.meta.real_user_count, 1);
    assert_eq!(detail.meta.assistant_text_count, 1);
    assert!(detail
        .events
        .iter()
        .any(|e| e["context"]["label"] == "系统提示词快照"));
    assert_eq!(fs::read(path).unwrap(), before);
    super::super::context_test_support::check_and_export(
        "claude-live",
        detail,
        "ASM_SYSTEM_CONTEXT",
        "ASM actual Claude answer",
    );
}
