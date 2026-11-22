use super::super::{context::{block, record}, ms_to_iso};
use serde_json::{json, Value};

pub(super) fn timestamp(entry: &Value) -> Option<String> {
    entry["timestamp"].as_str()
        .filter(|value| super::super::timestamp(Some(value)).is_some())
        .map(str::to_owned)
        .or_else(|| entry["timestamp"].as_i64().or_else(|| entry["message"]["timestamp"].as_i64()).and_then(ms_to_iso))
}

pub(super) fn is_conversation_message(entry: &Value) -> bool {
    entry["type"] == "message"
        && matches!(
            entry["message"]["role"].as_str(),
            Some("user" | "assistant" | "toolResult" | "bashExecution")
        )
}

fn message_event(entry: &Value, cwd: Option<&str>, role: &str, content: Value) -> Value {
    json!({"type":role,"uuid":entry["id"],"timestamp":timestamp(entry),"cwd":cwd,
        "message":{"role":role,"content":content}})
}

fn blocks(content: &Value, detail: bool) -> Vec<Value> {
    if let Some(text) = content.as_str().filter(|text| !text.trim().is_empty()) {
        return vec![json!({"type":"text","text":text.trim()})];
    }
    content.as_array().into_iter().flatten().filter_map(|part| {
        match part["type"].as_str()? {
            "text" => {
                let text = part["text"].as_str()?.trim();
                (!text.is_empty()).then(|| json!({"type":"text","text":text}))
            }
            "thinking" => {
                let text = part["thinking"].as_str().filter(|text| !text.trim().is_empty());
                text.map(|text| json!({"type":"thinking","thinking":if detail { text } else { "" }}))
            }
            "toolCall" => Some(if detail {
                json!({"type":"tool_use","id":part["id"],"name":part["name"],"input":part["arguments"]})
            } else {
                json!({"type":"tool_use"})
            }),
            "image" | "input_image" => {
                if !detail {
                    return Some(json!({"type":"image"}));
                }
                if let (Some(data), Some(mime)) = (part["data"].as_str(), part["mimeType"].as_str()) {
                    Some(json!({"type":"image","source":{"type":"base64","media_type":mime,"data":data}}))
                } else {
                    Some(part.clone())
                }
            }
            _ => None,
        }
    }).collect()
}

fn skill(text: &str) -> Option<(&str, &str, &str)> {
    let (raw, body, rest) = block(text, "skill")?;
    let opening = raw.split_once('>')?.0;
    let (name, attributes) = opening.strip_prefix("<skill name=\"")?.split_once('"')?;
    if name.is_empty() || !attributes.contains(" location=\"") || !body.starts_with('\n') {
        return None;
    }
    Some((raw, name, rest.trim()))
}

pub(super) fn entry(v: &Value, cwd: Option<&str>, detail: bool) -> Vec<Value> {
    let kind = v["type"].as_str().unwrap_or_default();
    let message = &v["message"];
    let role = message["role"].as_str().unwrap_or_default();
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
        return if detail { vec![record(v, kind, label, v)] } else { Vec::new() };
    }

    match role {
        "user" | "assistant" => {
            let mut events = Vec::new();
            let mut content = blocks(&message["content"], detail);
            if role == "user" {
                if let Some(first) = content.first_mut() {
                    if let Some((raw, name, rest)) = first["text"].as_str().and_then(skill) {
                        if detail {
                            let mut context = record(v, "instructions", "技能上下文", &json!(raw));
                            if let Some(id) = v["id"].as_str() {
                                context["uuid"] = json!(format!("{id}:context"));
                            }
                            events.push(context);
                        }
                        first["text"] = json!(if rest.is_empty() {
                            format!("/skill:{name}")
                        } else {
                            format!("/skill:{name}\n{rest}")
                        });
                    }
                }
            }
            if !content.is_empty() {
                let content = if content.iter().all(|block| block["type"] == "text") {
                    json!(content.iter().filter_map(|block| block["text"].as_str()).collect::<Vec<_>>().join("\n"))
                } else {
                    json!(content)
                };
                events.push(message_event(v, cwd, role, content));
            }
            if detail && role == "assistant" && message["stopReason"] == "error" {
                let error = message["errorMessage"].as_str().filter(|text| !text.trim().is_empty())
                    .unwrap_or("Pi 请求失败");
                events.push(json!({"type":"system","uuid":v["id"].as_str().map(|id| format!("{id}:error")),
                    "timestamp":timestamp(v),"cwd":cwd,"content":error}));
            }
            events
        }
        "toolResult" if detail => vec![message_event(v, cwd, "user", json!([{
            "type":"tool_result","tool_use_id":message["toolCallId"],"is_error":message["isError"] == true,
            "content":blocks(&message["content"], true)
        }]))],
        "bashExecution" if detail => {
            let id = v["id"].as_str().map(|id| format!("bash:{id}"));
            let cancelled = message["cancelled"] == true;
            let exit = message["exitCode"].as_i64();
            let mut output = message["output"].as_str().unwrap_or_default().to_owned();
            let status = if cancelled {
                Some("命令已取消".into())
            } else {
                exit.filter(|&exit| exit != 0).map(|exit| format!("退出码：{exit}"))
            };
            if let Some(status) = &status {
                if !output.is_empty() {
                    output.push('\n');
                }
                output.push_str(status);
            }
            vec![message_event(v, cwd, "assistant", json!([
                {"type":"tool_use","id":id,"name":"bash","input":{"command":message["command"]}},
                {"type":"tool_result","tool_use_id":id,"is_error":status.is_some(),"content":output}
            ]))]
        }
        _ => Vec::new(),
    }
}
