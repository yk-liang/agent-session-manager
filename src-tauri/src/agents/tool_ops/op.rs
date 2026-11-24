//! 工具卡片的统一描述：各 adapter 把自家工具调用映射成这些操作，前端按 `kind` 选卡片。
use super::{line_count, shell::Shell, text};
use serde::Serialize;
use serde_json::Value;
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
    Shell(Shell),
    Script(Script),
    Exec {
        calls: Vec<ExecCall>,
        lines: usize,
        /// 主动作所在的调用与操作下标。
        #[serde(skip_serializing_if = "Option::is_none")]
        lead: Option<(usize, usize)>,
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
        #[serde(skip_serializing_if = "Not::not")]
        find: bool,
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
    TerminalInput { session: u64, action: TerminalAction },
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

/// 后台会话的一次交互：读取输出、发送输入或发送 Ctrl-C。
#[derive(Serialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TerminalAction {
    Poll,
    Send,
    Interrupt,
}

/// exec 脚本里的一次工具调用。参数求得出时，按同名工具单独调用的规则描述成操作。
#[derive(Serialize, Clone, Debug, PartialEq)]
pub(crate) struct ExecCall {
    pub tool: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ops: Vec<Op>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// 没有操作时代替目标的参数摘要。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// 求出的参数；运行时才确定的字符串片段保留为 `${源码}`。
    #[serde(skip_serializing_if = "Value::is_null")]
    pub input: Value,
    /// 参数含运行时才确定的部分时，参数的源码。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expression: Option<String>,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ExecFeature {
    Parallel,
    Loop,
    Conditional,
    State,
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

    /// 解析各 adapter 统一格式的提问参数，并从结果里的 `"问题"="回答"` 取回回答。
    pub fn ask(questions: &Value, output: Option<&str>) -> Op {
        let output = output.unwrap_or_default();
        let questions = questions.as_array().into_iter().flatten().filter_map(|q| {
            let question = text(q, "title").or_else(|| text(q, "question"))?.to_owned();
            let key = format!("\"{question}\"=\"");
            let answer = output.split_once(&key)
                .and_then(|(_, rest)| rest.split_once('"'))
                .map(|(answer, _)| answer.to_owned());
            let options = q["options"].as_array().into_iter().flatten().filter_map(|o| Some(Choice {
                label: o.as_str().or_else(|| text(o, "label"))?.into(),
                description: text(o, "description").map(Into::into),
            })).collect();
            Some(Question { question, header: text(q, "header").map(Into::into), options, answer })
        }).collect();
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

    /// MCP 调用：标题用最能区分这次调用的参数（标题、网址、查询、路径、按键、视口等）。
    pub fn mcp(server: &str, tool: &str, input: &Value) -> Op {
        let title = ["title", "url", "query", "q", "path", "filePath", "selector", "key", "viewport", "uid", "name", "text", "prompt"].into_iter()
            .find_map(|key| text(input, key)).map(super::first_line);
        Op::Mcp { server: server.into(), tool: tool.into(), title }
    }

    /// 几个操作并列时谁能代表整体：改变状态的高于运行，运行高于查看，轮询、看图和记账最低。
    pub fn weight(&self) -> u8 {
        match self {
            Op::Shell(shell) => shell.priority(),
            Op::Edit { .. } | Op::Write { .. } => 6,
            Op::Script(_) | Op::WebSearch { .. } | Op::WebFetch { .. } | Op::Mcp { .. } | Op::GenerateImage { .. }
                | Op::Agent { .. } | Op::Message { .. } | Op::Stop { .. } => 5,
            Op::Delete { .. } => 4,
            Op::Read { .. } | Op::Search { .. } | Op::Find { .. } | Op::List { .. } | Op::Skill { .. } => 1,
            _ => 0,
        }
    }

    /// 延迟加载时只保留卡片标题所需的字段，正文随原始数据一起按需读取。
    pub fn summary(mut self) -> Op {
        self.strip_bodies();
        self
    }

    pub(super) fn strip_bodies(&mut self) {
        match self {
            Op::Edit { hunks, .. } => *hunks = None,
            Op::Write { content, .. } => *content = None,
            Op::Search { hits, .. } => *hits = None,
            Op::Shell(shell) => shell.strip_bodies(),
            Op::Script(script) => script.body = None,
            Op::Exec { calls, .. } => {
                for call in calls {
                    call.ops.iter_mut().for_each(Op::strip_bodies);
                    call.input = Value::Null;
                    call.expression = None;
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

impl Hunk {
    /// 只知道替换前后的文本时，剥离相同首尾后按有界 LCS 生成差异，中段过大或单侧为空时退化为整段替换。
    pub fn replacement(old: &str, new: &str, start: Option<u64>) -> Hunk {
        let old: Vec<_> = old.split('\n').collect();
        let new: Vec<_> = new.split('\n').collect();
        Hunk { old_start: start, new_start: start, lines: line_diff(&old, &new) }
    }

    /// 单个编辑或 `edits[]` 批量编辑的替换差异。
    pub fn replacements(input: &Value, old_key: &str, new_key: &str) -> Vec<Hunk> {
        let edits = input["edits"].as_array().map_or_else(|| vec![input], |edits| edits.iter().collect());
        edits.iter().map(|edit| Hunk::replacement(
            edit[old_key].as_str().unwrap_or_default(),
            edit[new_key].as_str().unwrap_or_default(),
            None,
        )).collect()
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

fn line_diff(old: &[&str], new: &[&str]) -> Vec<String> {
    let prefix = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    let suffix = old[prefix..].iter().rev().zip(new[prefix..].iter().rev()).take_while(|(a, b)| a == b).count();
    let (a, b) = (&old[prefix..old.len() - suffix], &new[prefix..new.len() - suffix]);
    let mut lines: Vec<String> = old[..prefix].iter().map(|line| format!(" {line}")).collect();
    if !a.is_empty() && !b.is_empty() && a.len().saturating_mul(b.len()) <= 250_000 {
        let stride = b.len() + 1;
        let mut common = vec![0u32; (a.len() + 1) * stride];
        for i in (0..a.len()).rev() {
            for j in (0..b.len()).rev() {
                common[i * stride + j] = if a[i] == b[j] {
                    common[(i + 1) * stride + j + 1] + 1
                } else {
                    common[(i + 1) * stride + j].max(common[i * stride + j + 1])
                };
            }
        }
        let (mut i, mut j) = (0, 0);
        while i < a.len() || j < b.len() {
            if i < a.len() && j < b.len() && a[i] == b[j] {
                lines.push(format!(" {}", a[i]));
                (i, j) = (i + 1, j + 1);
            } else if j == b.len() || i < a.len() && common[(i + 1) * stride + j] >= common[i * stride + j + 1] {
                lines.push(format!("-{}", a[i]));
                i += 1;
            } else {
                lines.push(format!("+{}", b[j]));
                j += 1;
            }
        }
    } else {
        lines.extend(a.iter().map(|line| format!("-{line}")));
        lines.extend(b.iter().map(|line| format!("+{line}")));
    }
    lines.extend(old[old.len() - suffix..].iter().map(|line| format!(" {line}")));
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replacement_keeps_shared_lines_as_context() {
        let hunk = Hunk::replacement("a\nb\nc", "a\nB\nb2\nc", Some(4));
        assert_eq!(hunk.lines, [" a", "-b", "+B", "+b2", " c"]);
        let moved = Hunk::replacement("a\nx\nb\nc\nd", "a\nb\nc\ny\nd", None);
        assert_eq!(moved.lines, [" a", "-x", " b", " c", "+y", " d"]);
        assert!(matches!(Op::edit("f", vec![hunk]), Op::Edit { added: 2, removed: 1, .. }));
    }

    #[test]
    fn unified_diff_hunks_keep_line_numbers() {
        let hunks = Hunk::parse_unified("--- a\n+++ b\n@@ -3,2 +3,3 @@\n x\n-y\n+z\n+w\n\\ No newline at end of file\n");
        assert_eq!(hunks, [Hunk { old_start: Some(3), new_start: Some(3), lines: vec![" x".into(), "-y".into(), "+z".into(), "+w".into()] }]);
    }
}
