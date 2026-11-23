//! Codex 的工具卡片。`item_completed` 中的执行记录单独展示，确认由成功完成的 exec 脚本发起时折叠包装脚本。
use super::super::tool_ops::{self, first_line, text, Choice, Hunk, Op, Question};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet};

#[derive(Default)]
pub(super) struct Activity {
    pub calls: HashSet<String>,
    scripts: HashMap<String, ExecSpan>,
    executions: HashMap<String, BTreeMap<i64, Vec<(String, String)>>>,
}

struct ExecSpan {
    turn: String,
    start: i64,
    end: Option<i64>,
    succeeded: bool,
    kind: Option<&'static str>,
}

impl Activity {
    pub fn start_exec(&mut self, id: &str, turn: String, start: i64, script: &str) {
        let kind = wrapped_tool(script).and_then(|tool| match tool {
            "exec_command" => Some("CommandExecution"),
            "apply_patch" => Some("FileChange"),
            "view_image" => Some("ImageView"),
            "web__run" | "clock__sleep" => Some("Extension"),
            tool if tool.starts_with("mcp__") => Some("McpToolCall"),
            _ => None,
        });
        self.scripts.insert(id.into(), ExecSpan { turn, start, end: None, succeeded: false, kind });
    }

    pub fn finish_exec(&mut self, id: &str, end: i64, output: &serde_json::value::RawValue) {
        if let Some(span) = self.scripts.get_mut(id) {
            span.end = Some(end);
            span.succeeded = end >= span.start && super::tool_output::script_completed(output);
        }
    }

    pub fn record_execution(&mut self, turn: String, start: i64, item: &Value) {
        if let (Some(id), Some(kind)) = (item["id"].as_str(), item["type"].as_str()) {
            self.executions.entry(turn).or_default().entry(start).or_default().push((id.into(), kind.into()));
        }
    }

    /// `prefix` 是开头继承自父会话的事件数，随前缀内被折叠的事件一起减少。
    pub fn remove_redundant_scripts(&self, events: &mut Vec<Value>, prefix: &mut usize) {
        let mut by_turn: HashMap<&str, Vec<(&String, &ExecSpan)>> = HashMap::new();
        for (id, span) in &self.scripts {
            by_turn.entry(&span.turn).or_default().push((id, span));
        }
        let mut replaced = HashMap::new();
        for (turn, mut scripts) in by_turn {
            let Some(executions) = self.executions.get(turn) else { continue };
            scripts.sort_unstable_by_key(|(_, span)| span.start);
            let mut previous_end = None;
            for (index, (id, span)) in scripts.iter().enumerate() {
                let end = span.end.unwrap_or(i64::MAX).max(span.start);
                let overlaps = previous_end.is_some_and(|previous| previous >= span.start)
                    || scripts.get(index + 1).is_some_and(|(_, next)| next.start <= end);
                previous_end = Some(previous_end.map_or(end, |previous: i64| previous.max(end)));
                if span.succeeded && !overlaps {
                    let mut records = executions.range(span.start..=end).flat_map(|(_, records)| records);
                    if let (Some((execution, kind)), None) = (records.next(), records.next()) {
                        if span.kind == Some(kind.as_str()) {
                            replaced.insert(id.as_str(), execution);
                        }
                    }
                }
            }
        }
        if replaced.is_empty() {
            return;
        }
        let mut outputs = HashMap::new();
        let boundary = *prefix;
        let mut position = 0;
        events.retain_mut(|event| {
            let inherited = position < boundary;
            position += 1;
            let Some(block) = event.pointer_mut("/message/content/0") else { return true };
            let id = match block["type"].as_str() {
                Some("tool_use") => &block["id"],
                Some("tool_result") => &block["tool_use_id"],
                _ => return true,
            };
            let Some(execution) = id.as_str().and_then(|id| replaced.get(id)) else { return true };
            if block["type"] == "tool_result" {
                outputs.insert(execution.as_str(), block.take());
            }
            *prefix -= usize::from(inherited);
            false
        });
        for event in events {
            let Some(block) = event.pointer_mut("/message/content/0") else { continue };
            if block["type"] == "tool_use" {
                if let Some(output) = block["id"].as_str().and_then(|id| outputs.remove(id)) {
                    block["script_result"] = output;
                }
            }
        }
    }
}

/// 折叠仅限没有额外逻辑的单次调用。作为参数的代码字符串可以包含引号和括号。
fn wrapped_tool(script: &str) -> Option<&str> {
    let script = script.trim().trim_end_matches(';').trim_end();
    let call = script.strip_prefix("await ").or_else(|| script.strip_prefix("text(await ")?.strip_suffix(')'))?;
    let (name, args) = call.strip_prefix("tools.")?.split_once('(')?;
    let args = args.strip_suffix(')')?.trim();
    if !args.starts_with('{') {
        return None;
    }
    let mut braces = 0usize;
    let mut quote = None;
    let mut escaped = false;
    for (index, ch) in args.char_indices() {
        if let Some(delimiter) = quote {
            if !escaped && ch == delimiter {
                quote = None;
            }
            escaped = !escaped && ch == '\\';
            continue;
        }
        match ch {
            '\'' | '"' => quote = Some(ch),
            '{' => braces += 1,
            '}' => {
                braces -= 1;
                if braces == 0 {
                    return args[index + 1..].trim().is_empty().then_some(name.trim_end());
                }
            }
            '(' | ')' | '`' | ';' => return None,
            _ => {}
        }
    }
    None
}

#[derive(Deserialize)]
pub(super) struct ItemHeader<'a> {
    #[serde(rename = "type", borrow)]
    kind: &'a str,
    #[serde(borrow)]
    pub id: Option<&'a str>,
}

impl ItemHeader<'_> {
    pub fn is_tool(&self) -> bool {
        matches!(self.kind, "CommandExecution" | "FileChange" | "ImageView" | "McpToolCall" | "Extension")
    }
}

/// 调用与结果各自成块，顺序固定，延迟读取按下标取回；`view` 时只保留卡片标题所需字段。
pub(super) fn blocks(item: &Value, view: bool) -> Vec<Value> {
    let id = item["id"].as_str().unwrap_or_default();
    let kind = item["type"].as_str();
    let command_exit = (kind == Some("CommandExecution")).then(|| item["exit_code"].as_i64()).flatten();
    let status = match item["status"].as_str() {
        Some("completed") | None if command_exit.is_some_and(|exit| exit != 0) => "error",
        Some("completed") | None => "completed",
        Some("in_progress") => "running",
        Some("pending") => "pending",
        _ => "error",
    };
    let (name, mut ops, input, output) = match kind {
        Some("CommandExecution") => {
            let command = command_line(&item["command"]);
            let output = item["aggregated_output"].as_str().unwrap_or_default();
            let op = tool_ops::shell(&command, None, command_exit, Some(super::tool_output::execution_output(output)));
            ("exec_command", vec![op], json!({"cmd":command,"cwd":item["cwd"]}), Some(json!(output)))
        }
        Some("FileChange") => {
            let ops = item["changes"].as_object().into_iter().flatten().map(|(path, change)| {
                let path = change["move_path"].as_str().unwrap_or(path);
                match change["type"].as_str() {
                    Some("add") => Op::write(path, change["content"].as_str().unwrap_or_default(), Some(true)),
                    Some("delete") => Op::Delete { path: path.into() },
                    _ => Op::edit(path, Hunk::parse_unified(change["unified_diff"].as_str().unwrap_or_default())),
                }
            }).collect();
            ("apply_patch", ops, item["changes"].clone(), None)
        }
        Some("ImageView") => {
            let path = item["path"].as_str().unwrap_or_default();
            ("view_image", vec![Op::Image { path: path.strip_prefix("file://").unwrap_or(path).into() }], json!({"path":path}), None)
        }
        Some("McpToolCall") => {
            let arguments: Value = match &item["arguments"] {
                Value::String(raw) => serde_json::from_str(raw).unwrap_or(Value::Null),
                arguments => arguments.clone(),
            };
            let op = Op::Mcp {
                server: text(item, "server").unwrap_or("mcp").into(),
                tool: text(item, "tool").unwrap_or("?").into(),
                title: text(&arguments, "title").map(Into::into),
            };
            (text(item, "tool").unwrap_or("mcp"), vec![op], arguments, Some(mcp_result(item)))
        }
        Some("Extension") => match (item["kind"].as_str(), &item["action"]) {
            (Some("web.search"), action) => {
                let results = item["results"].as_array();
                let op = match action["type"].as_str() {
                    Some("openPage" | "findInPage") => Op::WebFetch {
                        url: text(action, "url").or_else(|| text(item, "query")).unwrap_or_default().into(),
                        prompt: text(action, "pattern").map(Into::into),
                    },
                    _ => Op::WebSearch {
                        query: text(action, "query").or_else(|| text(item, "query")).unwrap_or_default().into(),
                        domain: None,
                        results: results.map(Vec::len),
                    },
                };
                let output = results.map(|results| json!(results.iter().map(|result| format!("{}\n{}",
                    result["title"].as_str().unwrap_or_default(), result["url"].as_str().unwrap_or_default())).collect::<Vec<_>>().join("\n\n")));
                (if matches!(op, Op::WebFetch { .. }) { "open_page" } else { "web_search" }, vec![op], action.clone(), output)
            }
            (Some("clock.sleep"), _) => ("sleep", vec![Op::Wait { seconds: item["durationMs"].as_f64().map(|ms| ms / 1000.0), target: None }], Value::Null, None),
            (Some("image_gen.generation"), _) => {
                let path = text(item, "savedPath").unwrap_or("生成的图片");
                let image = item["result"].as_str().filter(|data| data.starts_with("iVBORw0KGgo"));
                let output = image.map(|data| {
                    if view { Value::Null } else {
                        json!([{
                            "type": "image", "source": {"type": "base64", "media_type": "image/png", "data": data}
                        }])
                    }
                }).or_else(|| item.get("failure").filter(|value| !value.is_null()).cloned());
                ("image_gen", vec![Op::Image { path: path.into() }], json!({"revisedPrompt":item["revisedPrompt"],"savedPath":path}), output)
            }
            (kind, _) => (kind.unwrap_or("extension"), Vec::new(), item["action"].clone(),
                item.get("result").filter(|result| !result.is_null()).map(|result| if view { Value::Null } else { result.clone() })),
        },
        _ => return Vec::new(),
    };
    let mut call = json!({"type":"tool_use","id":id,"name":name,"status":status});
    let mut result = output.map(|content| json!({"type":"tool_result","tool_use_id":id,"is_error":status == "error","content":content}));
    if let Some(result) = &result {
        tool_ops::promote_skill(&mut ops, result);
    }
    if let Some(result) = &mut result {
        let content = &result["content"];
        if status == "error" {
            if let Some(reason) = super::tool_output::error_reason(content) {
                result["error_summary"] = json!(reason);
            }
        }
        if !view && kind == Some("CommandExecution") {
            if let Some(output) = item["aggregated_output"].as_str() {
                let body = super::tool_output::execution_output(output);
                if body.len() < output.len() {
                    result["command_output"] = json!(body);
                }
            }
        }
    }
    if view {
        ops = ops.into_iter().map(Op::summary).collect();
        if let Some(result) = &mut result {
            result.as_object_mut().map(|result| result.remove("content"));
        }
    } else {
        call["input"] = input;
    }
    call["ops"] = json!(ops);
    std::iter::once(call).chain(result).collect()
}

/// 函数调用形式的工具；一个补丁可改多个文件，每个文件一个操作。
pub(super) fn describe(name: &str, input: &Value) -> Vec<Op> {
    if name == "apply_patch" {
        return input.as_str().or_else(|| text(input, "input")).map(patch_ops).unwrap_or_default();
    }
    describe_one(name, input).into_iter().collect()
}

fn describe_one(name: &str, input: &Value) -> Option<Op> {
    Some(match name {
        "shell" | "container.exec" | "exec_command" => {
            let command = ["command", "cmd"].iter().map(|key| command_line(&input[*key])).find(|command| !command.trim().is_empty())?;
            tool_ops::shell(&command, None, None, None)
        }
        "exec" => {
            let script = input.as_str()?;
            exec_script(script)
        }
        "js" => Op::script("JavaScript", text(input, "title"), text(input, "code")?),
        "wait" => Op::Wait { seconds: input["yield_time_ms"].as_f64().map(|ms| ms / 1000.0), target: None },
        "sleep" => Op::Wait { seconds: input["duration_ms"].as_f64().map(|ms| ms / 1000.0), target: None },
        "wait_agent" => Op::Wait { seconds: input["timeout_ms"].as_f64().map(|ms| ms / 1000.0), target: Some("子智能体".into()) },
        "list_agents" => Op::AgentList,
        "spawn_agent" => Op::Agent {
            agent: None,
            task: text(input, "task_name").unwrap_or("子任务").into(),
            background: true,
            encrypted: text(input, "message").is_some_and(is_fernet_token),
        },
        "send_message" => {
            let message = text(input, "message");
            let encrypted = message.is_some_and(is_fernet_token);
            Op::Message { to: text(input, "target")?.into(), summary: message.filter(|_| !encrypted).map(first_line), encrypted }
        }
        "send_message_to_user_async" => Op::Message { to: "用户".into(), summary: text(input, "message").map(first_line), encrypted: false },
        "request_user_input" | "request_user_input_async" => Op::ask(input["questions"].as_array()?.iter().filter_map(|q| Some(Question {
            question: text(q, "title").or_else(|| text(q, "question"))?.into(),
            header: None,
            options: q["options"].as_array().into_iter().flatten()
                .filter_map(|o| Some(Choice { label: o.as_str().or_else(|| text(o, "label"))?.into(), description: None })).collect(),
            answer: None,
        })).collect(), None),
        "update_plan" => Op::todo(&input["plan"], "step"),
        "view_image" => Op::Image { path: text(input, "path")?.into() },
        "search_openai_docs" => Op::web_search(text(input, "query")?, None, None),
        "fetch_openai_doc" => Op::WebFetch { url: text(input, "url")?.into(), prompt: None },
        _ if name.starts_with("mcp__") => {
            let (server, tool) = name.strip_prefix("mcp__")?.split_once("__")?;
            Op::Mcp { server: server.into(), tool: tool.into(), title: text(input, "title").map(Into::into) }
        }
        _ => return None,
    })
}

/// 仅提取字面量工具调用的标题，完整 JavaScript 始终可查看。
fn exec_script(script: &str) -> Op {
    let title = if script.matches("tools.").count() == 1 && script.contains("tools.clock__curr_time(") {
        Some("查看当前时间".into())
    } else if script.matches("tools.").count() == 1 && script.contains("tools.write_stdin(") {
        let args = call_args(script, "write_stdin");
        let args = args.and_then(|args| args.split_once("})").map(|(args, _)| args));
        let fields = |name: &str| args.and_then(|args| args.split(',').find_map(|part| {
            let (key, value) = part.split_once(':')?;
            (key.trim() == name).then(|| value.trim())
        }));
        let action = match fields("chars") {
            Some("\"\"" | "''") => "读取后台输出",
            Some(_) => "发送会话输入",
            None => "访问后台会话",
        };
        Some(match fields("session_id").and_then(|id| id.parse::<u64>().ok()) {
            Some(id) => format!("{action} · 会话 {id}"),
            None => action.into(),
        })
    } else {
        call_args(script, "exec_command").and_then(|args| {
            let (key, value) = args.split_once(':')?;
            if !matches!(key.trim(), "cmd" | "\"cmd\"" | "'cmd'") {
                return None;
            }
            let command = js_string(value.trim_start())?;
            let head = first_line(&command);
            (!head.is_empty()).then_some(head)
        })
    };
    Op::script("JavaScript", title.as_deref(), script)
}

fn call_args<'a>(script: &'a str, tool: &str) -> Option<&'a str> {
    script.split_once(&format!("tools.{tool}("))?.1.trim_start().strip_prefix('{')
}

fn js_string(value: &str) -> Option<String> {
    let quote = value.chars().next().filter(|c| matches!(c, '\'' | '"'))?;
    let mut escaped = false;
    for (offset, ch) in value.char_indices().skip(1) {
        if !escaped && ch == quote {
            if !value[offset + 1..].trim_start().starts_with([',', '}']) {
                return None;
            }
            let literal = &value[..=offset];
            return if quote == '"' { serde_json::from_str(literal).ok() }
            else if !literal.contains('\\') { Some(literal[1..offset].into()) }
            else { None };
        }
        escaped = !escaped && ch == '\\';
    }
    None
}

/// `*** Begin Patch` 格式的补丁：每个文件一个修改、新建或删除操作；`*** Move to:` 改用新路径。
fn patch_ops(patch: &str) -> Vec<Op> {
    let Some(patch) = patch.trim_end().strip_prefix("*** Begin Patch\n").and_then(|body| body.strip_suffix("*** End Patch")) else {
        return Vec::new();
    };
    enum File {
        Update(String, Vec<Hunk>),
        Add(String, Vec<String>),
        Delete(String),
    }
    let mut files: Vec<File> = Vec::new();
    for line in patch.lines() {
        if let Some(path) = line.strip_prefix("*** Update File: ") {
            files.push(File::Update(path.trim().into(), Vec::new()));
        } else if let Some(path) = line.strip_prefix("*** Add File: ") {
            files.push(File::Add(path.trim().into(), Vec::new()));
        } else if let Some(path) = line.strip_prefix("*** Delete File: ") {
            files.push(File::Delete(path.trim().into()));
        } else if let Some(path) = line.strip_prefix("*** Move to: ") {
            if let Some(File::Update(target, _)) = files.last_mut() {
                *target = path.trim().into();
            }
        } else {
            match files.last_mut() {
                Some(File::Update(_, hunks)) if line.starts_with("@@") || hunks.is_empty() => {
                    hunks.push(Hunk { old_start: None, new_start: None, lines: Vec::new() });
                    if !line.starts_with("@@") && line.starts_with([' ', '+', '-']) {
                        if let Some(hunk) = hunks.last_mut() { hunk.lines.push(line.into()) }
                    }
                }
                Some(File::Update(_, hunks)) if line.starts_with([' ', '+', '-']) => {
                    if let Some(hunk) = hunks.last_mut() { hunk.lines.push(line.into()) }
                }
                Some(File::Add(_, lines)) if line.starts_with('+') => lines.push(line[1..].into()),
                _ => {}
            }
        }
    }
    files.into_iter().map(|file| match file {
        File::Update(path, hunks) => Op::edit(&path, hunks),
        File::Add(path, lines) => Op::write(&path, &lines.join("\n"), Some(true)),
        File::Delete(path) => Op::Delete { path },
    }).collect()
}

/// 多智能体 V2 的智能体间消息在日志里只有 Fernet 令牌，本地无法解密。令牌是规范的 URL 安全 Base64，
/// 解码后依次为版本字节 0x80、时间戳、IV、整块密文与 HMAC，即 57 字节加上 16 字节的整数倍。
fn is_fernet_token(text: &str) -> bool {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let body = text.trim_end_matches('=');
    let padding = text.len() - body.len();
    let bytes = body.len() * 3 / 4;
    text.len().is_multiple_of(4)
        && padding <= 2
        && body.starts_with("gAAAAA")
        && body.bytes().all(|byte| ALPHABET.contains(&byte))
        // 补位前最后一个字符的空余位必须为零。
        && ALPHABET.iter().position(|&byte| Some(byte) == body.bytes().last())
            .is_some_and(|value| value & ((1 << (padding * 2)) - 1) == 0)
        && bytes >= 73
        && (bytes - 57).is_multiple_of(16)
}

/// MCP 结果的文本和图片；没有内容时取错误原因。
fn mcp_result(item: &Value) -> Value {
    let parts: Vec<_> = item["result"]["content"].as_array().into_iter().flatten().filter_map(|part| match part["type"].as_str() {
        Some("text") => Some(json!({"type":"text","text":part["text"]})),
        Some("image") => Some(json!({"type":"image","source":{"type":"base64","media_type":part["mimeType"],"data":part["data"]}})),
        _ => None,
    }).collect();
    if !parts.is_empty() {
        return json!(parts);
    }
    json!(text(item, "error").or_else(|| text(&item["error"], "message")).unwrap_or_default())
}

/// `["/bin/zsh", "-lc", "cmd"]` 取脚本本身；其余参数按 shell 规则拼接。
fn command_line(command: &Value) -> String {
    if let Some(command) = command.as_str() {
        return command.into();
    }
    let args: Vec<_> = command.as_array().into_iter().flatten().filter_map(Value::as_str).collect();
    match args.as_slice() {
        [_, "-lc" | "-c", script] => (*script).to_owned(),
        _ => args.iter().map(|arg| {
            if !arg.is_empty() && arg.bytes().all(|b| b.is_ascii_alphanumeric() || b"/._-=:,@%+".contains(&b)) {
                (*arg).to_owned()
            } else {
                format!("'{}'", arg.replace('\'', r"'\''"))
            }
        }).collect::<Vec<_>>().join(" "),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exec_summary_keeps_the_script_and_labels_literal_calls() {
        let script = r#"text(await tools.write_stdin({ session_id: 42, chars: "", yield_time_ms: 1000 }));"#;
        let op = json!(exec_script(script));
        assert_eq!(op["title"], "读取后台输出 · 会话 42");
        assert_eq!(op["body"], script);
        let script = r#"text(await tools.exec_command({cmd: "git status\ngit diff"}));"#;
        assert_eq!(json!(exec_script(script))["title"], "git status");
        assert!(json!(exec_script(r#"text(await tools.exec_command({cmd: "echo " + value}));"#)).get("title").is_none());
    }

    #[test]
    fn generated_images_and_unknown_extensions_keep_their_deferred_output_positions() {
        let image = json!({"id":"image", "type":"Extension", "kind":"image_gen.generation", "savedPath":"/tmp/fixture.png", "result":"iVBORw0KGgo="});
        let summary = blocks(&image, true);
        let full = blocks(&image, false);
        assert_eq!(summary.len(), full.len());
        assert_eq!(summary[0]["ops"][0]["kind"], "image");
        assert!(summary[1].get("content").is_none());
        assert_eq!(full[1]["content"][0]["source"]["data"], "iVBORw0KGgo=");

        let item = json!({"id":"future", "type":"Extension", "kind":"example.future", "action":{"target":"fixture"}, "result":"done"});
        let full = blocks(&item, false);
        assert_eq!(full[0]["name"], "example.future");
        assert_eq!(full[0]["input"]["target"], "fixture");
        assert_eq!(full[1]["content"], "done");
    }
}
