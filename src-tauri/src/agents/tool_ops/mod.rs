//! 工具调用的语义解析。各 adapter 只把自家工具映射成 `Op`；shell 命令统一由 `shell` 解析，读、搜、找、列由 `browse` 解析。
mod browse;
mod op;
mod shell;
mod skill;

pub(crate) use browse::{find, list, search};
pub(crate) use op::*;
pub(crate) use shell::{shell, CommandOutput};
pub(crate) use skill::{loaded as loaded_skill, promote_text as promote_skill_text};

use serde_json::{json, Value};
use std::collections::HashMap;

/// 末尾换行不算作新的一行。
pub(crate) fn line_count(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }
    text.strip_suffix('\n').unwrap_or(text).split('\n').count()
}

/// 解析 `行号: 内容`、`行号| 内容` 或 `行号<Tab>内容` 形式的读取结果，跳过开头的说明行；行号必须连续，
/// 其后只能是空行或 `(…)`、`[…]`、`<…>` 形式的说明，否则不是这种格式。
pub(crate) fn numbered(text: &str) -> Option<(u64, u64, String)> {
    fn parse(line: &str) -> Option<(u64, &str)> {
        let (number, rest) = line.split_once([':', '|', '\t'])?;
        Some((number.trim_start().parse().ok()?, rest.strip_prefix(' ').unwrap_or(rest)))
    }
    let mut lines = text.lines().skip_while(|line| parse(line).is_none()).peekable();
    let (start, _) = parse(lines.peek()?)?;
    let mut end = start.checked_sub(1)?;
    let mut content = Vec::new();
    while let Some((number, rest)) = lines.peek().and_then(|line| parse(line)).filter(|(number, _)| Some(*number) == end.checked_add(1)) {
        content.push(rest);
        end = number;
        lines.next();
    }
    lines.all(|line| line.trim().is_empty() || line.starts_with(['(', '[', '<'])).then(|| (start, end, content.join("\n")))
}

pub(crate) struct ToolResult<'a> {
    pub block: &'a mut Value,
    /// 结果所在事件；adapter 可在这里取走随结果记录的原始元数据。
    pub event: &'a mut Value,
}

pub(crate) fn block_text(block: &Value) -> Option<&str> {
    let content = &block["content"];
    content.as_str().or_else(|| content.as_array()
        .filter(|parts| parts.len() == 1 && parts[0]["type"] == "text")
        .and_then(|parts| parts[0]["text"].as_str()))
}

impl ToolResult<'_> {
    /// 结果正文；多段时取唯一的文本段。
    pub fn text(&self) -> Option<&str> {
        block_text(self.block)
    }

    pub fn failed(&self) -> bool {
        self.block["is_error"] == true
    }
}

/// 按日志顺序把结果配给最近一次同 ID 的调用（与前端一致），由 adapter 给出调用的操作描述。
pub(crate) fn describe_tools(
    events: &mut [Value],
    mut describe: impl FnMut(&Value, Option<ToolResult<'_>>) -> Vec<Op>,
) {
    let mut pending: HashMap<String, (usize, usize)> = HashMap::new();
    let mut pairs = Vec::new();
    for (i, event) in events.iter().enumerate() {
        let Some(blocks) = event.pointer("/message/content").and_then(Value::as_array) else {
            continue;
        };
        for (k, block) in blocks.iter().enumerate() {
            match block["type"].as_str() {
                Some("tool_use") => match block["id"].as_str() {
                    Some(id) => {
                        if let Some(previous) = pending.insert(id.into(), (i, k)) {
                            pairs.push((previous, None));
                        }
                    }
                    None => pairs.push(((i, k), None)),
                },
                Some("tool_result") => {
                    if let Some(call) = block["tool_use_id"].as_str().and_then(|id| pending.remove(id)) {
                        pairs.push((call, Some((i, k))));
                    }
                }
                _ => {}
            }
        }
    }
    pairs.extend(pending.into_values().map(|call| (call, None)));
    for ((i, k), result) in pairs {
        let call = events[i]["message"]["content"][k].take();
        let ops = match result {
            Some((j, l)) => {
                let mut block = events[j]["message"]["content"][l].take();
                let mut ops = describe(&call, Some(ToolResult { block: &mut block, event: &mut events[j] }));
                skill::promote(&mut ops, &block);
                events[j]["message"]["content"][l] = block;
                ops
            }
            None => describe(&call, None),
        };
        events[i]["message"]["content"][k] = call;
        if !ops.is_empty() {
            events[i]["message"]["content"][k]["ops"] = json!(ops);
        }
    }
}

/// 工具参数里的非空字符串。
pub(crate) fn text<'a>(input: &'a Value, key: &str) -> Option<&'a str> {
    input[key].as_str().filter(|value| !value.trim().is_empty())
}

/// 长文本的首行，用作标题里的摘要。
pub(crate) fn first_line(text: &str) -> String {
    text.trim_start().lines().next().unwrap_or_default().trim().into()
}

/// 失败输出的首个非空行，截断至 240 字符作为悬停提示。
pub(crate) fn error_summary(text: &str) -> Option<String> {
    text.lines().map(str::trim).find(|line| !line.is_empty()).map(|line| line.chars().take(240).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbered_output_drops_headers_and_trailers() {
        let text = "Showing lines 7-9 of 20\n7: fn a() {\n8: \n9:   b()\n[Output truncated. Continue reading with offset: 10]";
        assert_eq!(numbered(text), Some((7, 9, "fn a() {\n\n  b()".into())));
        assert_eq!(numbered("no numbers"), None);
        assert_eq!(numbered("1: first\nunnumbered\n3: third"), None);
        assert_eq!(numbered("0: zero"), None);
    }
}
