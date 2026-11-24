use super::super::tool_ops::{self, first_line, text, Hunk, Op, ToolResult};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

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
            None => vec![Op::Skill { name: name.into(), args: args.map(Into::into), path: None, body: None }],
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
    let recorded = result.as_ref().and_then(ToolResult::text).map(str::to_owned);
    let bash = call["name"] == "Bash";
    let failed = result.as_ref().is_some_and(ToolResult::failed);
    // 命令的 stdout、stderr 和另存的完整输出另有记录；旧日志只能从结果文本里剥掉信封和尾注。
    let body = recorded.as_deref().map(|text| if bash { command_text(text) } else { text });
    let (exit_code, body) = if bash && failed { exit_status(body) } else { (None, body) };
    let (saved, body) = match body.map(|body| persisted(body).ok_or(body)) {
        Some(Ok((file, preview))) => (Some(file), Some(preview)),
        Some(Err(text)) => (None, Some(text)),
        None => (None, None),
    };
    let output = match outcome["stdout"].as_str().filter(|_| bash) {
        Some(stdout) => Some([stdout.trim_end(), command_text(outcome["stderr"].as_str().unwrap_or_default()).trim()]
            .into_iter().filter(|part| !part.is_empty()).collect::<Vec<_>>().join("\n")),
        None => body.map(str::to_owned),
    };
    if let (Some(result), Some(output)) = (&mut result, &output) {
        if Some(output.as_str()) != recorded.as_deref() {
            result.block["output"] = json!(output);
        }
        if let Some(file) = text(&outcome, "persistedOutputPath").or(saved) {
            result.block["output_file"] = json!(file);
        }
    }
    let output = output.as_deref();
    let input = &call["input"];
    let path = text(input, "file_path").or_else(|| text(input, "path"));
    let paths = || path.into_iter().map(Into::into).collect::<Vec<_>>();
    let name = call["name"].as_str().unwrap_or_default();
    let op = match (name, path) {
        ("Read", Some(path)) if outcome["type"] == "image" || result.as_ref().is_some_and(|result|
            result.block["content"].as_array().is_some_and(|parts| parts.iter().any(|part| part["type"] == "image"))) => Op::Image { path: path.into() },
        ("Read", Some(path)) => {
            let file = &outcome["file"];
            match (file["content"].as_str(), file["startLine"].as_u64(), file["numLines"].as_u64()) {
                (Some(content), Some(start), Some(lines)) => {
                    if let Some(result) = result {
                        if Some(content) != recorded.as_deref() {
                            result.block["output"] = json!(content);
                        }
                    }
                    Op::read_range(path, start, lines, file["totalLines"].as_u64())
                }
                _ => Op::read(path),
            }
        }
        ("Edit" | "MultiEdit", Some(path)) => Op::edit(path, patch(&outcome)
            .unwrap_or_else(|| Hunk::replacements(input, "old_string", "new_string"))),
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
            Some(command) => tool_ops::shell(command, text(input, "description"), exit_code, output),
            None => return Vec::new(),
        },
        ("WebSearch" | "web_search", _) => {
            let count = outcome["results"].as_array().and_then(|results| {
                let mut links = results.iter().filter_map(|result| result["content"].as_array()).peekable();
                if results.is_empty() || links.peek().is_some() {
                    Some(links.flatten().filter_map(|link| text(link, "url")).collect::<HashSet<_>>().len())
                } else { None }
            });
            Op::web_search(input, None, count)
        }
        ("WebFetch", _) => match text(input, "url") {
            Some(url) => Op::WebFetch { url: url.into(), prompt: text(input, "prompt").map(Into::into), find: false },
            None => return Vec::new(),
        },
        ("AskUserQuestion", _) => Op::ask(&input["questions"], output),
        ("Agent" | "Task", _) => Op::Agent {
            agent: text(input, "subagent_type").map(Into::into),
            task: text(input, "description").unwrap_or("子任务").into(),
            background: input["run_in_background"] == true || outcome["isAsync"] == true,
            encrypted: false,
        },
        ("SubagentHandback", _) => Op::Message {
            to: "主任务".into(),
            summary: text(input, "message").map(first_line),
            encrypted: false,
        },
        ("ListAgents", _) => Op::AgentList,
        ("SendMessage", _) => match text(input, "to") {
            Some(to) => Op::Message { to: to.into(), summary: text(input, "summary").or_else(|| text(input, "message")).map(first_line), encrypted: false },
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
                detail: output.and_then(|output| {
                    serde_json::from_str::<Value>(output).ok()
                        .and_then(|value| text(&value, "command").map(first_line))
                        .or_else(|| output.split_once(" (").map(|(_, rest)| rest.trim_end_matches(['}', '"', ')']).into()))
                }),
            },
            None => return Vec::new(),
        },
        ("Monitor", _) => Op::Monitor {
            description: text(input, "description").unwrap_or("后台事件").into(),
            minutes: input["timeout_ms"].as_u64().map(|ms| ms.div_ceil(60_000)),
        },
        ("ScheduleWakeup", _) if input["stop"] == true => Op::Stop { task: "定时唤醒".into(), detail: None },
        ("ScheduleWakeup", _) => match input["delaySeconds"].as_f64() {
            Some(seconds) => Op::Wake { seconds, reason: text(input, "reason").map(Into::into) },
            None => return Vec::new(),
        },
        ("TodoWrite", _) => Op::todo(&input["todos"], "content"),
        _ => match name.strip_prefix("mcp__").and_then(|rest| rest.rsplit_once("__")) {
            Some((server, tool)) => Op::mcp(server, tool, input),
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

/// 过长的输出存进文件，结果里只留预览：`<persisted-output>`、`Full output saved to: 路径`、`Preview (…):` 和预览正文。
fn persisted(text: &str) -> Option<(&str, &str)> {
    let body = text.trim().strip_prefix("<persisted-output>")?.strip_suffix("</persisted-output>")?;
    let file = body.split_once("Full output saved to: ")?.1.lines().next()?.trim();
    let preview = body.split_once("\nPreview (").and_then(|(_, rest)| rest.split_once("):\n")).map_or("", |(_, preview)| preview);
    Some((file, preview.trim_end()))
}

/// Claude Code 在命令输出末尾追加的说明（重置工作目录、提醒命令改过已读文件），以及没有输出时的占位文字。
fn command_text(text: &str) -> &str {
    if text.trim() == "(Bash completed with no output)" {
        return "";
    }
    let mut text = text.trim_end();
    loop {
        let (rest, line) = text.rsplit_once('\n').unwrap_or(("", text));
        let line = line.trim();
        if !(line.strip_prefix("Shell cwd was reset to ").is_some_and(|path| path.starts_with(['/', '~']) || path.as_bytes().get(1) == Some(&b':'))
            || line.starts_with("[This command modified ") && line.ends_with(']')) {
            return text;
        }
        text = rest.trim_end();
    }
}

/// 失败的命令结果首行是 `Exit code N`，其后才是命令自己的输出。
fn exit_status(output: Option<&str>) -> (Option<i64>, Option<&str>) {
    let parsed = output.and_then(|output| output.strip_prefix("Exit code ")).and_then(|rest| {
        let (code, rest) = rest.split_once('\n').unwrap_or((rest, ""));
        Some((code.trim().parse().ok()?, rest))
    });
    match parsed {
        Some((code, rest)) => (Some(code), Some(rest)),
        None => (None, output),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bash_results_drop_harness_text_and_keep_the_saved_output_path() {
        let call = json!({"name":"Bash","input":{"command":"cargo test"}});
        let describe = |content: &str, failed: bool| {
            let mut block = json!({"content":content,"is_error":failed});
            let ops = describe_call(&call, Some(ToolResult { block: &mut block, event: &mut Value::Null }));
            (json!(ops[0]), block)
        };
        let (op, block) = describe("Exit code 101\nerror: test failed\nShell cwd was reset to /repo", true);
        assert_eq!((op["exit_code"].clone(), block["output"].clone()), (json!(101), json!("error: test failed")));
        assert_eq!(describe("(Bash completed with no output)", false).1["output"], "");
        let (_, block) = describe("<persisted-output>\nOutput too large (30KB). Full output saved to: /tmp/out.txt\n\nPreview (first 2KB):\nline 1\n</persisted-output>\nShell cwd was reset to /repo", false);
        assert_eq!((block["output_file"].clone(), block["output"].clone()), (json!("/tmp/out.txt"), json!("line 1")));
        assert!(describe("plain output", false).1.get("output").is_none());
        let mut block = json!({"content":"<persisted-output>\npreview\n</persisted-output>","is_error":false});
        let mut event = json!({"toolUseResult":{"stdout":"full output\n","stderr":"\nShell cwd was reset to /repo","persistedOutputPath":"/tmp/full.txt"}});
        describe_call(&call, Some(ToolResult { block: &mut block, event: &mut event }));
        assert_eq!((block["output"].clone(), block["output_file"].clone()), (json!("full output"), json!("/tmp/full.txt")));
        let mut literal = json!({"content":"Shell cwd was reset to /repo","is_error":false});
        let mut literal_event = json!({"toolUseResult":{"stdout":"Shell cwd was reset to /repo","stderr":""}});
        describe_call(&call, Some(ToolResult { block: &mut literal, event: &mut literal_event }));
        assert!(literal.get("output").is_none());
    }
}
