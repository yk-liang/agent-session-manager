use super::super::tool_ops::{self, error_summary, first_line, numbered, text, Hunk, Op, ToolResult};
use serde_json::{json, Value};

/// 去掉执行时间、行号前缀和命令回执后写入 `output`，原始结果保留在 `content` 供「原文」查看。
pub(super) fn describe(call: &Value, result: Option<ToolResult<'_>>) -> Vec<Op> {
    let mut result = result;
    let recorded = result.as_ref().and_then(|result| result.block["content"].as_str()).unwrap_or_default().to_owned();
    let name = call["name"].as_str();
    let saved_file = truncated_file(&recorded).map(str::to_owned);
    // 命令的回执另由 `command_receipt` 解析，其余结果只去掉开头的截断说明和执行时间。
    let output = if name == Some("run_command") { recorded.as_str() } else { without_timestamps(&recorded) };
    let input = &call["input"];
    let mut cleaned = (output.len() != recorded.len()).then(|| output.to_owned());
    let op = match (name, text(input, "Path")) {
        (Some("read_file"), Some(path)) if is_image(path) => Some(Op::Image { path: path.into() }),
        (Some("read_file"), Some(path)) => match numbered(output) {
            Some((start, end, text)) => {
                let total = output.lines().find_map(|line| line.strip_prefix("Showing lines "))
                    .and_then(|range| range.rsplit_once(" of ")?.1.trim().parse().ok());
                cleaned = Some(text);
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
        (Some("run_command"), _) => text(input, "CommandLine").map(|command| {
            let receipt = command_receipt(output);
            if let Some((exit, body)) = receipt {
                cleaned = Some(body.to_owned());
                if let Some(result) = &mut result {
                    if exit != 0 {
                        if let Some(reason) = error_summary(body) {
                            result.block["error_summary"] = json!(reason);
                        }
                    }
                }
            }
            let (exit, stdout) = receipt.map_or((None, result.as_ref().map(|_| output)), |(exit, body)| (Some(exit), Some(body)));
            tool_ops::shell(command, text(input, "toolSummary"), exit, stdout)
        }),
        (Some("search_web"), _) => {
            if let Some((_, summary)) = output.split_once(" returned the following summary:\n").filter(|(intro, _)| intro.starts_with("The search for ")) {
                cleaned = Some(summary.to_owned());
            }
            Some(Op::web_search(input, None, None))
        }
        (Some("ask_question"), _) => Some(Op::ask(&input["questions"], None)),
        (Some("invoke_subagent"), _) => Some(Op::Agent {
            agent: text(input, "Role").or_else(|| text(input, "Model").filter(|model| *model != "inherit")).map(Into::into),
            task: text(input, "toolSummary").map(Into::into).or_else(|| text(input, "Prompt").map(first_line)).unwrap_or_else(|| "子任务".into()),
            background: false,
            encrypted: false,
        }),
        (Some("send_message"), _) => Some(Op::Message {
            to: text(input, "Recipient").unwrap_or("主任务").into(),
            summary: text(input, "toolSummary").map(Into::into).or_else(|| text(input, "Message").map(first_line)),
            encrypted: false,
        }),
        // 秒数有时以字符串记录（`"180"`）。
        (Some("schedule"), _) => input["DurationSeconds"].as_f64().or_else(|| text(input, "DurationSeconds")?.trim().parse().ok())
            .map(|seconds| Op::Wake { seconds, reason: text(input, "Prompt").map(Into::into) }),
        (Some("manage_task"), _) if text(input, "Action") == Some("kill") => text(input, "TaskId").map(|task| Op::Stop {
            task: task.rsplit('/').next().unwrap_or(task).into(),
            detail: output.lines().last().map(Into::into),
        }),
        (Some("manage_task"), _) if text(input, "Action") == Some("status") => text(input, "TaskId")
            .map(|task| Op::TaskStatus { task: task.rsplit('/').next().unwrap_or(task).into() }),
        (Some("manage_subagents"), _) if text(input, "Action") == Some("list") => Some(Op::AgentList),
        (Some("manage_subagents"), _) if text(input, "Action") == Some("kill_all") => Some(Op::Stop { task: "全部子智能体".into(), detail: None }),
        _ => None,
    };
    if let Some(result) = result {
        if let Some(cleaned) = cleaned.filter(|cleaned| cleaned != &recorded) {
            result.block["output"] = json!(cleaned);
        }
        if let Some(file) = saved_file {
            result.block["output_file"] = json!(file);
        }
    }
    op.into_iter().collect()
}

fn truncated_file(text: &str) -> Option<&str> {
    let first = text.lines().next()?;
    if !first.starts_with("The tool's output was truncated") {
        return None;
    }
    let rest = first.split_once("file://")?.1.trim_end();
    Some(rest.strip_suffix('.').unwrap_or(rest))
}

fn without_timestamps(text: &str) -> &str {
    let text = if text.starts_with("The tool's output was truncated") {
        text.split_once('\n').map_or(text, |(_, rest)| rest.trim_start_matches('\n'))
    } else {
        text
    };
    text.strip_prefix("Created At: ").and_then(|rest| rest.split_once('\n'))
        .and_then(|(_, rest)| rest.strip_prefix("Completed At: ")?.split_once('\n'))
        .map_or(text, |(_, rest)| rest)
}

fn is_image(path: &str) -> bool {
    path.rsplit_once('.').is_some_and(|(_, ext)| matches!(ext.to_ascii_lowercase().as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp"))
}

fn command_receipt(text: &str) -> Option<(i64, &str)> {
    let with_timestamps = !text.starts_with("Command exited with code ");
    let rest = if with_timestamps {
        let (prefix, rest) = text.split_once("Created At: ")?;
        if !prefix.is_empty() && !prefix.starts_with("The tool's output was truncated") { return None; }
        let (created, rest) = rest.split_once('\n')?;
        let (completed, rest) = rest.strip_prefix("Completed At: ")?.split_once('\n')?;
        chrono::DateTime::parse_from_rfc3339(created.trim_end()).ok()?;
        chrono::DateTime::parse_from_rfc3339(completed.trim_end()).ok()?;
        rest
    } else { text };
    let (status, rest) = rest.split_once('\n').unwrap_or((rest, ""));
    let exit = status.trim_end().strip_prefix("Command exited with code ")?.strip_suffix('.')?.parse().ok()?;
    let rest = rest.trim_start_matches(['\n', '\r']);
    let body = rest.strip_prefix("Command output:\n").or_else(|| rest.strip_prefix("Command output:\r\n"))
        .or_else(|| (rest.trim_end() == "No output from the command.").then_some(""));
    // 带时间的回执里，没有命令输出时其后是工具自己的说明（例如沙箱报错），也当作输出展示。
    if with_timestamps { Some((exit, body.unwrap_or(rest))) } else { body.map(|body| (exit, body)) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_receipts_preserve_output_but_use_recorded_exit() {
        let receipt = "Created At: 2026-01-12T12:00:00+00:00\nCompleted At: 2026-01-12T12:00:01+00:00\nCommand exited with code 128.\n\nCommand output:\nfatal: missing ref";
        let mut result = json!({"content":receipt,"status":"completed"});
        let ops = describe(&json!({"name":"run_command","input":{"CommandLine":"git reset --soft HEAD~1 && git log"}}),
            Some(ToolResult { block: &mut result, event: &mut Value::Null }));
        let shell = json!(&ops[0]);
        assert_eq!(shell["exit_code"], 128);
        assert_eq!(shell["intent"], "git_change");
        assert_eq!(result["content"], receipt);
        assert_eq!(result["output"], "fatal: missing ref");
        assert_eq!(command_receipt("Command exited with code 1.\n\nNo output from the command."), Some((1, "")));
        assert!(command_receipt("Command exited with code 128.\nother output").is_none());
        let raw_search = "Created At: 2026-01-12T12:00:00+00:00\nCompleted At: 2026-01-12T12:00:01+00:00\nThe search for \"q\" returned the following summary:\n## Answer";
        let mut search = json!({"content":raw_search});
        describe(&json!({"name":"search_web","input":{"query":"q"}}), Some(ToolResult { block: &mut search, event: &mut Value::Null }));
        assert_eq!((search["content"].clone(), search["output"].clone()), (json!(raw_search), json!("## Answer")));
        let raw_read = "The tool's output was truncated due to being too long. You can find the full output in a file at file:///tmp/output.txt.\nCreated At: 2026-01-12T12:00:00+00:00\nCompleted At: 2026-01-12T12:00:01+00:00\nShowing lines 1-2 of 2\n1: first\n2: second";
        let mut read = json!({"content":raw_read});
        describe(&json!({"name":"read_file","input":{"Path":"/repo/a.txt"}}), Some(ToolResult { block: &mut read, event: &mut Value::Null }));
        assert_eq!((read["content"].clone(), read["output"].clone(), read["output_file"].clone()), (json!(raw_read), json!("first\nsecond"), json!("/tmp/output.txt")));
    }
}
