use super::{context, tool_output};
use serde_json::{json, Value};

pub(super) fn text_of_content<'a>(items: impl IntoIterator<Item = &'a Value>) -> String {
    let mut text = String::new();
    for item in items {
        let t = item.get("type").and_then(Value::as_str).unwrap_or("");
        if matches!(t, "input_text" | "output_text" | "text" | "summary_text" | "reasoning_text") {
            if let Some(s) = item.get("text").and_then(Value::as_str) {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(s);
            }
        }
    }
    text
}

fn parse_arguments(raw: &str) -> Value {
    serde_json::from_str(raw).unwrap_or_else(|_| json!({ "raw": raw }))
}

pub(super) fn convert(
    payload: &Value,
    ts: Option<&str>,
    cwd: Option<&str>,
    out: &mut Vec<Value>,
) {
    let kind = payload.get("type").and_then(Value::as_str).unwrap_or("");
    match kind {
        "message" => {
            let role = payload.get("role").and_then(Value::as_str).unwrap_or("");
            let items = payload.get("content").and_then(Value::as_array);
            let Some(items) = items else { return };
            context::convert_message_content(role, items, ts, cwd, out);
        }
        "reasoning" => {
            let thinking = ["summary", "content"].into_iter()
                .filter_map(|key| payload[key].as_array())
                .map(|items| text_of_content(items))
                .filter(|text| !text.trim().is_empty())
                .collect::<Vec<_>>()
                .join("\n\n");
            if thinking.trim().is_empty() {
                return;
            }
            out.push(json!({
                "type": "assistant",
                "timestamp": ts,
                "cwd": cwd,
                "message": {
                    "role": "assistant",
                    "content": [{ "type": "thinking", "thinking": thinking }],
                },
            }));
        }
        "compaction" | "compaction_summary" | "context_compaction" => {
            out.push(context::compaction(payload["summary"].as_str(), ts, cwd));
        }
        "web_search_call" | "tool_search_call" => {
            let (name, input) = if kind == "web_search_call" {
                ("web_search", &payload["action"])
            } else {
                ("tool_search", &payload["arguments"])
            };
            out.push(json!({"type":"assistant","timestamp":ts,"cwd":cwd,
                "message":{"role":"assistant","content":[{
                    "type":"tool_use","id":payload["call_id"].as_str().or_else(|| payload["id"].as_str()),
                    "name":name,"input":input,"status":match payload["status"].as_str() {
                        Some("completed") => Some("completed"),
                        Some("in_progress" | "searching") => Some("running"),
                        Some("failed" | "error") => Some("error"),
                        _ => None
                    }
                }]}}));
        }
        "tool_search_output" => {
            out.push(json!({"type":"user","timestamp":ts,"cwd":cwd,
                "message":{"role":"user","content":[{
                    "type":"tool_result","tool_use_id":payload["call_id"],"content":payload["tools"],
                    "is_error":payload["status"] == "failed"
                }]}}));
        }
        "function_call" | "custom_tool_call" => {
            let name = payload.get("name").and_then(Value::as_str).unwrap_or("?");
            let call_id = payload.get("call_id").and_then(Value::as_str).unwrap_or("");
            let input = if kind == "function_call" {
                parse_arguments(
                    payload
                        .get("arguments")
                        .and_then(Value::as_str)
                        .unwrap_or("{}"),
                )
            } else {
                json!(payload.get("input").and_then(Value::as_str).unwrap_or(""))
            };
            out.push(json!({
                "type": "assistant",
                "timestamp": ts,
                "cwd": cwd,
                "message": {
                    "role": "assistant",
                    "content": [{ "type": "tool_use", "id": call_id, "name": name, "input": input }],
                },
            }));
        }
        "function_call_output" | "custom_tool_call_output" => {
            let call_id = payload.get("call_id").and_then(Value::as_str).unwrap_or("");
            let output = &payload["output"];
            let is_error = payload.get("output").is_some_and(tool_output::failed);
            let content = tool_output::content(output);
            out.push(json!({
                "type": "user",
                "timestamp": ts,
                "cwd": cwd,
                "message": {
                    "role": "user",
                    "content": [{
                        "type": "tool_result",
                        "tool_use_id": call_id,
                        "content": content,
                        "is_error": is_error,
                    }],
                },
            }));
        }
        _ => {}
    }
}
