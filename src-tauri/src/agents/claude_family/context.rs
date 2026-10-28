use super::super::context::{block, record};
use serde_json::{json, Value};

const TAGS: &[(&str, &str)] = &[
    ("system-reminder", "系统提醒"),
    ("local-command-caveat", "本地命令说明"),
    ("local-command-stdout", "本地命令输出"),
    ("command-stdout", "命令输出"),
    ("user-prompt-submit-hook", "钩子上下文"),
    ("hook_context", "钩子上下文"),
    ("loaded_context", "加载的上下文"),
    ("environment_context", "运行环境"),
    ("user_instructions", "用户预设指令"),
    ("turn_context", "轮次上下文"),
];

/// Only complete structured envelopes qualify. Prose, quotations and code fences stay intact.
fn structured(text: &str) -> Option<(Vec<(&'static str, String)>, String)> {
    let mut rest = text.trim();
    let mut contexts = Vec::new();
    let mut command = Vec::new();
    while !rest.is_empty() {
        if let Some((_, body, tail)) = block(rest, "command-name") {
            if !body.trim().starts_with('/') {
                return None;
            }
            command.push(body.trim().to_string());
            rest = tail.trim_start();
        } else if let Some((_, body, tail)) = block(rest, "command-args") {
            if command.is_empty() {
                return None;
            }
            command.push(body.trim().to_string());
            rest = tail.trim_start();
        } else {
            let mut found = None;
            for (tag, label) in TAGS
                .iter()
                .copied()
                .chain([("command-message", "命令上下文")])
            {
                if let Some((raw, _, tail)) = block(rest, tag) {
                    found = Some((label, raw, tail));
                    break;
                }
            }
            let (label, raw, tail) = found?;
            contexts.push((label, raw.to_string()));
            rest = tail.trim_start();
        }
    }
    if contexts.is_empty() && command.is_empty() {
        return None;
    }
    Some((contexts, command.join(" ")))
}

pub(super) fn visible_text(text: &str) -> String {
    structured(text)
        .map(|(_, command)| command)
        .unwrap_or_else(|| text.to_string())
}

fn context_label(v: &Value) -> Option<(&'static str, &'static str)> {
    match v["type"].as_str()? {
        "attachment" => Some(match v["attachment"]["type"].as_str().unwrap_or("") {
            "prompt_snapshot" => ("instructions", "系统提示词快照"),
            "environment" => ("environment", "运行环境"),
            "date" => ("environment", "时间上下文"),
            "context_sections" => ("instructions", "项目与记忆上下文"),
            "session_context" => ("environment", "会话上下文"),
            _ => ("attachment", "附件上下文"),
        }),
        "developer" => Some(("instructions", "开发者指令")),
        "system" => {
            // Operational errors/cancellation are useful status, not prompt context.
            let subtype = v["subtype"].as_str().unwrap_or("");
            if matches!(subtype, "api_error" | "error" | "informational")
                || v["level"].as_str() == Some("error")
            {
                return None;
            }
            Some(
                if matches!(subtype, "compact_boundary" | "microcompact_boundary") {
                    ("compaction", "上下文压缩记录")
                } else if subtype.is_empty() || subtype == "system_prompt" {
                    ("instructions", "系统指令")
                } else {
                    ("runtime", "运行记录")
                },
            )
        }
        "user" | "assistant" if v["isCompactSummary"] == true => Some(("compaction", "压缩摘要")),
        "user" | "assistant" if v["isMeta"] == true => Some(("instructions", "自动注入上下文")),
        "user" if v["isVisibleInTranscriptOnly"] == true => Some(("runtime", "本地运行记录")),
        _ => None,
    }
}

pub(super) fn normalize(v: Value) -> Vec<Value> {
    if let Some((kind, label)) = context_label(&v) {
        let event = record(&v, kind, label, &v);
        // Mixed tool-result envelopes must keep their tool results accessible.
        let tools: Vec<_> = v
            .pointer("/message/content")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|b| matches!(b["type"].as_str(), Some("tool_result" | "tool_use")))
            .cloned()
            .collect();
        if tools.is_empty() {
            return vec![event];
        }
        let mut normal = v;
        normal["message"]["content"] = json!(tools);
        normal.as_object_mut().unwrap().remove("isMeta");
        let mut context = event;
        if let Some(id) = normal["uuid"].as_str() {
            context["uuid"] = json!(format!("{id}:context"));
        }
        return vec![context, normal];
    }
    if v["type"] != "user" {
        return vec![v];
    }
    let Some(content) = v.pointer("/message/content") else {
        return vec![v];
    };
    let mut contexts = Vec::new();
    let mut clean = |text: &str| {
        if let Some((parts, command)) = structured(text) {
            for (label, raw) in parts {
                contexts.push((label, raw));
            }
            command
        } else {
            text.to_string()
        }
    };
    let content = match content {
        Value::String(text) => json!(clean(text)),
        Value::Array(parts) => json!(parts
            .iter()
            .filter_map(|part| {
                let mut part = part.clone();
                if part["type"] == "text" {
                    if let Some(text) = part["text"].as_str() {
                        let text = clean(text);
                        if text.trim().is_empty() {
                            return None;
                        }
                        part["text"] = json!(text);
                    }
                }
                Some(part)
            })
            .collect::<Vec<_>>()),
        _ => content.clone(),
    };
    let mut events = Vec::new();
    for (i, (label, text)) in contexts.into_iter().enumerate() {
        let mut context = record(&v, "instructions", label, &json!(text));
        if let Some(id) = v["uuid"].as_str() {
            context["uuid"] = json!(format!("{id}:context:{i}"));
        }
        events.push(context);
    }
    if content.as_str().is_some_and(|s| !s.trim().is_empty())
        || content.as_array().is_some_and(|a| !a.is_empty())
    {
        let mut normal = v;
        normal["message"]["content"] = content;
        events.push(normal);
    }
    events
}
