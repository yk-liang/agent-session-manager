//! Classify recorded runtime context by role and reserved message envelopes.
//! Never infer instructions from prose: users can discuss or quote these formats.
use serde_json::{json, Value};

struct Context<'a> {
    kind: &'static str,
    label: &'static str,
    summary: Option<&'static str>,
    text: &'a str,
}

fn block<'a>(text: &'a str, tag: &str) -> Option<(&'a str, &'a str)> {
    let prefix = format!("<{tag}");
    let after = text.strip_prefix(&prefix)?;
    if !after.starts_with('>') && !after.starts_with(char::is_whitespace) {
        return None;
    }
    let body = text.find('>')? + 1;
    let closing = format!("</{tag}>");
    let end = body + text[body..].find(&closing)? + closing.len();
    Some((&text[..end], &text[end..]))
}

fn page_context(text: &str) -> Option<Context<'_>> {
    let tag = "external_codex_apps_open_page";
    let (raw, rest) = block(text, tag)?;
    // Only a complete, standalone state notification is metadata. Quoted markup,
    // malformed payloads and messages discussing the notification remain text.
    if !rest.trim().is_empty() {
        return None;
    }
    let value: Value = serde_json::from_str(
        raw.strip_prefix(&format!("<{tag}>"))?
            .strip_suffix(&format!("</{tag}>"))?,
    )
    .ok()?;
    if value.as_object()?.len() != 1 {
        return None;
    }
    let summary = match value.get("page_id")? {
        Value::Null => "未打开页面",
        Value::String(id) if !id.trim().is_empty() => "已关联页面",
        _ => return None,
    };
    Some(Context {
        kind: "page",
        label: "页面上下文",
        summary: Some(summary),
        text: raw,
    })
}

fn user_parts(mut text: &str) -> (Vec<Context<'_>>, &str) {
    text = text.trim();
    if let Some(page) = page_context(text) {
        return (vec![page], "");
    }
    let mut contexts = Vec::new();
    loop {
        let original = text;
        let (prefix, candidate) = if text.starts_with("# AGENTS.md instructions for") {
            match text.split_once('\n') {
                Some((heading, rest)) => (heading.len() + 1, rest.trim_start()),
                None => break,
            }
        } else {
            (0, text)
        };
        let mut found = None;
        for (tag, label) in [
            ("environment_context", "运行环境"),
            ("user_instructions", "用户预设指令"),
            ("turn_context", "轮次上下文"),
            ("recommended_plugins", "插件上下文"),
            ("skill", "技能上下文"),
            ("INSTRUCTIONS", "项目指令"),
        ] {
            if prefix > 0 && tag != "INSTRUCTIONS" {
                continue;
            }
            if let Some((_, rest)) = block(candidate, tag) {
                let end = original.len() - rest.len();
                found = Some((
                    Context {
                        kind: "environment",
                        label,
                        summary: None,
                        text: &original[..end],
                    },
                    rest.trim_start(),
                ));
                break;
            }
        }
        let Some((context, rest)) = found else { break };
        contexts.push(context);
        text = rest;
    }
    (contexts, text.trim())
}

pub(super) fn convert_message(
    role: &str,
    text: &str,
    ts: Option<&str>,
    cwd: Option<&str>,
    out: &mut Vec<Value>,
) {
    let text = text.trim();
    if text.is_empty() {
        return;
    }
    let (contexts, conversation) = match role {
        "developer" | "system" => {
            let label = if role == "system" {
                "系统指令"
            } else if text.starts_with("<app-context>") {
                "应用上下文"
            } else if text.starts_with("<codex_apps_open_page_instructions>") {
                "页面上下文说明"
            } else if text.starts_with("<codex_apps_client_time_context>") {
                "时间上下文"
            } else {
                "开发者指令"
            };
            (
                vec![Context {
                    kind: "instructions",
                    label,
                    summary: None,
                    text,
                }],
                "",
            )
        }
        "user" => user_parts(text),
        "assistant" => (Vec::new(), text),
        _ => return,
    };
    for context in contexts {
        out.push(json!({"type":"context", "timestamp":ts, "cwd":cwd,
            "context":{"kind":context.kind,"label":context.label,"summary":context.summary},
            "message":{"role":role,"content":context.text}}));
    }
    if !conversation.is_empty() {
        out.push(json!({"type":role,"timestamp":ts,"cwd":cwd,
            "message":{"role":role,"content":conversation}}));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_complete_reserved_envelopes_are_context() {
        for text in [
            "请解释 <external_codex_apps_open_page>{\"page_id\":null}</external_codex_apps_open_page>",
            "```xml\n<external_codex_apps_open_page>{\"page_id\":null}</external_codex_apps_open_page>\n```",
            "<external_codex_apps_open_page>{\"page_id\":null}</external_codex_apps_open_page> 这是什么意思？",
            "<external_codex_apps_open_page>broken</external_codex_apps_open_page>",
            "<external_codex_apps_open_page>{\"page_id\":12}</external_codex_apps_open_page>",
            "<external_codex_apps_open_page>{\"page_id\":null,\"request\":\"keep this\"}</external_codex_apps_open_page>",
            "<environment_context>unclosed",
            "<environment_context_example>keep</environment_context_example>",
            "You are Codex. 请帮我解释这句提示词。",
        ] {
            let mut events = Vec::new();
            convert_message("user", text, None, None, &mut events);
            assert_eq!(events.len(), 1);
            assert_eq!(events[0]["type"], "user");
            assert_eq!(events[0]["message"]["content"], text);
        }
    }

    #[test]
    fn environment_prefix_keeps_the_request_and_original_context() {
        let text = "<environment_context>cwd</environment_context>\n<skill name=\"sample\">guide</skill>\n真正的问题";
        let (contexts, remaining) = user_parts(text);
        assert_eq!(remaining, "真正的问题");
        assert_eq!(contexts.len(), 2);
        assert_eq!(
            contexts[0].text,
            "<environment_context>cwd</environment_context>"
        );
        assert_eq!(
            user_parts(
                "# AGENTS.md instructions for /tmp\n\n<INSTRUCTIONS>rules</INSTRUCTIONS>\nquestion"
            )
            .1,
            "question"
        );
        assert_eq!(
            user_parts("<environment_context>only environment</environment_context>").1,
            ""
        );
    }
}
