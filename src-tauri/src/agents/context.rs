//! Shared presentation for recorded agent context. Classification stays in each adapter.
use serde_json::{json, Value};

pub(super) fn has_text(value: &Value) -> bool {
    match value {
        Value::String(text) => !text.trim().is_empty(),
        Value::Array(parts) => parts.iter().any(|part| {
            part["type"] == "text"
                && part["text"]
                    .as_str()
                    .is_some_and(|text| !text.trim().is_empty())
        }),
        _ => false,
    }
}

pub(super) fn record(source: &Value, kind: &str, label: &str, content: &Value) -> Value {
    let content = content
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| serde_json::to_string_pretty(content).unwrap_or_default());
    let mut event = json!({"type":"context", "context":{"kind":kind,"label":label},
        "message":{"role":source.pointer("/message/role").and_then(Value::as_str)
            .or_else(|| source.get("type").and_then(Value::as_str)).unwrap_or("system"),
            "content":content}});
    for key in ["uuid", "timestamp", "cwd"] {
        if let Some(value) = source.get(key) {
            event[key] = value.clone();
        }
    }
    if event.get("uuid").is_none() {
        if let Some(id) = source.get("id") {
            event["uuid"] = id.clone();
        }
    }
    event
}

/// Exact tag boundary, complete closing tag; callers decide whether trailing prose is allowed.
pub(super) fn block<'a>(text: &'a str, tag: &str) -> Option<(&'a str, &'a str, &'a str)> {
    let after = text.strip_prefix(&format!("<{tag}"))?;
    if !after.starts_with('>') && !after.starts_with(char::is_whitespace) {
        return None;
    }
    let body = text.find('>')? + 1;
    let closing = format!("</{tag}>");
    let end = body + text[body..].find(&closing)?;
    Some((
        &text[..end + closing.len()],
        &text[body..end],
        &text[end + closing.len()..],
    ))
}
