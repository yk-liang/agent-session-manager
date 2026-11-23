use super::super::tool_ops::{self, first_line, numbered, text, Choice, Hunk, Op, Question, ToolResult};
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
        (Some("run_command"), _) => text(input, "CommandLine").map(|command| {
            let receipt = command_receipt(output);
            if let (Some(result), Some((_, Some(body)))) = (&mut result, receipt) {
                result.block["command_output"] = json!(body);
                if receipt.is_some_and(|(exit, _)| exit != 0) {
                    if let Some(reason) = body.lines().map(str::trim).find(|line| !line.is_empty()) {
                        result.block["error_summary"] = json!(reason.chars().take(240).collect::<String>());
                    }
                }
            }
            let (exit, stdout) = receipt.map_or((None, result.as_ref().map(|_| output)), |(exit, body)| (Some(exit), body));
            tool_ops::shell(command, text(input, "toolSummary"), exit, stdout)
        }),
        (Some("search_web"), _) => Some(Op::web_search(input, None, None)),
        (Some("ask_question"), _) => Some(Op::ask(input["questions"].as_array().into_iter().flatten().filter_map(|q| Some(Question {
            question: text(q, "question")?.into(),
            header: None,
            options: q["options"].as_array().into_iter().flatten().filter_map(|o| Some(Choice { label: o.as_str()?.into(), description: None })).collect(),
            answer: None,
        })).collect(), None)),
        (Some("invoke_subagent"), _) => Some(Op::Agent {
            agent: text(input, "Role").or_else(|| text(input, "Model").filter(|model| *model != "inherit")).map(Into::into),
            task: text(input, "toolSummary").or_else(|| text(input, "Prompt").and_then(|prompt| prompt.lines().next())).unwrap_or("子任务").into(),
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
        result.block["content"] = content.unwrap_or(original);
    }
    op.into_iter().collect()
}

fn command_receipt(text: &str) -> Option<(i64, Option<&str>)> {
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
    (with_timestamps || body.is_some()).then_some((exit, body))
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
        assert_eq!(result["command_output"], "fatal: missing ref");
        assert_eq!(command_receipt("Command exited with code 1.\n\nNo output from the command."), Some((1, Some(""))));
        assert!(command_receipt("Command exited with code 128.\nother output").is_none());
    }
}
