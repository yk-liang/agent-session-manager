use super::super::tool_ops::{self, text, Choice, Hunk, Op, Question, ToolResult};
use serde_json::{json, Value};
use std::collections::HashMap;

/// 技能正文由紧随调用的 meta 消息注入（`sourceToolUseID` 指向 Skill 调用）；`injected` 按调用 ID 记录该消息的 uuid 和正文。
/// 正文并入 Skill 卡片后，原来的上下文事件不再单独显示。
pub(super) fn describe(events: &mut Vec<Value>, mut injected: HashMap<String, (String, String)>) {
    let mut merged = Vec::new();
    tool_ops::describe_tools(events, |call, result| {
        if call["name"] != "Skill" {
            return describe_call(call, result);
        }
        let (name, args) = (text(&call["input"], "skill").unwrap_or("?"), text(&call["input"], "args"));
        let body = result.as_ref().filter(|result| !result.failed()).and(call["id"].as_str()).and_then(|id| injected.remove(id));
        match body {
            Some((uuid, body)) => {
                merged.push(uuid);
                vec![tool_ops::loaded_skill(name, args, &body)]
            }
            None => vec![Op::Skill { name: name.into(), args: args.map(Into::into), path: None, description: None, body: None }],
        }
    });
    if !merged.is_empty() {
        events.retain(|event| event["type"] != "context" || !event["uuid"].as_str().is_some_and(|uuid| merged.iter().any(|m| m == uuid)));
    }
}

/// 注入消息只含文本时才能并入调用结果；含图片等其他内容的仍作为上下文显示。
pub(super) fn injected_text(event: &Value) -> Option<String> {
    match &event["message"]["content"] {
        Value::String(text) => Some(text.clone()),
        Value::Array(parts) => parts.iter().map(|part| (part["type"] == "text").then(|| part["text"].as_str()).flatten())
            .collect::<Option<Vec<_>>>().map(|texts| texts.join("\n\n")),
        _ => None,
    }
}

/// `toolUseResult` 记录了实际读取的行范围和带行号的补丁；用完即从事件中移除。
fn describe_call(call: &Value, result: Option<ToolResult<'_>>) -> Vec<Op> {
    let mut result = result;
    let outcome = result.as_mut()
        .and_then(|result| result.event.as_object_mut()?.remove("toolUseResult"))
        .unwrap_or(Value::Null);
    let output = result.as_ref().and_then(ToolResult::text).map(str::to_owned);
    let output = output.as_deref();
    let input = &call["input"];
    let path = text(input, "file_path").or_else(|| text(input, "path"));
    let paths = || path.into_iter().map(Into::into).collect::<Vec<_>>();
    let name = call["name"].as_str().unwrap_or_default();
    let op = match (name, path) {
        ("Read", Some(path)) => {
            let file = &outcome["file"];
            match (file["content"].as_str(), file["startLine"].as_u64(), file["numLines"].as_u64()) {
                (Some(content), Some(start), Some(lines)) => {
                    if start == 1 && file["totalLines"].as_u64() == Some(lines) {
                        if let Some(skill) = tool_ops::read_skill(path, content) {
                            return vec![skill];
                        }
                    }
                    if let Some(result) = result {
                        result.block["content"] = json!(content);
                    }
                    Op::read_range(path, start, lines, file["totalLines"].as_u64())
                }
                _ => Op::read(path),
            }
        }
        ("Edit" | "MultiEdit", Some(path)) => Op::edit(path, patch(&outcome).unwrap_or_else(|| {
            let edits = input["edits"].as_array().map_or_else(|| vec![input], |edits| edits.iter().collect());
            edits.iter().map(|edit| Hunk::replacement(
                edit["old_string"].as_str().unwrap_or_default(),
                edit["new_string"].as_str().unwrap_or_default(),
                None,
            )).collect()
        })),
        ("Write", Some(path)) => match patch(&outcome) {
            Some(hunks) if outcome["type"] == "update" => Op::edit(path, hunks),
            _ => Op::write(path, input["content"].as_str().unwrap_or_default(), (outcome["type"] == "create").then_some(true)),
        },
        ("Grep", _) => {
            let Some(pattern) = text(input, "pattern") else { return Vec::new() };
            let mode = text(input, "output_mode").unwrap_or("files_with_matches");
            tool_ops::search(pattern, paths(), mode == "files_with_matches", output.filter(|_| mode != "count"))
        }
        ("Glob", _) => tool_ops::find(text(input, "pattern"), paths(), output),
        ("LS", _) => tool_ops::list(paths(), output),
        ("Bash", _) => match text(input, "command") {
            Some(command) => tool_ops::shell(command, text(input, "description"), None, output),
            None => return Vec::new(),
        },
        ("WebSearch", _) => match text(input, "query") {
            Some(query) => Op::web_search(query, None, output),
            None => return Vec::new(),
        },
        ("WebFetch", _) => match text(input, "url") {
            Some(url) => Op::WebFetch { url: url.into(), prompt: text(input, "prompt").map(Into::into) },
            None => return Vec::new(),
        },
        ("AskUserQuestion", _) => Op::ask(input["questions"].as_array().into_iter().flatten().filter_map(|q| Some(Question {
            question: text(q, "question")?.into(),
            header: text(q, "header").map(Into::into),
            options: q["options"].as_array().into_iter().flatten().filter_map(|o| Some(Choice {
                label: text(o, "label")?.into(),
                description: text(o, "description").map(Into::into),
            })).collect(),
            answer: None,
        })).collect(), output),
        ("Agent" | "Task", _) => Op::Agent {
            agent: text(input, "subagent_type").map(Into::into),
            task: text(input, "description").unwrap_or("子任务").into(),
            background: input["run_in_background"] == true || outcome["isAsync"] == true,
            encrypted: false,
        },
        ("SendMessage", _) => match text(input, "to") {
            Some(to) => Op::Message { to: to.into(), summary: text(input, "summary").map(Into::into), encrypted: false },
            None => return Vec::new(),
        },
        ("ToolSearch", _) => match text(input, "query") {
            Some(query) => Op::LoadTools {
                names: query.strip_prefix("select:").map_or_else(|| vec![query.into()], |names| names.split(',').map(|n| n.trim().into()).collect()),
            },
            None => return Vec::new(),
        },
        ("TaskStop" | "KillShell" | "KillBash", _) => match text(input, "task_id").or_else(|| text(input, "shell_id")) {
            Some(task) => Op::Stop {
                task: task.into(),
                detail: output.and_then(|output| output.split_once(" (")).map(|(_, rest)| rest.trim_end_matches(['}', '"', ')']).into()),
            },
            None => return Vec::new(),
        },
        ("Monitor", _) => Op::Monitor {
            description: text(input, "description").unwrap_or("后台事件").into(),
            minutes: input["timeout_ms"].as_u64().map(|ms| ms.div_ceil(60_000)),
        },
        ("ScheduleWakeup", _) => match input["delaySeconds"].as_f64() {
            Some(seconds) => Op::Wake { seconds, reason: text(input, "reason").map(Into::into) },
            None => return Vec::new(),
        },
        ("TodoWrite", _) => Op::todo(&input["todos"], "content"),
        _ => match name.strip_prefix("mcp__").and_then(|rest| rest.rsplit_once("__")) {
            Some((server, tool)) => Op::Mcp { server: server.into(), tool: tool.into(), title: None },
            None => return Vec::new(),
        },
    };
    vec![op]
}

fn patch(outcome: &Value) -> Option<Vec<Hunk>> {
    let hunks = outcome["structuredPatch"].as_array().filter(|hunks| !hunks.is_empty())?;
    Some(hunks.iter().map(|hunk| Hunk {
        old_start: hunk["oldStart"].as_u64(),
        new_start: hunk["newStart"].as_u64(),
        lines: hunk["lines"].as_array().into_iter().flatten()
            .filter_map(|line| line.as_str().map(str::to_owned)).collect(),
    }).collect())
}
