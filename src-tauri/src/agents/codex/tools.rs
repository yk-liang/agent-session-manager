//! Codex 的工具卡片。`item_completed` 中的执行记录单独展示，确认由成功完成的 exec 脚本发起时折叠包装脚本。
use super::super::tool_ops::{self, text, Choice, Hunk, Op, Question};
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
    kinds: Vec<&'static str>,
}

impl Activity {
    pub fn start_exec(&mut self, id: &str, turn: String, start: i64, script: &str) {
        let kinds = [
            ("CommandExecution", &["tools.exec_command(", "tools.write_stdin("][..]),
            ("FileChange", &["tools.apply_patch("]),
            ("ImageView", &["tools.view_image("]),
            ("McpToolCall", &["tools.mcp__"]),
            ("Extension", &["tools.web__run(", "tools.clock__sleep("]),
        ].into_iter().filter_map(|(kind, names)| names.iter().any(|name| script.contains(name)).then_some(kind)).collect();
        self.scripts.insert(id.into(), ExecSpan { turn, start, end: None, succeeded: false, kinds });
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

    pub fn remove_redundant_scripts(&self, events: &mut Vec<Value>) {
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
                    if let Some((execution, _)) = executions.range(span.start..=end).flat_map(|(_, records)| records)
                        .find(|(_, kind)| span.kinds.contains(&kind.as_str()))
                    {
                        replaced.insert(id.as_str(), execution);
                    }
                }
            }
        }
        if replaced.is_empty() {
            return;
        }
        let mut outputs = HashMap::new();
        events.retain_mut(|event| {
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
    let status = match item["status"].as_str() {
        Some("completed") | None => "completed",
        Some("in_progress") => "running",
        _ => "error",
    };
    let (name, mut ops, input, output) = match item["type"].as_str() {
        Some("CommandExecution") => {
            let command = command_line(&item["command"]);
            let output = item["aggregated_output"].as_str().unwrap_or_default();
            let op = tool_ops::shell(&command, None, item["exit_code"].as_i64(), Some(output));
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
            _ => return Vec::new(),
        },
        _ => return Vec::new(),
    };
    let mut call = json!({"type":"tool_use","id":id,"name":name,"status":status});
    let mut result = output.map(|content| json!({"type":"tool_result","tool_use_id":id,"is_error":status == "error","content":content}));
    if let Some(result) = &result {
        tool_ops::promote_skill(&mut ops, result);
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

/// 函数调用形式的工具。
pub(super) fn describe(name: &str, input: &Value) -> Option<Op> {
    Some(match name {
        "shell" | "container.exec" | "exec_command" => {
            let command = ["command", "cmd"].iter().map(|key| command_line(&input[*key])).find(|command| !command.trim().is_empty())?;
            tool_ops::shell(&command, None, None, None)
        }
        "exec" => Op::script("JavaScript", None, input.as_str()?),
        "js" => Op::script("JavaScript", text(input, "title"), text(input, "code")?),
        "wait" => Op::Wait { seconds: input["yield_time_ms"].as_f64().map(|ms| ms / 1000.0), target: text(input, "cell_id").map(Into::into) },
        "sleep" => Op::Wait { seconds: input["duration_ms"].as_f64().map(|ms| ms / 1000.0), target: None },
        "spawn_agent" => Op::Agent { agent: None, task: text(input, "task_name").unwrap_or("子任务").into(), background: true },
        "send_message" => Op::Message { to: text(input, "target")?.into(), summary: None },
        "send_message_to_user_async" => Op::Message { to: "用户".into(), summary: text(input, "message").map(|m| m.lines().next().unwrap_or(m).into()) },
        "request_user_input" | "request_user_input_async" => Op::ask(input["questions"].as_array()?.iter().filter_map(|q| Some(Question {
            question: text(q, "title").or_else(|| text(q, "question"))?.into(),
            header: None,
            options: q["options"].as_array().into_iter().flatten()
                .filter_map(|o| Some(Choice { label: o.as_str().or_else(|| text(o, "label"))?.into(), description: None })).collect(),
            answer: None,
        })).collect(), None),
        "update_plan" => Op::todo(&input["plan"], "step"),
        "view_image" => Op::Image { path: text(input, "path")?.into() },
        _ => return None,
    })
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
