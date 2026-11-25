use super::super::tool_ops::{self, first_line, numbered, text, Hunk, Op};
use serde_json::Value;

pub(super) fn bash_exit(state: &Value) -> Option<i64> {
    state["metadata"]["exit"].as_i64().or_else(|| {
        let error = [&state["error"], &state["error"]["message"], &state["error"]["data"]["message"]]
            .into_iter().filter_map(Value::as_str).find(|s| !s.trim().is_empty())?;
        error.trim().strip_prefix("Exit code ")?.parse().ok()
    })
}

/// 另返回去掉工具包装（行号前缀、目录摘要、退出码尾注、占位文字）的输出正文，原始结果保留给「原文」。
pub(super) fn describe(name: &str, state: &Value) -> (Vec<Op>, Option<String>) {
    let input = &state["input"];
    let metadata = &state["metadata"];
    let output = match &state["output"] {
        Value::String(output) => Some(output.as_str()),
        _ => state["content"].as_array().filter(|parts| parts.len() == 1).and_then(|parts| parts[0]["text"].as_str()),
    }.filter(|_| state["status"] == "completed");
    let path = text(input, "filePath").or_else(|| text(input, "path"));
    let paths = || path.into_iter().map(Into::into).collect::<Vec<_>>();
    let mut body = None;
    let op = match (name, path) {
        ("read", Some(path)) if state["content"].as_array().is_some_and(|parts| parts.iter().any(|part| {
            let mime = part["mime"].as_str().unwrap_or_default();
            part["type"] == "file" && matches!(mime, "image/png" | "image/jpeg" | "image/webp" | "image/gif")
                && part["uri"].as_str().is_some_and(|uri| uri.starts_with("data:image/"))
        })) => Op::Image { path: path.into() },
        ("read", Some(_)) if output.is_some_and(|output| output.starts_with("Read directory ")) => {
            let entries = output.and_then(|output| output.split_once('\n')).map_or("", |(_, entries)| entries);
            body = Some(entries.into());
            tool_ops::list(paths(), Some(entries))
        }
        ("read", Some(path)) => match output.and_then(numbered) {
            Some((start, end, text)) => {
                body = Some(text);
                let total = (metadata["truncated"] == false).then_some(end);
                Op::read_range(path, start, end - start + 1, total)
            }
            None => Op::read(path),
        },
        ("edit" | "multiedit", Some(path)) => Op::edit(path, patches(metadata).into_iter().next()
            .map(|(_, _, hunks)| hunks).filter(|h| !h.is_empty())
            .unwrap_or_else(|| Hunk::replacements(input, "oldString", "newString"))),
        ("write", Some(path)) => Op::write(path, input["content"].as_str().unwrap_or_default(),
            metadata["exists"].as_bool().map(|exists| !exists)),
        ("patch" | "apply_patch", _) => return (patches(metadata).into_iter().map(|(path, status, hunks)| match status {
            "added" => {
                let content: Vec<_> = hunks.iter().flat_map(|h| &h.lines).filter_map(|l| l.strip_prefix('+')).collect();
                Op::write(&path, &content.join("\n"), Some(true))
            }
            "deleted" => Op::Delete { path },
            _ => Op::edit(&path, hunks),
        }).collect(), None),
        ("grep", _) => match text(input, "pattern") {
            Some(pattern) => tool_ops::search(pattern, paths(), false, output),
            None => return (Vec::new(), None),
        },
        ("glob", _) => {
            if output.is_some_and(|output| output.trim() == "No files found") {
                body = Some(String::new());
            }
            tool_ops::find(text(input, "pattern"), paths(), body.as_deref().or(output))
        }
        ("list", _) => tool_ops::list(paths(), output),
        ("bash" | "shell", _) => match text(input, "command") {
            Some(command) => {
                let recorded = state["content"].as_array().filter(|parts| !parts.is_empty()).and_then(|parts| {
                    parts.iter().map(|part| part["text"].as_str()).collect::<Option<Vec<_>>>()
                        .map(|texts| texts.join("\n"))
                });
                let raw = state["output"].as_str().or(recorded.as_deref());
                let recorded_exit = bash_exit(state);
                let failed = state["status"] == "error" || state.get("error").is_some_and(|v| !v.is_null());
                let (exit, stripped) = raw.map(|text| shell_output(text, recorded_exit, failed)).unzip();
                let stripped = stripped.flatten();
                if let Some(stdout) = stripped {
                    body = Some(stdout.into());
                }
                tool_ops::shell(command, text(input, "description"), recorded_exit.or(exit.flatten()), stripped.or(raw))
            }
            None => return (Vec::new(), None),
        },
        ("execute", _) => match text(input, "code") {
            Some(code) => Op::script("JavaScript", None, code),
            None => return (Vec::new(), None),
        },
        ("skill", _) => {
            tool_ops::loaded_skill(text(input, "name").or_else(|| text(input, "id")).unwrap_or("?"), None, output.unwrap_or_default())
        }
        ("webfetch", _) => match text(input, "url") {
            Some(url) => Op::WebFetch { url: url.into(), prompt: None, find: false },
            None => return (Vec::new(), None),
        },
        ("websearch", _) => Op::web_search(input, None, None),
        ("question", _) => Op::ask(&input["questions"], output),
        ("task" | "subagent", _) => Op::Agent {
            agent: text(input, "subagent_type").or_else(|| text(input, "agent")).map(Into::into),
            task: text(input, "description").map(Into::into).or_else(|| text(input, "prompt").map(first_line)).unwrap_or_else(|| "子任务".into()),
            background: false,
            encrypted: false,
        },
        ("todowrite", _) => Op::todo(&input["todos"], "content"),
        _ => return (Vec::new(), None),
    };
    let mut ops = vec![op];
    if let Some(text) = body.as_deref().or(output) {
        tool_ops::promote_skill_text(&mut ops, text);
    }
    (ops, body)
}

/// 命令输出后附带的退出码（无结构化退出码时不采信会把完成/失败语义反转的文本尾注）；
/// 没有输出时整段是 `(no output)`。
fn shell_output(text: &str, recorded_exit: Option<i64>, failed: bool) -> (Option<i64>, Option<&str>) {
    let trimmed = text.trim_end();
    if trimmed == "(no output)" {
        return (None, Some(""));
    }
    let (rest, last) = trimmed.rsplit_once('\n').unwrap_or(("", trimmed));
    let separated = rest.ends_with('\n') || rest == "(no output)";
    let signal = |value: &str| value.strip_prefix("(SIG").and_then(|s| s.strip_suffix(')'))
        .is_some_and(|name| !name.is_empty() && name.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit()));
    let parsed_exit = match (last.strip_prefix("Exited with code "), last.strip_prefix("Exit code: ")) {
        (Some(code), _) if separated => code.parse().ok(),
        (_, Some(code)) => match code.split_once(' ') {
            Some((code, sig)) if signal(sig) => code.parse().ok(),
            None if separated => code.parse().ok(),
            _ => None,
        },
        _ => None,
    };
    let exit = parsed_exit.filter(|code| match recorded_exit {
        Some(recorded) => recorded == *code,
        // 无结构化退出码时：完成态只认 0，失败态不认可能把失败翻成 no_match 的 1。
        None => if failed { *code != 0 && *code != 1 } else { *code == 0 },
    });
    if exit.is_none() {
        return (None, None);
    }
    let body = rest.trim_end();
    (exit, Some(if body == "(no output)" { "" } else { body }))
}

/// `metadata.files[]` 带每个文件的 unified diff；旧版只有单个 `metadata.diff`。
fn patches(metadata: &Value) -> Vec<(String, &str, Vec<Hunk>)> {
    if let Some(diff) = metadata["diff"].as_str() {
        return vec![(String::new(), "modified", Hunk::parse_unified(diff))];
    }
    metadata["files"].as_array().into_iter().flatten().filter_map(|file| {
        let path = file["filePath"].as_str().or_else(|| file["file"].as_str())?;
        let status = file["status"].as_str().or_else(|| file["type"].as_str()).unwrap_or("modified");
        Some((path.to_owned(), status, Hunk::parse_unified(file["patch"].as_str().unwrap_or_default())))
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tool_notes_become_structure_instead_of_output() {
        let completed = |name: &str, input: Value, output: &str| {
            let (ops, content) = describe(name, &json!({"status":"completed","input":input,"output":output}));
            (json!(ops[0]), content)
        };
        let failed = |name: &str, input: Value, output: &str| {
            let (ops, content) = describe(name, &json!({"status":"error","input":input,"output":output}));
            (json!(ops[0]), content)
        };
        let (shell, body) = failed("bash", json!({"command":"sqlite3 db"}), "zsh:1: command not found: sqlite3\n\nExited with code 127");
        assert_eq!((shell["exit_code"].clone(), body.as_deref()), (json!(127), Some("zsh:1: command not found: sqlite3")));
        let (empty_err, body) = failed("bash", json!({"command":"false"}), "(no output)\nExited with code 2");
        assert_eq!((empty_err["exit_code"].clone(), body.as_deref()), (json!(2), Some("")));
        assert!(completed("bash", json!({"command":"cat log"}), "line\n\nExited with code 1").0.get("exit_code").is_none());
        assert!(failed("bash", json!({"command":"cat doc"}), "Exit code: 7 (documented example)").0.get("exit_code").is_none());
        assert_eq!(completed("bash", json!({"command":"echo x"}), "(no output) from step 1").1, None);
        assert_eq!(completed("bash", json!({"command":"echo x"}), "(no output)\nfrom step 1").1, None);
        assert_eq!(completed("bash", json!({"command":"true"}), "(no output)").1.as_deref(), Some(""));
        let (ops, body) = describe("bash", &json!({"status":"completed","input":{"command":"cat log"},"metadata":{"exit":0},"output":"a\n\nExited with code 7"}));
        assert_eq!((json!(ops[0])["exit_code"].clone(), body), (json!(0), None));
        let (list, body) = completed("read", json!({"filePath":"/repo"}), "Read directory /repo, entries 1-2\nsrc/\nREADME.md");
        assert_eq!((list["kind"].clone(), list["count"].clone(), body.as_deref()), (json!("list"), json!(2), Some("src/\nREADME.md")));
        assert_eq!(completed("glob", json!({"pattern":"*.mdx"}), "No files found").0["count"], 0);
    }
}
