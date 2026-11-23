//! 保留消息、分段顺序；用户多段正文合并为一条指令，助手分段独立供虚拟列表渲染。
use super::*;
use crate::agents::tool_ops::Op;

fn error_text(error: &Value) -> Option<&str> {
    [error, &error["message"], &error["data"]["message"], &error["type"], &error["name"], &error["kind"]]
        .into_iter().filter_map(Value::as_str).find(|text| !text.trim().is_empty())
}

fn with_error(error: &str, output: Option<&Value>) -> Value {
    match output {
        Some(Value::Array(parts)) => {
            let mut result = Vec::with_capacity(parts.len() + 1);
            result.push(json!({"type":"text","text":error}));
            result.extend(parts.iter().cloned());
            json!(result)
        }
        Some(Value::String(text)) if !text.is_empty() && text != error => json!(format!("{error}\n\n{text}")),
        Some(value) if !value.is_string() => json!([{"type":"text","text":error}, value]),
        _ => json!(error),
    }
}

pub(super) fn tool_blocks(
    part: &Value,
    identity: &str,
    include_content: bool,
) -> (Value, Option<Value>) {
    let state = &part["state"];
    let status = state.get("status").and_then(Value::as_str);
    let status = if status == Some("streaming") {
        Some("pending")
    } else {
        status
    };
    let name = part.get("tool").or_else(|| part.get("name")).and_then(Value::as_str).unwrap_or("?");
    let mut call = json!({"type":"tool_use", "id":identity, "name":name});
    let (ops, content) = super::tools::describe(name, state);
    if !ops.is_empty() {
        call["ops"] = if include_content { json!(ops) } else { json!(ops.into_iter().map(Op::summary).collect::<Vec<_>>()) };
    }
    if let Some(description) = state
        .get("title")
        .and_then(Value::as_str)
        .filter(|title| !title.trim().is_empty())
    {
        call["description"] = json!(description);
    }
    if include_content {
        call["input"] = state
            .get("input")
            .or_else(|| state.get("raw"))
            .cloned()
            .unwrap_or(Value::Null);
    }
    if matches!(status, Some("pending" | "running" | "completed" | "error")) {
        call["status"] = json!(status);
    }
    let failed = status == Some("error") || state.get("error").is_some_and(|v| !v.is_null());
    // 有输出不代表已经完成：运行状态单独保留，pending/running 不伪造成功结果。
    let output = (if failed { ["content", "output"] } else { ["output", "content"] })
        .into_iter().find_map(|key| state.get(key).filter(|value| !value.is_null()));
    let result = if matches!(status, Some("completed" | "error")) || output.is_some() || failed {
        let mut result = json!({"type":"tool_result", "tool_use_id":identity, "is_error":failed});
        if failed && matches!(name, "bash" | "shell") {
            let exit = state["metadata"]["exit"].as_i64().or_else(|| {
                error_text(&state["error"])?.trim().strip_prefix("Exit code ")?.parse::<i64>().ok()
            });
            if let Some(exit) = exit {
                result["exit_code"] = json!(exit);
            }
        }
        if include_content {
            result["content"] = if let Some(error) = error_text(&state["error"]) {
                with_error(error, output)
            } else if let Some(text) = content {
                json!(text)
            } else {
                output.cloned().unwrap_or_else(|| json!(if failed { "工具执行失败" } else { "" }))
            };
        }
        Some(result)
    } else {
        None
    };
    (call, result)
}

fn event(role: &str, id: &str, timestamp: Option<String>, content: Value) -> Value {
    json!({"type":role,"uuid":id,"timestamp":timestamp,"message":{"role":role,"content":content}})
}

fn add_user_block(
    events: &mut Vec<Value>,
    index: &mut Option<usize>,
    id: &str,
    time: Option<String>,
    block: Value,
) {
    if let Some(index) = *index {
        let content = &mut events[index]["message"]["content"];
        if let (Some(previous), Some(text)) = (content.as_str(), block.as_str()) {
            *content = json!(format!("{previous}\n{text}"));
        } else {
            if !content.is_array() {
                *content = json!([{"type":"text","text":content.take()}]);
            }
            let block = if block.is_string() { json!({"type":"text","text":block}) } else { block };
            content.as_array_mut().unwrap().push(block);
        }
    } else {
        *index = Some(events.len());
        let content = if block.is_string() { block } else { json!([block]) };
        events.push(event("user", id, time, content));
    }
}

pub(super) fn read(
    conn: &Connection,
    path: &Path,
    session_id: &str,
    view: bool,
) -> Result<(Vec<Value>, Vec<String>), String> {
    // 同毫秒写入依靠 OpenCode 的单调 ID 稳定排序，不能按类型分组后再拼接。
    let order = storage::order_column(conn);
    let mut stmt = conn
        .prepare(&format!(
            "SELECT m.id,m.data,p.id,p.data,COALESCE(p.time_created,m.time_created)
        FROM message m LEFT JOIN part p ON p.message_id=m.id WHERE m.session_id=?1
        ORDER BY m.{order},m.id,p.time_created,p.id"
        ))
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([session_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, Option<i64>>(4)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    let mut events = Vec::new();
    let mut warnings = Vec::new();
    let mut current_msg = String::new();
    let mut role = String::new();
    let mut user_index: Option<usize> = None;
    let mut summary = false;
    let mut user_time = None;
    let mut bad = 0u32;
    let mut current_model = None;
    let mut message_error = None;
    for row in rows {
        let (msg_id, msg_data, part_id, part_data, created) = match row {
            Ok(row) => row,
            Err(_) => {
                bad += 1;
                continue;
            }
        };
        if msg_id != current_msg {
            events.extend(message_error.take());
            user_index = None;
            current_msg = msg_id;
            let message = serde_json::from_str::<Value>(&msg_data).unwrap_or(Value::Null);
            role = message
                .get("role")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            summary = message["summary"] == true;
            current_model = super::message_meta::from_message(&message);
            user_time = created.and_then(ms_to_iso);
            let error = error_text(&message["error"]);
            let notice = if role == "idle" {
                match message["outcome"].as_str() {
                    Some("failed") => Some(error.map_or_else(|| "轮次失败".into(), |text| format!("轮次失败：{text}"))),
                    Some("interrupted") => Some("轮次已中断".into()),
                    _ => None,
                }
            } else {
                error.map(str::to_owned)
            };
            message_error = notice.map(|text| json!({"type":"system", "uuid":format!("{current_msg}:status"), "timestamp":user_time, "content":text}));
            if let Some(system) = message.get("system").filter(|v| !v.is_null() && **v != "") {
                let source = event(
                    &role,
                    &format!("{current_msg}:system"),
                    user_time.clone(),
                    Value::Null,
                );
                events.push(super::super::context::record(
                    &source,
                    "instructions",
                    "系统指令",
                    system,
                ));
            }
            if !matches!(
                role.as_str(),
                "user" | "assistant" | "compaction" | "synthetic" | "system" | "developer" | "shell" | "skill" | "idle"
            ) {
                bad += 1;
            }
        }
        if !matches!(
            role.as_str(),
            "user" | "assistant" | "compaction" | "synthetic" | "system" | "developer" | "shell" | "skill" | "idle"
        ) {
            continue;
        }
        let (Some(part_id), Some(data)) = (part_id, part_data) else {
            continue;
        };
        let part: Value = match serde_json::from_str(&data) {
            Ok(part) => part,
            Err(_) => {
                bad += 1;
                continue;
            }
        };
        let id = format!("{current_msg}:{part_id}");
        let time = created.and_then(ms_to_iso);
        let before = events.len();
        let metadata = if role == "compaction" || summary {
            Some(("compaction", "压缩摘要"))
        } else if matches!(role.as_str(), "system" | "developer") {
            Some(("instructions", "系统指令"))
        } else if role == "skill" {
            Some(("instructions", "技能上下文"))
        } else if part["ignored"] == true {
            Some(("synthetic", "已忽略上下文"))
        } else if role == "synthetic" || part["synthetic"] == true {
            Some(("synthetic", "自动注入上下文"))
        } else {
            None
        };
        if let Some((kind, label)) = metadata {
            let source = event(&role, &id, time, Value::Null);
            events.push(super::super::context::record(
                &source,
                kind,
                label,
                if role == "skill" { &part["record"]["text"] } else { part.get("record").unwrap_or(&part) },
            ));
            continue;
        }
        match part.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(text) = part
                    .get("text")
                    .and_then(Value::as_str)
                    .filter(|t| !t.trim().is_empty())
                {
                    if role == "user" {
                        add_user_block(&mut events, &mut user_index, &current_msg, user_time.clone(), json!(text.trim()));
                    } else {
                        events.push(event("assistant", &id, time, json!(text.trim())));
                    }
                }
            }
            Some("reasoning") if role == "assistant" => {
                if let Some(text) = part
                    .get("text")
                    .and_then(Value::as_str)
                    .filter(|t| !t.trim().is_empty())
                {
                    events.push(event(
                        "assistant",
                        &id,
                        time,
                        json!([{"type":"thinking","thinking":text}]),
                    ));
                }
            }
            Some("file") if role == "user" => {
                let attachment = if let (Some(mime), Some(data)) =
                    (part["mime"].as_str(), part["data"].as_str())
                {
                    json!({"type":"file","filename":part["filename"],"source":{"type":"base64","media_type":mime,"data":data}})
                } else {
                    part
                };
                add_user_block(&mut events, &mut user_index, &current_msg, user_time.clone(), attachment);
            }
            Some("tool") if matches!(role.as_str(), "assistant" | "shell") => {
                // 作用域包括源 part，避免不同消息复用 callID 时把结果配给另一张卡片。
                let identity = format!(
                    "{id}:{}",
                    part.get("callID")
                        .or_else(|| part.get("id"))
                        .and_then(Value::as_str)
                        .unwrap_or(&part_id)
                );
                let (mut call, mut result) = tool_blocks(&part, &identity, !view);
                if !part.get("state").is_some_and(Value::is_object) {
                    bad += 1;
                }
                if view {
                    let token = super::deferred::register(
                        path,
                        session_id,
                        &current_msg,
                        &part_id,
                        &identity,
                        &data,
                    );
                    call.as_object_mut().unwrap().remove("input");
                    call["deferred"] = json!(format!("{token}:input"));
                    if let Some(result) = result.as_mut() {
                        result.as_object_mut().unwrap().remove("content");
                        result["deferred"] = json!(format!("{token}:result"));
                    }
                }
                events.push(event("assistant", &id, time.clone(), json!([call])));
                if let Some(result) = result {
                    events.push(event(
                        "user",
                        &format!("{id}:result"),
                        time,
                        json!([result]),
                    ));
                }
            }
            _ => {}
        }
        if let Some(model) = &current_model {
            for event in &mut events[before..] {
                if event["type"] == "assistant" {
                    event["message"]["model"] = json!(model);
                }
            }
        }
    }
    events.extend(message_error);
    if bad > 0 {
        warnings.push(format!(
            "OpenCode 会话有 {bad} 处消息或分段无法解析，当前显示可读取内容"
        ));
    }
    Ok((events, warnings))
}
