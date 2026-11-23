use super::super::tool_ops::{self, numbered, text, Choice, Hunk, Op, Question, ToolResult};
use serde_json::{json, Value};

/// 读取结果带有时间、范围说明和行号前缀，正文替换为原文后由卡片按范围编号。
pub(super) fn describe(call: &Value, result: Option<ToolResult<'_>>) -> Vec<Op> {
    let mut result = result;
    let original = result.as_mut().map(|result| result.block["content"].take()).unwrap_or(Value::Null);
    let output = original.as_str().unwrap_or_default();
    let input = &call["input"];
    let mut content = None;
    let op = match (call["name"].as_str(), text(input, "Path")) {
        (Some("read_file"), Some(path)) => match numbered(output) {
            Some((start, end, text)) if result.is_some() => {
                let total = output.lines().find_map(|line| line.strip_prefix("Showing lines "))
                    .and_then(|range| range.rsplit_once(" of ")?.1.trim().parse().ok());
                content = Some(json!(text));
                Some(Op::read_range(path, start, end - start + 1, total))
            }
            _ => Some(Op::read(path)),
        },
        (Some("edit_file"), Some(path)) => {
            let start = output.split_once(" starting at line ")
                .and_then(|(_, rest)| rest.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok());
            Some(Op::edit(path, vec![Hunk::replacement(
                input["Target"].as_str().unwrap_or_default(),
                input["Replacement"].as_str().unwrap_or_default(),
                start,
            )]))
        }
        (Some("write_file"), Some(path)) => Some(Op::write(path, input["Content"].as_str().unwrap_or_default(),
            output.lines().any(|line| line == "Created file.").then_some(true))),
        (Some("run_command"), _) => text(input, "CommandLine")
            .map(|command| tool_ops::shell(command, text(input, "toolSummary"), None, result.as_ref().map(|_| output))),
        (Some("search_web"), _) => text(input, "query").map(|query| Op::web_search(query, text(input, "domain"), None)),
        (Some("ask_question"), _) => Some(Op::ask(input["questions"].as_array().into_iter().flatten().filter_map(|q| Some(Question {
            question: text(q, "question")?.into(),
            header: None,
            options: q["options"].as_array().into_iter().flatten().filter_map(|o| Some(Choice { label: o.as_str()?.into(), description: None })).collect(),
            answer: None,
        })).collect(), None)),
        (Some("invoke_subagent"), _) => Some(Op::Agent {
            agent: text(input, "Model").filter(|model| *model != "inherit").map(Into::into),
            task: text(input, "toolSummary").or_else(|| text(input, "Prompt").and_then(|prompt| prompt.lines().next())).unwrap_or("子任务").into(),
            background: false,
            encrypted: false,
        }),
        (Some("send_message"), _) => Some(Op::Message { to: text(input, "Recipient").unwrap_or("主任务").into(), summary: text(input, "toolSummary").map(Into::into), encrypted: false }),
        (Some("schedule"), _) => input["DurationSeconds"].as_f64().map(|seconds| Op::Wake { seconds, reason: text(input, "Prompt").map(Into::into) }),
        (Some("manage_task"), _) if text(input, "Action") == Some("kill") => text(input, "TaskId").map(|task| Op::Stop {
            task: task.rsplit('/').next().unwrap_or(task).into(),
            detail: output.lines().last().map(Into::into),
        }),
        _ => None,
    };
    if let Some(result) = result {
        result.block["content"] = content.unwrap_or(original);
    }
    op.into_iter().collect()
}
