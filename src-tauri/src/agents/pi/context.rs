use super::super::context::{block, record};
use serde_json::{json, Value};

pub(super) fn entry(v: &Value, cwd: Option<&str>, collect_context: bool) -> Vec<Value> {
    let kind = v["type"].as_str().unwrap_or("");
    let role = v["message"]["role"].as_str().unwrap_or("");
    let label = match kind {
        "compaction" => Some(("compaction", "压缩摘要")),
        "branch_summary" => Some(("compaction", "分支摘要")),
        "custom_message" => Some(("extension", "扩展上下文")),
        "message" => match role {
            "system" => Some(("instructions", "系统指令")),
            "developer" => Some(("instructions", "开发者指令")),
            "custom" | "hookMessage" => Some(("extension", "扩展上下文")),
            "compactionSummary" => Some(("compaction", "压缩摘要")),
            "branchSummary" => Some(("compaction", "分支摘要")),
            _ => None,
        },
        _ => return Vec::new(),
    };
    if let Some((kind, label)) = label {
        return if collect_context {
            vec![record(v, kind, label, v)]
        } else {
            Vec::new()
        };
    }
    if !matches!(role, "user" | "assistant") {
        return Vec::new();
    }
    let text = v
        .pointer("/message/content")
        .map(super::spoken_text)
        .unwrap_or_default();
    let text = text.trim();
    if text.is_empty() {
        return Vec::new();
    }
    let mut events = Vec::new();
    let mut normal = text.to_string();
    // Pi's skill expansion records this exact envelope instead of the slash command.
    // Keep the invocation as user text, and expose the injected skill body separately.
    if role == "user" && text.starts_with("<skill name=\"") {
        if let Some((raw, body, rest)) = block(text, "skill") {
            let opening = &raw[..raw.find('>').unwrap()];
            let name = opening
                .strip_prefix("<skill name=\"")
                .and_then(|s| s.split_once('"'))
                .map(|v| v.0);
            if opening.contains(" location=\"") && body.starts_with('\n') {
                if let Some(name) = name.filter(|name| !name.is_empty()) {
                    normal = format!(
                        "/skill:{name}{}",
                        if rest.trim().is_empty() {
                            String::new()
                        } else {
                            format!("\n{}", rest.trim())
                        }
                    );
                    if collect_context {
                        let mut context = record(v, "instructions", "技能上下文", &json!(raw));
                        if let Some(id) = v["id"].as_str() {
                            context["uuid"] = json!(format!("{id}:context"));
                        }
                        events.push(context);
                    }
                }
            }
        }
    }
    let mut event = json!({"type":role,"message":{"role":role,"content":normal},"cwd":cwd});
    if let Some(id) = v.get("id") {
        event["uuid"] = id.clone();
    }
    if let Some(timestamp) = v.get("timestamp") {
        event["timestamp"] = timestamp.clone();
    }
    events.push(event);
    events
}
