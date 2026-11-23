//! 工具卡片的统一描述。各 adapter 只把自家工具映射到这里；shell 命令统一由 `shell` 解析。
mod browse;
mod shell;
mod skill;

pub(crate) use browse::{find, list, search};
pub(crate) use shell::{shell, CommandOutput};
pub(crate) use skill::{loaded as loaded_skill, promote as promote_skill, promote_text as promote_skill_text};

use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::ops::Not;

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum Op {
    Read {
        path: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        start: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        end: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        total: Option<u64>,
        /// `tail` 只知道读了末尾几行。
        #[serde(skip_serializing_if = "Option::is_none")]
        tail: Option<u64>,
    },
    Edit {
        path: String,
        added: usize,
        removed: usize,
        #[serde(skip_serializing_if = "Option::is_none")]
        hunks: Option<Vec<Hunk>>,
    },
    Write {
        path: String,
        lines: usize,
        #[serde(skip_serializing_if = "Option::is_none")]
        created: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        content: Option<String>,
    },
    Delete {
        path: String,
    },
    Search {
        pattern: String,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        paths: Vec<String>,
        /// 范围是具体文件而不是目录。
        #[serde(skip_serializing_if = "Not::not")]
        in_file: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        matches: Option<usize>,
        #[serde(skip_serializing_if = "Option::is_none")]
        files: Option<usize>,
        #[serde(skip_serializing_if = "Option::is_none")]
        hits: Option<Vec<Hits>>,
        #[serde(skip_serializing_if = "Not::not")]
        truncated: bool,
    },
    Find {
        #[serde(skip_serializing_if = "Option::is_none")]
        pattern: Option<String>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        paths: Vec<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        count: Option<usize>,
    },
    List {
        #[serde(skip_serializing_if = "Vec::is_empty")]
        paths: Vec<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        count: Option<usize>,
    },
    Shell(shell::Shell),
    Script(Script),
    Exec {
        calls: Vec<ExecCall>,
        lines: usize,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        features: Vec<ExecFeature>,
    },
    Skill {
        name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        args: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        path: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        body: Option<String>,
    },
    WebSearch {
        queries: Vec<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        domain: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        results: Option<usize>,
    },
    WebFetch {
        url: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        prompt: Option<String>,
    },
    Ask {
        questions: Vec<Question>,
    },
    Agent {
        #[serde(skip_serializing_if = "Option::is_none")]
        agent: Option<String>,
        task: String,
        #[serde(skip_serializing_if = "Not::not")]
        background: bool,
        /// 数据源只保存了指令的密文。
        #[serde(skip_serializing_if = "Not::not")]
        encrypted: bool,
    },
    Message {
        to: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        summary: Option<String>,
        #[serde(skip_serializing_if = "Not::not")]
        encrypted: bool,
    },
    Wait {
        #[serde(skip_serializing_if = "Option::is_none")]
        seconds: Option<f64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        target: Option<String>,
    },
    Wake {
        seconds: f64,
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    Stop {
        task: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
    TaskStatus { task: String },
    TerminalInput { session: u64, send: bool },
    AgentList,
    Monitor {
        description: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        minutes: Option<u64>,
    },
    LoadTools {
        names: Vec<String>,
    },
    Todo {
        items: Vec<TodoItem>,
    },
    Image {
        path: String,
    },
    GenerateImage {
        #[serde(skip_serializing_if = "Option::is_none")]
        path: Option<String>,
    },
    Report {
        title: &'static str,
        #[serde(skip_serializing_if = "Option::is_none")]
        summary: Option<String>,
    },
    Mcp {
        server: String,
        tool: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        title: Option<String>,
    },
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum ExecCall {
    Command {
        shell: shell::Shell,
        #[serde(skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
    },
    Patch { changes: Vec<Op> },
    Image { path: String },
    Tool {
        name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        action: Option<&'static str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        target: Option<String>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        fields: Vec<(String, Value)>,
        #[serde(skip_serializing_if = "Option::is_none")]
        expression: Option<String>,
        #[serde(skip_serializing_if = "Not::not")]
        dynamic: bool,
    },
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ExecFeature {
    Parallel,
    Loop,
    Conditional,
    State,
    Output,
    Invalid,
}

/// 行以 ` `、`-`、`+` 开头；起始行号未知时为空。
#[derive(Serialize, Clone, Debug, PartialEq)]
pub(crate) struct Hunk {
    pub old_start: Option<u64>,
    pub new_start: Option<u64>,
    pub lines: Vec<String>,
}

/// 一个文件里的搜索命中；`context` 是 `-A`、`-B` 带出的上下文行。
#[derive(Serialize, Clone, Debug, PartialEq)]
pub(crate) struct Hits {
    pub path: String,
    pub lines: Vec<Hit>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub(crate) struct Hit {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u64>,
    pub text: String,
    #[serde(skip_serializing_if = "Not::not")]
    pub context: bool,
}

/// 脚本正文：heredoc、`-c`/`-e` 参数或脚本工具的代码。
#[derive(Serialize, Clone, Debug, PartialEq)]
pub(crate) struct Script {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub lines: usize,
    /// 第一行有意义的代码，没有说明时作为标题旁的摘要。
    pub first: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub(crate) struct Question {
    pub question: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub header: Option<String>,
    pub options: Vec<Choice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub(crate) struct Choice {
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub(crate) struct TodoItem {
    pub text: String,
    pub status: String,
}

impl Op {
    pub fn read(path: &str) -> Op {
        Op::Read { path: path.into(), start: None, end: None, total: None, tail: None }
    }

    /// `lines` 是实际返回的行数；未截断到文件末尾时总行数未知。
    pub fn read_range(path: &str, start: u64, lines: u64, total: Option<u64>) -> Op {
        let end = lines.checked_sub(1).and_then(|last| start.checked_add(last));
        Op::Read { path: path.into(), start: Some(start), end, total, tail: None }
    }

    pub fn edit(path: &str, hunks: Vec<Hunk>) -> Op {
        let count = |sign| hunks.iter().flat_map(|h| &h.lines).filter(|l| l.starts_with(sign)).count();
        Op::Edit { path: path.into(), added: count('+'), removed: count('-'), hunks: Some(hunks) }
    }

    pub fn write(path: &str, content: &str, created: Option<bool>) -> Op {
        Op::Write { path: path.into(), lines: line_count(content), created, content: Some(content.into()) }
    }

    pub fn script(lang: &str, title: Option<&str>, code: &str) -> Op {
        let mut script = Script::new(Some(lang), code);
        script.title = title.map(str::trim).filter(|t| !t.is_empty()).map(Into::into);
        Op::Script(script)
    }

    /// 问题的回答从结果里的 `"问题"="回答"` 取出。
    pub fn ask(mut questions: Vec<Question>, output: Option<&str>) -> Op {
        let output = output.unwrap_or_default();
        for question in &mut questions {
            let key = format!("\"{}\"=\"", question.question);
            question.answer = output.split_once(&key)
                .and_then(|(_, rest)| rest.split_once('"'))
                .map(|(answer, _)| answer.to_owned());
        }
        Op::Ask { questions }
    }

    pub fn todo(items: &Value, text_key: &str) -> Op {
        let items = items.as_array().into_iter().flatten().filter_map(|item| Some(TodoItem {
            text: item[text_key].as_str()?.to_owned(),
            status: item["status"].as_str().unwrap_or("pending").to_owned(),
        })).collect();
        Op::Todo { items }
    }

    pub fn web_search(input: &Value, fallback: Option<&str>, results: Option<usize>) -> Op {
        let mut queries: Vec<String> = Vec::new();
        for query in input["queries"].as_array().into_iter().flatten().filter_map(Value::as_str)
            .chain(text(input, "query")).map(str::trim).filter(|query| !query.is_empty())
        {
            if !queries.iter().any(|seen| seen == query) {
                queries.push(query.into());
            }
        }
        if queries.is_empty() {
            queries.extend(fallback.map(str::trim).filter(|query| !query.is_empty()).map(str::to_owned));
        }
        Op::WebSearch { queries, domain: text(input, "domain").map(Into::into), results }
    }

    /// 延迟加载时只保留卡片标题所需的字段，正文随原始数据一起按需读取。
    pub fn summary(mut self) -> Op {
        self.strip_bodies();
        self
    }

    fn strip_bodies(&mut self) {
        match self {
            Op::Edit { hunks, .. } => *hunks = None,
            Op::Write { content, .. } => *content = None,
            Op::Search { hits, .. } => *hits = None,
            Op::Shell(shell) => shell.strip_bodies(),
            Op::Script(script) => script.body = None,
            Op::Exec { calls, .. } => {
                for call in calls {
                    match call {
                        ExecCall::Command { shell, .. } => shell.strip_bodies(),
                        ExecCall::Patch { changes } => changes.iter_mut().for_each(Op::strip_bodies),
                        ExecCall::Tool { fields, expression, .. } => {
                            fields.clear();
                            *expression = None;
                        }
                        _ => {}
                    }
                }
            }
            Op::Skill { body, .. } => *body = None,
            _ => {}
        }
    }
}

impl Script {
    pub fn new(lang: Option<&str>, body: &str) -> Script {
        let first = body.lines().map(str::trim)
            .find(|line| !line.is_empty() && !["#", "//", "import ", "from ", "use ", "\"use ", "'use "].iter().any(|p| line.starts_with(p)))
            .unwrap_or_default();
        Script {
            lang: lang.map(Into::into),
            title: None,
            lines: line_count(body),
            first: first.chars().take(160).collect(),
            body: Some(body.into()),
        }
    }
}

/// 末尾换行不算作新的一行。
pub(crate) fn line_count(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }
    text.strip_suffix('\n').unwrap_or(text).split('\n').count()
}

impl Hunk {
    /// 只知道替换前后的文本时，首尾相同的行作为上下文，中间按整段替换。
    pub fn replacement(old: &str, new: &str, start: Option<u64>) -> Hunk {
        let old: Vec<_> = old.split('\n').collect();
        let new: Vec<_> = new.split('\n').collect();
        let prefix = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
        let suffix = old[prefix..].iter().rev().zip(new[prefix..].iter().rev())
            .take_while(|(a, b)| a == b).count();
        let context = |lines: &[&str]| lines.iter().map(|l| format!(" {l}")).collect::<Vec<_>>();
        let mut lines = context(&old[..prefix]);
        lines.extend(old[prefix..old.len() - suffix].iter().map(|l| format!("-{l}")));
        lines.extend(new[prefix..new.len() - suffix].iter().map(|l| format!("+{l}")));
        lines.extend(context(&old[old.len() - suffix..]));
        Hunk { old_start: start, new_start: start, lines }
    }

    /// 解析 unified diff，忽略文件头与 `\ No newline` 标记。
    pub fn parse_unified(text: &str) -> Vec<Hunk> {
        let mut hunks: Vec<Hunk> = Vec::new();
        for line in text.lines() {
            if let Some(header) = line.strip_prefix("@@ ") {
                let start = |sign| header.split(' ').find_map(|part| part.strip_prefix(sign))
                    .and_then(|range| range.split(',').next()?.parse().ok());
                hunks.push(Hunk { old_start: start('-'), new_start: start('+'), lines: Vec::new() });
            } else if let Some(hunk) = hunks.last_mut() {
                if line.starts_with([' ', '+', '-']) {
                    hunk.lines.push(line.into());
                } else if line.is_empty() {
                    hunk.lines.push(" ".into());
                }
            }
        }
        for hunk in &mut hunks {
            while hunk.lines.last().is_some_and(|line| line == " ") {
                hunk.lines.pop();
            }
        }
        hunks
    }
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

impl ToolResult<'_> {
    /// 结果正文；多段时取唯一的文本段。
    pub fn text(&self) -> Option<&str> {
        let content = &self.block["content"];
        content.as_str().or_else(|| content.as_array()
            .filter(|parts| parts.len() == 1 && parts[0]["type"] == "text")
            .and_then(|parts| parts[0]["text"].as_str()))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replacement_keeps_shared_lines_as_context() {
        let hunk = Hunk::replacement("a\nb\nc", "a\nB\nb2\nc", Some(4));
        assert_eq!(hunk.lines, [" a", "-b", "+B", "+b2", " c"]);
        assert!(matches!(Op::edit("f", vec![hunk]), Op::Edit { added: 2, removed: 1, .. }));
    }

    #[test]
    fn numbered_output_drops_headers_and_trailers() {
        let text = "Showing lines 7-9 of 20\n7: fn a() {\n8: \n9:   b()\n[Output truncated. Continue reading with offset: 10]";
        assert_eq!(numbered(text), Some((7, 9, "fn a() {\n\n  b()".into())));
        assert_eq!(numbered("no numbers"), None);
        assert_eq!(numbered("1: first\nunnumbered\n3: third"), None);
        assert_eq!(numbered("0: zero"), None);
    }

    #[test]
    fn unified_diff_hunks_keep_line_numbers() {
        let hunks = Hunk::parse_unified("--- a\n+++ b\n@@ -3,2 +3,3 @@\n x\n-y\n+z\n+w\n\\ No newline at end of file\n");
        assert_eq!(hunks, [Hunk { old_start: Some(3), new_start: Some(3), lines: vec![" x".into(), "-y".into(), "+z".into(), "+w".into()] }]);
    }
}
