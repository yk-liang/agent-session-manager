//! shell 命令解析：去掉 `cd`、环境变量、重定向等噪音并给每一步归类；只读、搜、找、列的命令拆成文件操作。
mod classify;
mod syntax;

use super::{browse, line_count, Op, Script};
use classify::{classify, filter, is_pure_filter, option, script_lang, Segment};
use serde::Serialize;
use std::ops::Not;
use syntax::{heredocs, pipelines};

#[derive(Serialize, Clone, Debug, PartialEq)]
pub(crate) struct Shell {
    /// 完整命令；延迟加载时随正文一起读取。
    #[serde(skip_serializing_if = "Option::is_none")]
    command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    exit_code: Option<i64>,
    #[serde(skip_serializing_if = "Not::not")]
    no_match: bool,
    /// 开头的 `cd` 或 `git -C` 指定的目录。
    #[serde(skip_serializing_if = "Option::is_none")]
    cwd: Option<String>,
    /// 第一步的意图和原文（去掉环境变量、无关重定向）；循环取首行。
    intent: Intent,
    #[serde(skip_serializing_if = "String::is_empty")]
    head: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    filters: Vec<Filter>,
    /// 第一步之后还有几步。
    #[serde(skip_serializing_if = "is_zero")]
    more: usize,
    #[serde(skip_serializing_if = "Not::not")]
    looped: bool,
    /// 写入或原地修改的文件；`append` 表示以 `>>` 追加。
    #[serde(skip_serializing_if = "Option::is_none")]
    target: Option<String>,
    #[serde(skip_serializing_if = "Not::not")]
    append: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    script: Option<Script>,
    /// 只读、搜、找、列的命令按步骤给出文件操作，卡片按文件操作展示。
    #[serde(skip_serializing_if = "Vec::is_empty")]
    steps: Vec<Step>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub(crate) struct Step {
    #[serde(flatten)]
    op: Op,
    program: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    filters: Vec<Filter>,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Intent {
    Read, Search, Find, List, Write, Modify, Script, Run,
    GitDiff, GitLog, GitCommit, Git, Github,
    Test, Build, Install, Remote, Http, Browser, Database,
    Mkdir, Remove, Copy, Move, Process, Kill, Wait, Open, Count, Other,
}

/// 管道后段：截取行数、筛选和计数。
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub(crate) enum Filter {
    Head(u64),
    Tail(u64),
    Include(String),
    Exclude(String),
    Count,
}

fn is_zero(value: &usize) -> bool {
    *value == 0
}

impl Shell {
    pub(super) fn strip_bodies(&mut self) {
        self.command = None;
        if let Some(script) = &mut self.script {
            script.body = None;
        }
        for step in &mut self.steps {
            step.op.strip_bodies();
        }
    }

    pub(super) fn single_read(&self) -> Option<&str> {
        match self.steps.as_slice() {
            [Step { op: Op::Read { path, .. }, program, filters }] if program == "cat" && filters.is_empty() => Some(path),
            _ => None,
        }
    }
}

/// `output` 只用于单步浏览命令的统计。
pub(crate) fn shell(command: &str, description: Option<&str>, exit_code: Option<i64>, output: Option<&str>) -> Op {
    let (head, bodies) = heredocs(command);
    let mut bodies = bodies.into_iter();
    let mut cwd = None;
    let mut parsed: Vec<Parsed> = Vec::new();
    let mut vars = Vec::new();
    let mut no_match = false;
    // 循环从哪里开始；循环体里的路径多半是循环变量，不按文件操作展示。
    let mut looped = None;
    let pipelines = pipelines(&head);
    let no_match_output = pipelines.len() == 1 && exit_code == Some(1) && output.is_some_and(|text| text.trim().is_empty());
    for pipeline in pipelines {
        let segment = Segment::parse(pipeline[0], &vars);
        let rest: Vec<_> = pipeline[1..].iter().map(|text| Segment::parse(text, &vars)).collect();
        let body = segment.heredoc.then(|| bodies.next()).flatten();
        let mut action = classify(&segment, body.is_some());
        // 只读要求后续管道段只做筛选、结果不写进文件；`tee` 和重定向等于写文件。
        if matches!(action, Action::Browse(_)) {
            if segment.target.is_some() || rest.iter().any(|s| s.program() == "tee" || s.target.is_some()) {
                action = Action::Run(Intent::Write);
            } else if !rest.iter().all(is_pure_filter) {
                action = Action::Run(Intent::Other);
            }
        }
        if no_match_output && matches!(action, Action::Browse(_)) {
            no_match = matches!(rest.last().unwrap_or(&segment).program(), "grep" | "egrep" | "fgrep" | "rg");
        }
        match action {
            Action::Noise if parsed.is_empty() && segment.program() == "cd" => cwd = segment.args.get(1).cloned(),
            Action::Noise => vars.extend(segment.assignments()),
            Action::Control => {
                if looped.is_none() && matches!(segment.program(), "for" | "while" | "until" | "if") {
                    looped = Some(pipeline[0].as_ptr() as usize - head.as_ptr() as usize);
                }
            }
            _ => parsed.push(Parsed {
                target: matches!(action, Action::Run(Intent::Write | Intent::Modify)).then(|| written(&segment, &rest)).flatten(),
                filters: rest.iter().filter_map(filter).collect(),
                text: pipeline[0],
                segment,
                action,
                body,
            }),
        }
    }

    let browse = looped.is_none() && !parsed.is_empty() && parsed.iter().all(|p| matches!(p.action, Action::Browse(_)));
    let mut shell = Shell {
        command: Some(command.into()),
        description: description.map(str::trim).filter(|d| !d.is_empty()).map(Into::into),
        exit_code,
        no_match,
        cwd,
        intent: parsed.first().map_or(Intent::Other, |p| p.action.intent()),
        head: String::new(),
        filters: Vec::new(),
        more: parsed.len().saturating_sub(1),
        looped: looped.is_some(),
        target: None,
        append: false,
        script: None,
        steps: Vec::new(),
    };
    if browse {
        let count = parsed.iter().map(|p| match &p.action { Action::Browse(specs) => specs.len(), _ => 0 }).sum::<usize>();
        let output = output.filter(|_| count == 1);
        for p in parsed {
            let Action::Browse(specs) = p.action else { continue };
            let last = specs.len() - 1;
            for (index, spec) in specs.into_iter().enumerate() {
                shell.steps.push(Step {
                    op: spec.build(output, &p.filters),
                    program: p.segment.program().into(),
                    filters: if index == last { p.filters.clone() } else { Vec::new() },
                });
            }
        }
        return Op::Shell(shell);
    }
    let Some(first) = parsed.first() else {
        shell.head = command.lines().next().unwrap_or_default().trim().into();
        return Op::Shell(shell);
    };
    shell.filters = first.filters.clone();
    shell.head = match looped {
        Some(start) => head[start..].lines().next().unwrap_or_default().trim().into(),
        None => first.segment.head(first.text),
    };
    if first.segment.program() == "git" {
        shell.cwd = shell.cwd.take().or_else(|| option(&first.segment.args, "-C").map(Into::into));
    }
    if let Some((target, append)) = parsed.iter().find_map(|p| p.target.clone()) {
        shell.target = Some(target);
        shell.append = append;
    }
    shell.script = parsed.iter().find_map(|p| {
        let lang = script_lang(p.segment.program());
        match (&p.body, p.segment.inline_code()) {
            (Some(body), _) => Some(Script::new(lang, body)),
            (None, Some(code)) => Some(Script::new(lang, code)),
            _ => None,
        }
    });
    Op::Shell(shell)
}

struct Parsed<'a> {
    text: &'a str,
    /// 写入或原地修改的文件，以及是否追加。
    target: Option<(String, bool)>,
    segment: Segment,
    action: Action,
    filters: Vec<Filter>,
    body: Option<String>,
}

enum Action {
    Noise,
    Control,
    Browse(Vec<Spec>),
    Run(Intent),
}

impl Action {
    fn intent(&self) -> Intent {
        match self {
            Action::Browse(specs) => match specs.first() {
                Some(Spec::Read { .. }) => Intent::Read,
                Some(Spec::Search { .. }) => Intent::Search,
                Some(Spec::Find { .. }) => Intent::Find,
                _ => Intent::List,
            },
            Action::Run(intent) => *intent,
            _ => Intent::Other,
        }
    }
}

enum Spec {
    Read { path: String, start: Option<u64>, end: Option<u64>, tail: Option<u64>, whole: bool },
    /// `counted` 是 `-c` 计数模式，输出不是命中行。
    Search { pattern: String, paths: Vec<String>, files_only: bool, counted: bool },
    Find { pattern: Option<String>, paths: Vec<String> },
    List { paths: Vec<String> },
}

impl Spec {
    /// 输出经过管道筛选后不再是文件原文：读取、查找、列目录只用未筛选的输出统计；
    /// 搜索的筛选保留命中行格式，计数后的输出不能再按命中解析。
    fn build(self, output: Option<&str>, filters: &[Filter]) -> Op {
        let unfiltered = output.filter(|_| filters.is_empty());
        match self {
            Spec::Read { path, start, end, tail, whole } => {
                let lines = unfiltered.map(|output| line_count(output) as u64);
                match (start, lines) {
                    (Some(start), Some(lines)) => Op::read_range(&path, start, lines, whole.then_some(lines)),
                    (Some(start), None) if !whole => Op::Read { path, start: Some(start), end, total: None, tail: None },
                    _ => Op::Read { path, start: None, end: None, total: None, tail },
                }
            }
            Spec::Search { pattern, paths, files_only, counted } => browse::search(&pattern, paths, files_only,
                output.filter(|_| !counted && !filters.contains(&Filter::Count))),
            Spec::Find { pattern, paths } => browse::find(pattern.as_deref(), paths, unfiltered),
            Spec::List { paths } => browse::list(paths, unfiltered),
        }
    }
}

/// 写入目标：重定向、`tee` 的文件或 `sed -i` 修改的文件。
fn written(segment: &Segment, rest: &[Segment]) -> Option<(String, bool)> {
    if let Some(output) = rest.iter().rev().chain(std::iter::once(segment)).find(|stage| stage.target.is_some()) {
        return output.target.as_ref().map(|target| (target.clone(), output.append));
    }
    let tee = std::iter::once(segment).chain(rest).find(|s| s.program() == "tee");
    let file = match tee {
        Some(tee) => tee.positional(&[]).first().copied(),
        None => segment.positional(&["-e", "-f"]).last().copied(),
    };
    file.map(|file| (file.to_owned(), tee.is_some_and(|tee| tee.has_flag('a', "--append"))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn parse(command: &str, output: Option<&str>) -> Value {
        json!(shell(command, None, None, output))
    }

    #[test]
    fn browse_commands_become_file_steps() {
        let search = parse(r#"cd /repo && grep -rn "A\|B" src-tauri/src 2>/dev/null | grep -v "tests.rs" | head -20"#, Some("src-tauri/src/a.rs:3:A\n"));
        assert_eq!(search["cwd"], "/repo");
        assert_eq!(search["steps"], json!([{"kind":"search","pattern":"A\\|B","paths":["src-tauri/src"],"matches":1,"files":1,
            "hits":[{"path":"src-tauri/src/a.rs","lines":[{"line":3,"text":"A"}]}],"program":"grep",
            "filters":[{"kind":"exclude","value":"tests.rs"},{"kind":"head","value":20}]}]));

        let steps = parse("sed -n 60,130p a/types.go; cat a.ts b.ts; rg --files -g '*.rs' -g '!target' src; ls -la docs", Some("x"))["steps"].clone();
        assert_eq!(steps.as_array().unwrap().iter().map(|s| s["kind"].as_str().unwrap()).collect::<Vec<_>>(), ["read", "read", "read", "find", "list"]);
        assert_eq!((steps[0]["start"].clone(), steps[0]["end"].clone(), steps[3]["pattern"].clone()), (json!(60), json!(130), json!("*.rs")));

        assert!(parse("cat a.rs | wc -l", Some("100"))["steps"][0].get("end").is_none());
        assert!(parse("rg -c x src/a.rs", Some("11"))["steps"][0].get("matches").is_none());
        let read = parse("tail -n 5 AGENTS.md", Some("a\nb"));
        assert_eq!(read["steps"][0]["tail"], 5);
        assert_eq!(parse("cat src/a.ts", Some("1\n2\n3\n"))["steps"][0], json!({"kind":"read","path":"src/a.ts","start":1,"end":3,"total":3,"program":"cat"}));
    }

    #[test]
    fn other_commands_keep_a_clean_head() {
        let test = parse(r#"export PATH="$HOME/.cargo/bin:$PATH" && cargo test --offline 2>&1 | grep -E "FAILED|result" | tail -40"#, None);
        assert_eq!((test["intent"].clone(), test["head"].clone()), (json!("test"), json!("cargo test --offline")));
        assert_eq!(test["filters"], json!([{"kind":"include","value":"FAILED|result"},{"kind":"tail","value":40}]));

        let git = parse("git -C /Users/me/repo diff --stat HEAD && wc -l a.rs", None);
        assert_eq!((git["intent"].clone(), git["head"].clone(), git["cwd"].clone(), git["more"].clone()), (json!("git_diff"), json!("git diff --stat HEAD"), json!("/Users/me/repo"), json!(1)));

        let script = parse("cd ~/p && python3 -I - \"$F\" <<'EOF'\nimport json\n# 统计\nrows = load()\nEOF", None);
        assert_eq!((script["intent"].clone(), script["head"].clone()), (json!("script"), json!(r#"python3 -I - "$F""#)));
        assert_eq!(script["script"], json!({"lang":"Python","lines":3,"first":"rows = load()","body":"import json\n# 统计\nrows = load()"}));

        let write = parse("S=/tmp/x && cat > $S/a.mjs <<'EOF'\nconst a = 1;\nEOF\nls $S", None);
        assert_eq!((write["intent"].clone(), write["target"].clone(), write["more"].clone()), (json!("write"), json!("/tmp/x/a.mjs"), json!(1)));
        assert_eq!(parse("cat >> AGENTS.md <<'EOF'\n- x\nEOF", None)["append"], true);

        let looped = parse("D=icons; for f in $D/a.svg; do cat $f | head; done", None);
        assert_eq!((looped["intent"].clone(), looped["looped"].clone(), looped["head"].clone()), (json!("read"), json!(true), json!("for f in $D/a.svg; do cat $f | head; done")));
        assert_eq!(parse("S=/tmp/x && export T=$S/y; cat $S/a.ts ${T}/b.ts", None)["steps"], json!([
            {"kind":"read","path":"/tmp/x/a.ts","program":"cat"}, {"kind":"read","path":"/tmp/x/y/b.ts","program":"cat"}]));
        assert_eq!(parse("APP=$(pgrep -f \"x y\"); ps -p $APP", None)["intent"], "process");
        assert_eq!(parse("cat a.txt | tee b.txt", None)["target"], "b.txt");
        assert_eq!(parse("ls > files.txt", None)["intent"], "write");
        let redirected = parse("cat input.txt | sort >> output.txt", None);
        assert_eq!((redirected["target"].clone(), redirected["append"].clone()), (json!("output.txt"), json!(true)));
        assert_eq!(parse("cat input.txt |& sort >> output.txt", None)["target"], "output.txt");
        assert_eq!(json!(shell("rg -n x fixture.txt", None, Some(1), Some("")))["no_match"], true);
        assert!(json!(shell("rg -n x fixture.txt | awk 'BEGIN { exit 1 }'", None, Some(1), Some(""))).get("no_match").is_none());
        assert_eq!(parse("printf '%s\\n' '<<EOF'\nls src", None)["steps"][0]["paths"], json!(["src"]));
        assert_eq!(parse("P=a.txt; cat '${P}' ${P} $(printf '%s' b.txt)", None)["steps"].as_array().unwrap().iter()
            .map(|step| step["path"].as_str().unwrap()).collect::<Vec<_>>(), ["${P}", "a.txt", "$(printf %s b.txt)"]);
    }
}
