use super::super::tool_ops::{self, numbered, text, Choice, Hunk, Op, Question};
use serde_json::Value;

/// 读取、技能等结果另返回整理后的正文，替换卡片里的原始输出。
pub(super) fn describe(name: &str, state: &Value) -> (Vec<Op>, Option<String>) {
    let input = &state["input"];
    let metadata = &state["metadata"];
    let output = match &state["output"] {
        Value::String(output) => Some(output.as_str()),
        _ => state["content"].as_array().filter(|parts| parts.len() == 1).and_then(|parts| parts[0]["text"].as_str()),
    }.filter(|_| state["status"] == "completed");
    let path = text(input, "filePath").or_else(|| text(input, "path"));
    let paths = || path.into_iter().map(Into::into).collect::<Vec<_>>();
    let op = match (name, path) {
        ("read", Some(path)) => {
            return match output.and_then(numbered) {
                Some((start, end, content)) => match tool_ops::read_skill(path, &content) {
                    Some(skill) => (vec![skill], None),
                    None => {
                        let total = (metadata["truncated"] == false).then_some(end);
                        (vec![Op::read_range(path, start, end - start + 1, total)], Some(content))
                    }
                },
                None => (vec![Op::read(path)], None),
            };
        }
        ("edit" | "multiedit", Some(path)) => Op::edit(path, patches(metadata).into_iter().next()
            .map(|(_, _, hunks)| hunks)
            .unwrap_or_else(|| {
                let edits = input["edits"].as_array().map_or_else(|| vec![input], |edits| edits.iter().collect());
                edits.iter().map(|edit| Hunk::replacement(
                    edit["oldString"].as_str().unwrap_or_default(),
                    edit["newString"].as_str().unwrap_or_default(),
                    None,
                )).collect()
            })),
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
        ("glob", _) => tool_ops::find(text(input, "pattern"), paths(), output),
        ("list", _) => tool_ops::list(paths(), output),
        ("bash" | "shell", _) => match text(input, "command") {
            Some(command) => tool_ops::shell(command, text(input, "description"), metadata["exit"].as_i64(), output),
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
            Some(url) => Op::WebFetch { url: url.into(), prompt: None },
            None => return (Vec::new(), None),
        },
        ("websearch", _) => match text(input, "query") {
            Some(query) => Op::web_search(query, None, output),
            None => return (Vec::new(), None),
        },
        ("question", _) => Op::ask(input["questions"].as_array().into_iter().flatten().filter_map(|q| Some(Question {
            question: text(q, "question")?.into(),
            header: text(q, "header").map(Into::into),
            options: q["options"].as_array().into_iter().flatten().filter_map(|o| Some(Choice {
                label: text(o, "label")?.into(),
                description: text(o, "description").map(Into::into),
            })).collect(),
            answer: None,
        })).collect(), output),
        ("task" | "subagent", _) => Op::Agent {
            agent: text(input, "subagent_type").or_else(|| text(input, "agent")).map(Into::into),
            task: text(input, "description").or_else(|| text(input, "prompt").and_then(|prompt| prompt.lines().next())).unwrap_or("子任务").into(),
            background: false,
            encrypted: false,
        },
        ("todowrite", _) => Op::todo(&input["todos"], "content"),
        _ => return (Vec::new(), None),
    };
    (vec![op], None)
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
