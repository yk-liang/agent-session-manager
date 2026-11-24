use super::super::tool_ops::{self, first_line, line_count, text, Hunk, Op, ToolResult};
use serde_json::{json, Value};

/// 结果事件上的 `details` 由转换时保留，带有补丁；用完即移除。
pub(super) fn describe(call: &Value, result: Option<ToolResult<'_>>) -> Vec<Op> {
    let mut result = result;
    let details = result.as_mut()
        .and_then(|result| result.event.as_object_mut()?.remove("details"))
        .unwrap_or(Value::Null);
    let failed = result.as_ref().is_some_and(ToolResult::failed);
    let output = result.as_ref().and_then(ToolResult::text).map(str::to_owned);
    let output = output.as_deref();
    let input = &call["input"];
    let path = text(input, "path");
    let paths = || path.into_iter().map(Into::into).collect::<Vec<_>>();
    let op = match (call["name"].as_str(), path) {
        (Some("read"), Some(path)) => match result {
            Some(result) if result.block["content"].as_array().is_some_and(|parts| parts.iter().any(|part| part["type"] == "image")) => {
                Op::Image { path: path.into() }
            }
            Some(result) if !result.failed() => read(path, input, result.block),
            _ => Op::read(path),
        },
        (Some("edit"), Some(path)) => {
            let hunks = details["patch"].as_str().map(Hunk::parse_unified).filter(|h| !h.is_empty())
                .unwrap_or_else(|| Hunk::replacements(input, "oldText", "newText"));
            Op::edit(path, hunks)
        }
        (Some("write"), Some(path)) => Op::write(path, input["content"].as_str().unwrap_or_default(), None),
        (Some("grep"), _) => match text(input, "pattern") {
            Some(pattern) => tool_ops::search(pattern, paths(), false, output.filter(|_| !failed)),
            None => return Vec::new(),
        },
        (Some("find"), _) => tool_ops::find(text(input, "pattern"), paths(), output.filter(|_| !failed)),
        (Some("ls"), _) => tool_ops::list(paths(), output.filter(|_| !failed)),
        (Some("bash"), _) => match text(input, "command") {
            Some(command) => {
                // 失败的输出末尾记着退出码；没有输出时是 `(no output)`。
                let (code, body) = match output {
                    Some(text) if failed => text.trim_end_matches('\n').rsplit_once("\n\nCommand exited with code ")
                        .and_then(|(body, status)| Some((Some(status.parse().ok()?), Some(body)))).unwrap_or((None, None)),
                    output => (None, output),
                };
                let body = body.map(|body| if body.trim() == "(no output)" { "" } else { body });
                if let (Some(body), Some(result)) = (body, &mut result) {
                    if Some(body) != output {
                        result.block["output"] = json!(body);
                    }
                }
                tool_ops::shell(command, None, code, body)
            }
            None => return Vec::new(),
        },
        // pi-subagents 扩展：`label` 是任务的短名。
        (Some("subagent"), _) if input["action"] == "guide" => Op::Report {
            title: "子智能体指南", summary: text(&input["options"], "topic").map(Into::into),
        },
        (Some("subagent"), _) => Op::Agent {
            agent: text(input, "agent").map(Into::into),
            task: text(input, "label").map(Into::into).or_else(|| text(input, "task").map(first_line)).unwrap_or_else(|| "子任务".into()),
            background: input["async"] == true,
            encrypted: false,
        },
        (Some("project_report"), _) => Op::Report { title: "项目报告", summary: None },
        (Some("lens_diagnostics"), _) => Op::Report { title: "项目诊断", summary: None },
        _ => return Vec::new(),
    };
    vec![op]
}

/// 去掉续读提示，只留文件正文；提示里给出的范围和总行数写入描述。
fn read(path: &str, input: &Value, block: &mut Value) -> Op {
    let fallback = || match (input["offset"].as_u64().filter(|&n| n > 0), input["limit"].as_u64().filter(|&n| n > 0)) {
        (Some(start), Some(limit)) => Op::read_range(path, start, limit, None),
        (Some(start), None) => Op::Read { path: path.into(), start: Some(start), end: None, total: None, tail: None },
        _ => Op::read(path),
    };
    let parts = block["content"].as_array().filter(|parts| parts.iter().all(|part| part["type"] == "text"));
    let (widened, output) = match parts.map(Vec::as_slice) {
        Some([only]) => (None, only["text"].as_str()),
        Some([note, body]) if note["text"].as_str().is_some_and(|text| text.starts_with("[pi-lens:")) =>
            (note["text"].as_str(), body["text"].as_str()),
        _ => return fallback(),
    };
    let Some(output) = output else { return fallback() };
    let widened_start = widened.and_then(|note| note.split_once("this shows lines "))
        .and_then(|(_, rest)| number(rest)).filter(|&n| n > 0);
    let start = widened_start.or_else(|| input["offset"].as_u64()).unwrap_or(1).max(1);
    let (content, notice) = match output.rsplit_once("\n\n[") {
        Some((content, notice)) if notice.ends_with(" to continue.]") => (content, Some(notice)),
        _ => (output, None),
    };
    if content.starts_with("[Line ") {
        return fallback();
    }
    let lines = line_count(content) as u64;
    let end = (start - 1).checked_add(lines);
    let total = match notice {
        Some(notice) => notice.split_once(" of ").and_then(|(_, rest)| number(rest))
            .or_else(|| notice.split_once(" more lines").and_then(|(more, _)| more.parse().ok())
                .and_then(|more| end?.checked_add(more))),
        None => end,
    };
    let op = Op::read_range(path, start, lines, total);
    if widened.is_some() || content != output {
        block["output"] = json!(content);
    }
    op
}

fn number(text: &str) -> Option<u64> {
    text.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()
}
