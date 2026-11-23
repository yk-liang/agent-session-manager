use super::super::tool_ops::{self, line_count, text, Hunk, Op, ToolResult};
use serde_json::{json, Value};

/// 结果事件上的 `details` 由转换时保留，带有补丁；用完即移除。
pub(super) fn describe(call: &Value, result: Option<ToolResult<'_>>) -> Vec<Op> {
    let mut result = result;
    let details = result.as_mut()
        .and_then(|result| result.event.as_object_mut()?.remove("details"))
        .unwrap_or(Value::Null);
    let output = result.as_ref().filter(|result| !result.failed()).and_then(ToolResult::text).map(str::to_owned);
    let output = output.as_deref();
    let input = &call["input"];
    let path = text(input, "path");
    let paths = || path.into_iter().map(Into::into).collect::<Vec<_>>();
    let op = match (call["name"].as_str(), path) {
        (Some("read"), Some(path)) => match result {
            Some(result) if !result.failed() => read(path, input, result.block),
            _ => Op::read(path),
        },
        (Some("edit"), Some(path)) => {
            let hunks = details["patch"].as_str().map(Hunk::parse_unified).filter(|h| !h.is_empty())
                .unwrap_or_else(|| {
                    let edits = input["edits"].as_array().map_or_else(|| vec![input], |edits| edits.iter().collect());
                    edits.iter().map(|edit| Hunk::replacement(
                        edit["oldText"].as_str().unwrap_or_default(),
                        edit["newText"].as_str().unwrap_or_default(),
                        None,
                    )).collect()
                });
            Op::edit(path, hunks)
        }
        (Some("write"), Some(path)) => Op::write(path, input["content"].as_str().unwrap_or_default(), None),
        (Some("grep"), _) => match text(input, "pattern") {
            Some(pattern) => tool_ops::search(pattern, paths(), false, output),
            None => return Vec::new(),
        },
        (Some("find"), _) => tool_ops::find(text(input, "pattern"), paths(), output),
        (Some("ls"), _) => tool_ops::list(paths(), output),
        (Some("bash"), _) => match text(input, "command") {
            Some(command) => tool_ops::shell(command, None, None, output),
            None => return Vec::new(),
        },
        _ => return Vec::new(),
    };
    vec![op]
}

/// 去掉续读提示，只留文件正文；提示里给出的范围和总行数写入描述。
fn read(path: &str, input: &Value, block: &mut Value) -> Op {
    let Some(output) = block["content"].as_array()
        .filter(|parts| parts.len() == 1 && parts[0]["type"] == "text")
        .and_then(|parts| parts[0]["text"].as_str())
    else {
        return Op::read(path);
    };
    let start = input["offset"].as_u64().unwrap_or(1).max(1);
    let (content, notice) = match output.rsplit_once("\n\n[") {
        Some((content, notice)) if notice.ends_with(" to continue.]") => (content, Some(notice)),
        _ => (output, None),
    };
    if content.starts_with("[Line ") {
        return Op::read(path);
    }
    if start == 1 && notice.is_none() {
        if let Some(skill) = tool_ops::read_skill(path, content) {
            return skill;
        }
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
    block["content"] = json!(content);
    op
}

fn number(text: &str) -> Option<u64> {
    text.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()
}
