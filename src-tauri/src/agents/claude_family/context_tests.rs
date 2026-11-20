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
    for (name, use_runtime_model) in [("claude", false), ("qoder", true)] {
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
            json!({"type":"assistant","message":{"role":"assistant","content":[
                {"type":"server_tool_use","id":"server","name":"web_search","input":{"query":"example"}},
                {"type":"web_search_tool_result","tool_use_id":"server","content":{"type":"web_search_tool_result_error","error_code":"unavailable"}}
            ]}}),
            json!({"type":"attachment","attachment":{"type":"queued_command","origin":{"kind":"human"},"commandMode":"prompt",
                "prompt":[{"type":"text","text":"queued question"},{"type":"image","source":{"type":"base64","media_type":"image/png","data":"AAAA"}}]}}),
            json!({"type":"attachment","attachment":{"type":"queued_command","origin":{"kind":"peer"},"commandMode":"prompt","prompt":"context-secret peer"}}),
            json!({"type":"assistant","isApiErrorMessage":true,"message":{"model":"<synthetic>","content":[{"type":"text","text":"API error line"}]}}),
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
            use_runtime_model,
            filter_non_cli_entrypoint: false,
        };
        let catalog = catalog(dir.path(), opts, false, &|| false).unwrap();
        assert_eq!(catalog.sessions.len(), 1);
        assert_eq!(catalog.sessions[0].real_user_count, 4);
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
            9
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
        let server = detail.events.iter().find(|event| event["uuid"] == "event-12").unwrap();
        assert_eq!(server["message"]["content"][0]["type"], "tool_use");
        assert_eq!(server["message"]["content"][1]["type"], "tool_result");
        assert_eq!(server["message"]["content"][1]["is_error"], true);
        let queued = &detail.events.iter().find(|e| e["uuid"] == "event-13").unwrap();
        assert_eq!(queued["type"], "user");
        assert_eq!(queued["message"]["content"][1]["type"], "image");
        assert!(detail.events.iter().any(|e| e["type"] == "system" && e["content"] == "API error line"));
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
        use_runtime_model: false,
        filter_non_cli_entrypoint: false,
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
