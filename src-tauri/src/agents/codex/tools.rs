//! Codex 的工具卡片：函数调用、`exec` 脚本里的调用，以及 `item_completed` 中的执行记录。
use super::super::tool_ops::{self, first_line, text, Hunk, Op, TerminalAction};
use serde::Deserialize;
use serde_json::{json, Value};

pub(super) fn image_path(path: &str) -> String {
    path.strip_prefix("file://").unwrap_or(path).into()
}

pub(super) fn parse_mcp_args(arguments: &Value) -> Value {
    match arguments {
        Value::String(raw) => serde_json::from_str(raw).unwrap_or(Value::Null),
        other => other.clone(),
    }
}

pub(super) fn file_change_ops(changes: &Value) -> Vec<Op> {
    changes.as_object().into_iter().flatten().map(|(path, change)| {
        let path = text(change, "move_path").unwrap_or(path);
        match change["type"].as_str() {
            Some("add") => Op::write(path, change["content"].as_str().unwrap_or_default(), Some(true)),
            Some("delete") => Op::Delete { path: path.into() },
            _ => Op::edit(path, Hunk::parse_unified(change["unified_diff"].as_str().unwrap_or_default())),
        }
    }).collect()
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
    let recorded_status = item["status"].as_str();
    let command_exit = (kind == Some("CommandExecution")).then(|| item["exit_code"].as_i64()).flatten();
    let status = match recorded_status {
        Some("completed") | None if command_exit.is_some_and(|exit| exit != 0) => "error",
        Some("completed") | None => "completed",
        Some("in_progress") => "running",
        Some("pending") => "pending",
        _ => "error",
    };
    let (name, mut ops, input, output) = match kind {
        Some("CommandExecution") => {
            let command = command_line(&item["command"]);
            let output = item["aggregated_output"].as_str();
            let body = output.map(super::tool_output::execution_output);
            let concluded = matches!(recorded_status, None | Some("completed" | "failed"));
            let mut ops = vec![tool_ops::shell(&command, None, command_exit, body.filter(|_| concluded))];
            if status != "error" {
                if let Some(body) = body {
                    tool_ops::promote_skill_text(&mut ops, body);
                }
            }
            ("exec_command", ops, json!({"cmd":command,"cwd":item["cwd"]}), Some(json!(output.unwrap_or_default())))
        }
        Some("FileChange") => ("apply_patch", file_change_ops(&item["changes"]), item["changes"].clone(), None),
        Some("ImageView") => {
            let path = item["path"].as_str().unwrap_or_default();
            ("view_image", vec![Op::Image { path: image_path(path) }], json!({"path":path}), None)
        }
        Some("McpToolCall") => {
            let arguments = parse_mcp_args(&item["arguments"]);
            let server = text(item, "server").unwrap_or("mcp");
            let tool = text(item, "tool").unwrap_or("?");
            (tool, vec![mcp_operation(server, tool, &arguments)], arguments, Some(mcp_result(item)))
        }
        Some("Extension") => match (item["kind"].as_str(), &item["action"]) {
            (Some("web.search"), action) => {
                let results = item["results"].as_array();
                let (name, op) = web_action(action, text(item, "query"), results.map(Vec::len));
                let output = results.map(|results| if view { Value::Null } else { json!(results) });
                (name, op.into_iter().collect(), action.clone(), output)
            }
            (Some("clock.sleep"), _) => ("sleep", vec![Op::Wait { seconds: item["durationMs"].as_f64().map(|ms| ms / 1000.0), target: None }], Value::Null, None),
            (Some("image_gen.generation"), _) => {
                let path = text(item, "savedPath");
                let image = item["result"].as_str().filter(|data| data.starts_with("iVBORw0KGgo"));
                let output = image.map(|data| {
                    if view { Value::Null } else {
                        json!([{
                            "type": "image", "source": {"type": "base64", "media_type": "image/png", "data": data}
                        }])
                    }
                }).or_else(|| item.get("failure").filter(|value| !value.is_null()).cloned());
                ("image_gen", vec![Op::GenerateImage { path: path.map(Into::into) }],
                    json!({"revisedPrompt":item["revisedPrompt"],"savedPath":path}), output)
            }
            (kind, _) => (kind.unwrap_or("extension"), Vec::new(), item["action"].clone(),
                item.get("result").filter(|result| !result.is_null()).map(|result| if view { Value::Null } else { result.clone() })),
        },
        _ => return Vec::new(),
    };
    let mut call = json!({"type":"tool_use","id":id,"name":name,"status":status});
    let mut result = output.map(|content| json!({"type":"tool_result","tool_use_id":id,"is_error":status == "error","content":content}));
    if let Some(result) = &mut result {
        if status == "error" {
            if let Some(reason) = super::tool_output::error_reason(&result["content"]) {
                result["error_summary"] = json!(reason);
            }
        }
        if view {
            result.as_object_mut().map(|result| result.remove("content"));
        } else if kind == Some("CommandExecution") {
            if let Some(output) = item["aggregated_output"].as_str() {
                let body = super::tool_output::execution_output(output);
                if body.len() < output.len() {
                    result["output"] = json!(body);
                }
            }
        }
    }
    if view {
        ops = ops.into_iter().map(Op::summary).collect();
    } else {
        call["input"] = input;
    }
    call["ops"] = json!(ops);
    std::iter::once(call).chain(result).collect()
}

pub(super) fn web_action(action: &Value, fallback: Option<&str>, results: Option<usize>) -> (&'static str, Option<Op>) {
    match action["type"].as_str() {
        Some("openPage" | "open_page") => ("open_page", Some(Op::WebFetch {
            url: text(action, "url").or(fallback).unwrap_or_default().into(), prompt: None, find: false,
        })),
        Some("findInPage" | "find_in_page") => ("find_in_page", Some(Op::WebFetch {
            url: text(action, "url").unwrap_or_default().into(), prompt: text(action, "pattern").map(Into::into), find: true,
        })),
        Some("search") | None => ("web_search", Some(Op::web_search(action, fallback, results))),
        _ => ("web", None),
    }
}

/// 函数调用形式的工具；一个补丁可改多个文件，每个文件一个操作。
pub(super) fn describe(name: &str, input: &Value) -> Vec<Op> {
    match name {
        "apply_patch" => input.as_str().or_else(|| text(input, "input")).map(patch_ops).unwrap_or_default(),
        "web__run" => web_run(input),
        _ => describe_one(name, input).into_iter().collect(),
    }
}

/// exec 里的网页工具：搜索合成一次网页搜索，打开和页内查找各算一次读取。
fn web_run(input: &Value) -> Vec<Op> {
    let entries = |key: &str| input[key].as_array().into_iter().flatten();
    let queries: Vec<_> = ["search_query", "image_query", "literature_search_query"].into_iter()
        .flat_map(entries).filter_map(|entry| text(entry, "q")).collect();
    let mut ops: Vec<_> = (!queries.is_empty()).then(|| Op::web_search(&json!({"queries":queries}), None, None)).into_iter().collect();
    for (key, prompt, find) in [("open", None, false), ("find", Some("pattern"), true)] {
        ops.extend(entries(key).filter_map(|entry| Some(Op::WebFetch {
            url: text(entry, "ref_id")?.into(),
            prompt: prompt.and_then(|key| text(entry, key)).map(Into::into),
            find,
        })));
    }
    ops
}

fn javascript(input: &Value) -> Option<Op> {
    if let Some(code) = text(input, "code") {
        return Some(Op::script("JavaScript", text(input, "title"), code));
    }
    Some(Op::Report { title: "完成电脑操作", summary: Some(text(input, "completion_summary")?.into()) })
}

fn docs_operation(tool: &str, input: &Value) -> Option<Op> {
    match tool {
        "search_openai_docs" => Some(Op::web_search(input, None, None)),
        "fetch_openai_doc" => text(input, "url").map(|url| Op::WebFetch { url: url.into(), prompt: None, find: false }),
        _ => None,
    }
}

fn mcp_operation(server: &str, tool: &str, input: &Value) -> Op {
    let semantic = match (server, tool) {
        ("node_repl" | "cua_repl", "js") => javascript(input),
        ("openaiDeveloperDocs", tool) => docs_operation(tool, input),
        _ => None,
    };
    semantic.unwrap_or_else(|| Op::mcp(server, tool, input))
}

fn describe_one(name: &str, input: &Value) -> Option<Op> {
    Some(match name {
        "shell" | "container.exec" | "exec_command" => {
            let command = ["command", "cmd"].iter().map(|key| command_line(&input[*key])).find(|command| !command.trim().is_empty())?;
            tool_ops::shell(&command, text(input, "justification"), None, None)
        }
        "exec" => super::exec::describe(input.as_str()?),
        "js" => javascript(input)?,
        "write_stdin" => Op::TerminalInput {
            session: input["session_id"].as_u64()?,
            action: match input["chars"].as_str()? {
                "" => TerminalAction::Poll,
                "\u{3}" => TerminalAction::Interrupt,
                _ => TerminalAction::Send,
            },
        },
        "wait" => Op::Wait { seconds: input["yield_time_ms"].as_f64().map(|ms| ms / 1000.0), target: text(input, "cell_id").map(Into::into) },
        "sleep" | "clock__sleep" => Op::Wait {
            seconds: input["duration_ms"].as_f64().map(|ms| ms / 1000.0).or_else(|| input["seconds"].as_f64()),
            target: None,
        },
        "wait_agent" => Op::Wait { seconds: input["timeout_ms"].as_f64().map(|ms| ms / 1000.0), target: Some("子智能体".into()) },
        "list_agents" => Op::AgentList,
        "spawn_agent" => Op::Agent {
            agent: None,
            task: text(input, "task_name").unwrap_or("子任务").into(),
            background: true,
            encrypted: text(input, "message").is_some_and(is_fernet_token),
        },
        "send_message" | "followup_task" => {
            let message = text(input, "message");
            let encrypted = message.is_some_and(is_fernet_token);
            Op::Message { to: text(input, "target")?.into(), summary: message.filter(|_| !encrypted).map(first_line), encrypted }
        }
        "send_message_to_user_async" => Op::Message { to: "用户".into(), summary: text(input, "message").map(first_line), encrypted: false },
        "interrupt_agent" | "close_agent" => Op::Stop {
            task: text(input, "target").or_else(|| text(input, "id")).unwrap_or("子智能体").into(),
            detail: None,
        },
        "request_user_input" | "request_user_input_async" => Op::ask(&input["questions"], None),
        "update_plan" => Op::todo(&input["plan"], "step"),
        "view_image" => Op::Image { path: image_path(text(input, "path")?) },
        "image_gen__imagegen" => Op::GenerateImage { path: None },
        "search_openai_docs" | "fetch_openai_doc" => docs_operation(name, input)?,
        _ if name.starts_with("mcp__") => {
            let (server, tool) = name.strip_prefix("mcp__")?.split_once("__")?;
            mcp_operation(server, tool, input)
        }
        _ => return None,
    })
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
pub(super) fn command_line(command: &Value) -> String {
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
    fn web_search_uses_recorded_queries_and_keeps_other_web_actions_distinct() {
        let action = json!({"type":"search","query":null,"queries":["first","second","first"]});
        let item = json!({"id":"search","type":"Extension","kind":"web.search","status":"completed",
            "action":action,"query":"display summary","results":[{"title":"Result","url":"https://example.com"}]});
        let blocks = blocks(&item, false);
        assert_eq!(blocks[0]["input"], action);
        assert_eq!(blocks[0]["ops"][0], json!({"kind":"web_search","queries":["first","second"],"results":1}));

        let (name, op) = web_action(&json!({"type":"find_in_page","pattern":"needle","url":null}), None, None);
        assert_eq!(name, "find_in_page");
        assert_eq!(json!(op), json!({"kind":"web_fetch","url":"","prompt":"needle","find":true}));
        assert!(web_action(&json!({"type":"other"}), None, None).1.is_none());

        let script = r#"text(await tools.web__run({search_query:[{q:"first",domains:["example.com"],recency:7},{q:"second"}],open:[{ref_id:"ref"}]}));"#;
        let op = json!(super::super::exec::describe(script));
        assert_eq!(op["calls"][0]["ops"], json!([{"kind":"web_search","queries":["first","second"]},{"kind":"web_fetch","url":"ref"}]));
        assert_eq!(op["calls"][0]["input"]["search_query"][0]["domains"], json!(["example.com"]));
    }

    #[test]
    fn generated_images_and_unknown_extensions_keep_their_deferred_output_positions() {
        let image = json!({"id":"image", "type":"Extension", "kind":"image_gen.generation", "savedPath":"/tmp/fixture.png", "result":"iVBORw0KGgo="});
        let summary = blocks(&image, true);
        let full = blocks(&image, false);
        assert_eq!(summary.len(), full.len());
        assert_eq!(summary[0]["ops"][0]["kind"], "generate_image");
        assert!(summary[1].get("content").is_none());
        assert_eq!(full[1]["content"][0]["source"]["data"], "iVBORw0KGgo=");

        let args = json!({"title":"Probe state","code":"console.log(1)"});
        let native_js = blocks(&json!({"id":"js","type":"McpToolCall","server":"node_repl","tool":"js","arguments":args}), false);
        assert_eq!(native_js[0]["ops"], json!(describe("mcp__node_repl__js", &args)));
        assert_eq!(native_js[0]["ops"][0]["kind"], "script");
        assert_eq!(json!(describe("interrupt_agent", &json!({"target":"worker"})))[0]["task"], "worker");
        assert_eq!(json!(describe("followup_task", &json!({"target":"worker","message":"continue"})))[0]["summary"], "continue");

        let item = json!({"id":"future", "type":"Extension", "kind":"example.future", "action":{"target":"fixture"}, "result":"done"});
        let full = blocks(&item, false);
        assert_eq!(full[0]["name"], "example.future");
        assert_eq!(full[0]["input"]["target"], "fixture");
        assert_eq!(full[1]["content"], "done");
    }
}
