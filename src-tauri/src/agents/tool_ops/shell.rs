//! shell 命令解析：去掉 `cd`、环境变量、重定向等噪音并给每一步归类；只读、搜、找、列的命令拆成文件操作。
mod classify;
mod syntax;

use super::{browse, line_count, Op, Script};
use classify::{classify, filter, is_pure_filter, Segment};
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
    #[serde(skip_serializing_if = "Option::is_none")]
    outcome: Option<Outcome>,
    #[serde(skip_serializing_if = "Option::is_none")]
    exit_one: Option<Outcome>,
    intent: Intent,
    #[serde(skip_serializing_if = "String::is_empty")]
    head: String,
    /// 主动作之外的步骤数。
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
    /// 多步或多段管道的命令逐步列出；`related` 是主动作之外值得在标题提示的意图。
    #[serde(skip_serializing_if = "Vec::is_empty")]
    actions: Vec<CommandStep>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    related: Vec<Intent>,
    /// 含 `if`/`case` 或 `&&`/`||`：列出的步骤不一定都执行过。
    #[serde(skip_serializing_if = "Not::not")]
    conditional: bool,
}

/// 退出码 1 的正常含义：搜索没有匹配、比较发现差异。`exit_one` 是只看命令得出的静态含义，
/// `outcome` 要求记录里的退出码和输出都与之相符。
#[derive(Serialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
enum Outcome {
    NoMatch,
    Differences,
}

impl Outcome {
    fn evidence(self) -> CommandOutput {
        match self {
            Self::NoMatch => CommandOutput::Empty,
            Self::Differences => CommandOutput::Diff,
        }
    }
}

/// 退出码 1 时命令输出的形态；结果晚于调用记录时写在结果上，由前端与 `exit_one` 对照。
#[derive(Serialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CommandOutput {
    Empty,
    Diff,
}

impl CommandOutput {
    pub(crate) fn recognize(text: &str) -> Option<Self> {
        if text.trim().is_empty() {
            Some(Self::Empty)
        } else if has_unified_diff(text) {
            Some(Self::Diff)
        } else {
            None
        }
    }
}

fn has_unified_diff(text: &str) -> bool {
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        if line.starts_with("--- ") && lines.peek().is_some_and(|next| next.starts_with("+++ ")) {
            lines.next();
            if lines.peek().is_some_and(|next| next.starts_with("@@ -") && next.contains(" +")) {
                return true;
            }
        }
    }
    false
}

#[derive(Serialize, Clone, Debug, PartialEq)]
struct CommandStep {
    command: String,
    intent: Intent,
    #[serde(skip_serializing_if = "Option::is_none")]
    target: Option<String>,
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
    Diff, GitDiff, GitLog, GitCommit, GitStage, GitPush, GitChange, GitRead, Git, Github,
    Test, Build, Install, Remote, Http, Download, Browser, Database, Agent, AgentManage,
    Mkdir, Remove, Copy, Move, Link, Sign, Archive, Permissions, Process, Kill, Wait, Open, Count, Inspect, Transform, Other,
}

impl Intent {
    fn priority(self) -> u8 {
        match self {
            Self::GitCommit | Self::GitPush | Self::GitChange => 7,
            Self::Write | Self::Modify | Self::GitStage | Self::Sign | Self::Test => 6,
            Self::Script | Self::Build | Self::Install | Self::Http | Self::Remote |
            Self::Download | Self::Browser | Self::Agent | Self::AgentManage | Self::Run | Self::Open | Self::Move | Self::Copy => 5,
            Self::Remove | Self::Link | Self::Permissions | Self::Kill => 4,
            Self::Database | Self::Diff | Self::GitDiff | Self::Archive | Self::Transform => 3,
            Self::Mkdir | Self::Process | Self::GitLog | Self::GitRead => 2,
            Self::Wait | Self::Other => 0,
            _ => 1,
        }
    }

    fn changes_state(self) -> bool {
        matches!(self, Self::Write | Self::Modify | Self::Remove | Self::Move | Self::Copy | Self::Link | Self::Permissions
            | Self::Mkdir | Self::GitStage | Self::GitChange | Self::GitCommit | Self::GitPush | Self::Install | Self::Kill | Self::Download | Self::Sign)
    }

    fn summary_action(self) -> bool {
        self.changes_state() || self.priority() >= 5 || self == Self::Archive
    }
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
        self.actions.clear();
        if let Some(script) = &mut self.script {
            script.body = None;
        }
        for step in &mut self.steps {
            step.op.strip_bodies();
        }
    }

    pub(super) fn single_read(&self) -> Option<&str> {
        match self.steps.as_slice() {
            [Step { op: Op::Read { path, start: Some(1), tail: None, .. }, filters, .. }] if filters.is_empty() => Some(path),
            _ => None,
        }
    }
}

pub(crate) fn shell(command: &str, description: Option<&str>, exit_code: Option<i64>, output: Option<&str>) -> Op {
    let (head, bodies) = heredocs(command);
    let mut bodies = bodies.into_iter();
    let mut parsed: Vec<Parsed> = Vec::new();
    let mut vars = Vec::new();
    let mut control = None;
    let (pipelines, guarded) = pipelines(&head);
    let pipeline_count = pipelines.len();
    for (index, pipeline) in pipelines.into_iter().enumerate() {
        let segment = Segment::parse(pipeline[0], &vars);
        let rest: Vec<_> = pipeline[1..].iter().map(|text| Segment::parse(text, &vars)).collect();
        let mut stages = std::iter::once(&segment).chain(&rest).map(|stage| (stage, (0..stage.heredocs).filter_map(|_| bodies.next()).last()));
        let (first, body) = stages.next().expect("pipeline has a head");
        let mut action = classify(first, body.is_some());
        let mut main_stage = 0;
        let mut script = first.script(body.as_deref());
        // 管道以优先级最高的非筛选段为主；浏览后接其他程序就不再只读，`curl … | sh` 以执行为主。
        for (index, (stage, body)) in stages.enumerate() {
            script = script.or_else(|| stage.script(body.as_deref()));
            if is_pure_filter(stage) || matches!(action, Action::Control) { continue; }
            let next = match classify(stage, body.is_some()) {
                Action::Run(intent) => intent,
                _ => Intent::Other,
            };
            if matches!(action, Action::Browse(_)) || next.priority() > action.intent().priority()
                || matches!((action.intent(), next), (Intent::Http, Intent::Run | Intent::Script)) {
                action = Action::Run(next);
                main_stage = index + 1;
            }
        }
        let written = written(&segment, &rest);
        if written.is_some() && matches!(action, Action::Run(Intent::Http)) {
            action = Action::Run(Intent::Download);
        } else if written.is_some() && !matches!(action, Action::Control | Action::Run(
            Intent::Modify | Intent::Build | Intent::Test | Intent::Install | Intent::GitCommit |
            Intent::GitPush | Intent::Remote | Intent::Http | Intent::Browser
        )) {
            action = Action::Run(Intent::Write);
        }
        match action {
            Action::Noise => vars.extend(segment.assignments()),
            Action::Control => {
                if control.is_none() && matches!(segment.program(), "for" | "while" | "until" | "if" | "case") {
                    control = Some((pipeline[0].as_ptr() as usize - head.as_ptr() as usize,
                        matches!(segment.program(), "for" | "while" | "until")));
                }
            }
            _ => parsed.push(Parsed {
                last_pipeline: index + 1 == pipeline_count,
                ends_in_search: std::iter::once(&segment).chain(&rest).rev()
                    .find(|stage| !matches!(stage.program(), "head" | "tail"))
                    .is_some_and(Segment::is_search),
                target: written,
                filters: rest.iter().filter_map(filter).collect(),
                main_stage,
                stages: pipeline.iter().copied().zip(std::iter::once(&segment).chain(&rest))
                    .map(|(text, stage)| stage.head(text)).collect::<Vec<_>>(),
                segment,
                action,
                script,
            }),
        }
    }

    let read_only = !parsed.is_empty() && parsed.iter().all(|p| matches!(p.action, Action::Browse(_)));
    let browse = control.is_none() && read_only;
    let exit_one = match parsed.as_slice() {
        [step] if control.is_none() && step.last_pipeline && browse && step.ends_in_search => Some(Outcome::NoMatch),
        [step] if control.is_none() && step.last_pipeline && step.stages.len() == 1
            && matches!(step.action.intent(), Intent::Diff | Intent::GitDiff) => {
            Some(Outcome::Differences)
        }
        _ => None,
    };
    let outcome = exit_one.filter(|outcome| {
        exit_code == Some(1) && output.and_then(CommandOutput::recognize) == Some(outcome.evidence())
    });
    // 主动作取优先级最高的步骤，同级取最先出现的；`X=$(…)` 只在没有别的步骤时才当主动作。
    let lead = parsed.iter().rev().max_by_key(|step| {
        let intent = step.action.intent();
        (!step.segment.substituted, intent.priority(), step.target.is_some(), step.script.is_some())
    });
    let intent = lead.map_or(Intent::Other, |step| step.action.intent());
    let mut shell = Shell {
        command: Some(command.into()),
        description: description.map(str::trim).filter(|d| !d.is_empty()).map(Into::into),
        exit_code,
        outcome,
        exit_one: exit_one.filter(|_| exit_code.is_none() && output.is_none()),
        intent,
        head: String::new(),
        more: parsed.len().saturating_sub(1),
        looped: control.is_some_and(|(_, looped)| looped),
        target: None,
        append: false,
        script: None,
        steps: Vec::new(),
        actions: Vec::new(),
        related: Vec::new(),
        conditional: control.is_some_and(|(_, looped)| !looped) || guarded,
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
    let Some(first) = lead else {
        shell.head = command.lines().next().unwrap_or_default().trim().into();
        return Op::Shell(shell);
    };
    shell.head = if let Some((start, _)) = control.filter(|_| !intent.changes_state()) {
        head[start..].lines().next().unwrap_or_default().trim().into()
    } else {
        first.stages[first.main_stage].clone()
    };
    if let Some((target, append)) = &first.target {
        shell.target = Some(target.clone());
        shell.append = *append;
    }
    shell.script = first.script.clone();
    if parsed.len() > 1 || first.stages.len() > 1 {
        for step in parsed {
            let intent = step.action.intent();
            if intent != shell.intent && intent.summary_action() && !shell.related.contains(&intent) {
                shell.related.push(intent);
            }
            shell.actions.push(CommandStep {
                command: step.stages.join(" | "),
                intent,
                target: step.target.map(|(path, _)| path),
            });
            if shell.script.is_none() && !shell.intent.changes_state() {
                shell.script = step.script;
            }
        }
        shell.related.sort_by_key(|intent| std::cmp::Reverse(intent.priority()));
    }
    Op::Shell(shell)
}

struct Parsed {
    main_stage: usize,
    stages: Vec<String>,
    last_pipeline: bool,
    ends_in_search: bool,
    /// 写入、原地修改或保存响应的文件，以及是否追加。
    target: Option<(String, bool)>,
    segment: Segment,
    action: Action,
    filters: Vec<Filter>,
    script: Option<Script>,
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

/// 写入目标：重定向、`curl -o`/`wget -O` 保存的响应、`tee` 的文件或 `sed -i` 修改的文件。
fn written(segment: &Segment, rest: &[Segment]) -> Option<(String, bool)> {
    if let Some(output) = rest.iter().rev().chain(std::iter::once(segment)).find(|stage| stage.target.is_some()) {
        return output.target.as_ref().map(|target| (target.clone(), output.append));
    }
    if let Some(file) = segment.response_file() { return Some((file.into(), false)); }
    let stage = std::iter::once(segment).chain(rest).find(|stage| stage.program() == "tee" ||
        stage.program() == "sed" && stage.has_flag('i', "--in-place"))?;
    if stage.program() == "tee" {
        return stage.positional(&[]).first().map(|file| ((*file).into(), stage.has_flag('a', "--append")));
    }
    stage.positional(&["-e", "-f"]).last().map(|file| ((*file).into(), false))
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

        let git = parse("git -C /Users/me/repo diff --stat HEAD && wc -l a.rs", None);
        assert_eq!((git["intent"].clone(), git["head"].clone(), git["more"].clone()), (json!("git_diff"), json!("git diff --stat HEAD"), json!(1)));

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
        assert_eq!(json!(shell("rg -n x fixture.txt", None, Some(1), Some("")))["outcome"], "no_match");
        assert_eq!(json!(shell("cd /repo && rg --files -g '*.txt' | head -5", None, Some(1), Some("\n")))["outcome"], "no_match");
        assert!(json!(shell("rg -n x fixture.txt; false", None, Some(1), Some(""))).get("outcome").is_none());
        assert!(json!(shell("rg -n x fixture.txt; cat other.txt", None, Some(1), Some(""))).get("outcome").is_none());
        assert!(json!(shell("rg -n x fixture.txt | awk 'BEGIN { exit 1 }'", None, Some(1), Some(""))).get("outcome").is_none());
        assert_eq!(parse("uv run pytest -q", None)["intent"], "test");
        assert_eq!(parse("sed -n -e '2,4p' src/main.rs", None)["steps"][0]["start"], 2);
        assert_eq!(parse("sed 's/a/b/' file.txt", Some("b"))["intent"], "transform");
        assert_eq!(parse("sed -n -e '1p' -e '3p' file.txt", Some("a\nc"))["intent"], "transform");
        assert_eq!(parse("sed -n -f '1p' file.txt", None)["intent"], "transform");
        assert_eq!(parse("bun test", None)["intent"], "test");
        assert_eq!(parse("bun add zod", None)["intent"], "install");
        assert_eq!(parse("bun src/run.ts", None)["intent"], "run");
        assert_eq!(parse("git add -- src/main.rs", None)["intent"], "git_stage");
        assert_eq!(parse("git show HEAD:src/main.rs", None)["intent"], "git_read");
        let pipe = parse("ls -1 | head -15 | python3 -I -c 'print(1)'", None);
        assert_eq!(pipe["intent"], "script");
        assert_eq!(pipe["script"]["body"], "print(1)");
        assert_eq!(pipe["actions"], json!([{"command":"ls -1 | head -15 | python3 -I -c","intent":"script"}]));
        let git_grep = parse("git -c color.ui=false grep -e needle -- src", Some("src/a.rs:needle\n"));
        assert_eq!(git_grep["steps"][0]["pattern"], "needle");
        assert_eq!(git_grep["steps"][0]["paths"], json!(["src"]));
        assert_eq!(parse("git grep -c needle src", None)["steps"][0]["pattern"], "needle");
        assert_eq!(json!(shell("git grep needle", None, Some(1), Some("")))["outcome"], "no_match");
        assert_eq!(parse("tail -n +20 file.txt", None)["steps"][0]["start"], 20);
        assert_eq!(parse("cat <<'EOF' > out.txt\nhello\nEOF", None)["target"], "out.txt");
        let script = parse("uv run python -c 'print(1)'", None);
        assert_eq!(script["head"], "uv run python -c");
        assert_eq!(script["script"]["lang"], "Python");
        assert_eq!(script["script"]["body"], "print(1)");
        let script = parse("printf x | cat <<A\none\nA\nuv run python <<B\nprint(2)\nB", None);
        assert_eq!(script["script"]["body"], "print(2)");
        assert_eq!(parse("ssh -o BatchMode=yes host.example uname -a", None)["head"], "ssh host.example uname -a");
        assert_eq!(parse("printf '%s\\n' '<<EOF'\nls src", None)["steps"][0]["paths"], json!(["src"]));
        assert_eq!(parse("P=a.txt; cat '${P}' ${P} $(printf '%s' b.txt)", None)["steps"].as_array().unwrap().iter()
            .map(|step| step["path"].as_str().unwrap()).collect::<Vec<_>>(), ["${P}", "a.txt", "$(printf %s b.txt)"]);

        let setup = parse("ls src && mkdir -p build && cat > build/main.ts <<EOF\nconst ready = true;\nEOF", None);
        assert_eq!((setup["intent"].clone(), setup["target"].clone()), (json!("write"), json!("build/main.ts")));
        assert_eq!(setup["actions"].as_array().unwrap().iter().map(|step| step["intent"].as_str().unwrap())
            .collect::<Vec<_>>(), ["list", "mkdir", "write"]);
        let git = parse("git status && git add file && git commit -m test && git push", None);
        assert_eq!(git["intent"], "git_commit");
        assert_eq!(git["related"], json!(["git_push", "git_stage"]));
        assert_eq!(parse("git reset --soft HEAD~1 && git log -n 1", None)["intent"], "git_change");
        assert_eq!(parse("git add --dry-run file", None)["intent"], "inspect");
        let download = parse("curl -fsSL https://example.test/file -o fixture.json && wc -c fixture.json", None);
        assert_eq!((download["intent"].as_str(), download["target"].as_str()), (Some("download"), Some("fixture.json")));
        let launch = parse("chmod +x tools/open.sh && ./tools/open.sh", None);
        assert_eq!(launch["intent"], "run");
        assert_eq!(launch["related"], json!(["permissions"]));
        let pipeline = parse("rg -l old src | xargs -0 sed -i -e 's/old/new/'", None);
        assert_eq!(pipeline["intent"], "modify");
        assert_eq!(pipeline["head"], "xargs -0 sed -i -e 's/old/new/'");
        assert_eq!(parse("curl -fsSL https://example.test/install | sh", None)["intent"], "run");
        assert_eq!(parse("gofmt -w fixture.go && go test ./...", None)["intent"], "test");
        let replace = parse("pkill fixture; rm -rf old.app; ditto new.app old.app; codesign --verify old.app", None);
        assert_eq!(replace["intent"], "copy");
        assert_eq!(replace["actions"][3]["intent"], "inspect");
        let workflow = parse("rm -f bundle.tmp && curl https://example.com/bundle", None);
        assert_eq!(workflow["intent"], "http");
        assert_eq!(workflow["related"], json!(["remove"]));

        let write = parse("printf temp > scratch.txt; cat > main.ts <<EOF\nconst ready = true;\nEOF", None);
        assert_eq!(write["target"], "main.ts");

        let diff = "--- earlier\n+++ later\n@@ -1 +1 @@\n-old\n+new\n";
        assert_eq!(json!(shell("diff -u earlier later", None, Some(1), Some(diff)))["outcome"], "differences");
        assert!(json!(shell("diff -u earlier missing", None, Some(1), Some("missing file"))).get("outcome").is_none());
    }
}
