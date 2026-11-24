//! shell 命令解析：去掉 `cd`、环境变量、重定向等噪音并给每一步归类；只读、搜、找、列的命令拆成文件操作。
mod classify;
mod syntax;

use super::{browse, line_count, Op, Script};
use classify::{classify, filter, is_pure_filter, is_safe_setup, unwrap_substitution, Segment};
use serde::Serialize;
use std::{collections::HashSet, ops::Not};
use syntax::{heredocs, pipelines, MAX_EXPANSION_BYTES};

const MAX_SUBSTITUTION_DEPTH: u8 = 8;

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
    exit_one: Option<ExitOne>,
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
    /// 多步命令逐步列出；`related` 是主动作之外值得在标题提示的意图。
    #[serde(skip_serializing_if = "Vec::is_empty")]
    actions: Vec<CommandStep>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    related: Vec<Intent>,
    /// 含 `if`/`case`、`[`/`test` 或 `||`：列出的步骤不一定都执行过。
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

#[derive(Serialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
enum ExitOne {
    NoMatch,
    Differences,
    SilentDifferences,
}

impl ExitOne {
    fn outcome(self) -> Outcome {
        match self {
            Self::NoMatch => Outcome::NoMatch,
            Self::Differences | Self::SilentDifferences => Outcome::Differences,
        }
    }

    fn evidence(self) -> CommandOutput {
        match self {
            Self::NoMatch | Self::SilentDifferences => CommandOutput::Empty,
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
        } else if has_diff_output(text) {
            Some(Self::Diff)
        } else {
            None
        }
    }
}

fn is_normal_diff_hunk(line: &str) -> bool {
    let Some(op) = line.find(['a', 'c', 'd']) else { return false };
    let range = |part: &str| {
        let mut parts = part.split(',');
        let Some(first) = parts.next() else { return false };
        let valid = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
        valid(first) && parts.next().is_none_or(valid) && parts.next().is_none()
    };
    range(&line[..op]) && range(&line[op + 1..])
}

fn is_cmp_long(text: &str) -> bool {
    let mut any = false;
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let mut parts = line.split_whitespace();
        let (Some(pos), Some(a), Some(b), None) = (parts.next(), parts.next(), parts.next(), parts.next()) else { return false };
        let octal = |s: &str| !s.is_empty() && s.len() <= 3 && s.bytes().all(|c| (b'0'..=b'7').contains(&c));
        if pos.parse::<u64>().ok().filter(|&n| n > 0).is_none() || !octal(a) || !octal(b) {
            return false;
        }
        any = true;
    }
    any
}

fn has_diff_output(text: &str) -> bool {
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        if line.starts_with("--- ") && lines.peek().is_some_and(|next| next.starts_with("+++ ")) {
            lines.next();
            if lines.peek().is_some_and(|next| next.starts_with("@@ -") && next.contains(" +")) {
                return true;
            }
        }
        if is_normal_diff_hunk(line) && lines.peek().is_some_and(|next| next.starts_with("< ") || next.starts_with("> ")) {
            return true;
        }
        if line.starts_with("Only in ") && line.contains(": ")
            || (line.starts_with("Files ") || line.starts_with("Binary files ")) && line.ends_with(" differ")
            || line.contains(" differ: char ") || line.contains(" differ: byte ")
            || line.starts_with("cmp: EOF on ")
        {
            return true;
        }
    }
    is_cmp_long(text)
}

#[derive(Serialize, Clone, Debug, PartialEq)]
struct CommandStep {
    command: String,
    intent: Intent,
    #[serde(skip_serializing_if = "Option::is_none")]
    target: Option<String>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
struct Step {
    #[serde(flatten)]
    op: Op,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    filters: Vec<Filter>,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
enum Intent {
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
            Self::Mkdir | Self::GitLog | Self::GitRead => 2,
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
enum Filter {
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
    pub(super) fn priority(&self) -> u8 {
        self.intent.priority()
    }

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

enum Work<'a> {
    Pipeline { stages: Vec<&'a str>, last: bool, and_next: bool, substituted: bool, depth: u8 },
    RestoreVars { len: usize, clear: String },
}

pub(crate) fn shell(command: &str, description: Option<&str>, exit_code: Option<i64>, output: Option<&str>) -> Op {
    let (head, bodies) = heredocs(command);
    let mut bodies = bodies.into_iter();
    let mut parsed: Vec<Parsed> = Vec::new();
    let mut vars = Vec::new();
    let mut expansion_budget = MAX_EXPANSION_BYTES;
    // 第一个控制结构：`Some(true)` 是循环，`Some(false)` 是 if/case/[。
    let mut control = None;
    let (outer, mut guarded) = pipelines(&head);
    let mut fallible_prereq = false;
    let outer_len = outer.len();
    let mut work: Vec<Work<'_>> = outer.into_iter().enumerate().rev()
        .map(|(index, (stages, and_next))| Work::Pipeline { stages, last: index + 1 == outer_len, and_next, substituted: false, depth: 0 })
        .collect();
    while let Some(item) = work.pop() {
        let Work::Pipeline { stages: pipeline, last: last_pipeline, and_next, substituted, depth } = item else {
            if let Work::RestoreVars { len, clear } = item {
                vars.truncate(len);
                vars.retain(|(name, _)| name != &clear);
            }
            continue;
        };
        if pipeline.len() == 1 && depth < MAX_SUBSTITUTION_DEPTH {
            if let Some((earlier, assigned, inner)) = unwrap_substitution(pipeline[0], &vars, &mut expansion_budget) {
                vars.extend(earlier);
                work.push(Work::RestoreVars { len: vars.len(), clear: assigned });
                let (inner_pipes, inner_guarded) = pipelines(inner);
                guarded |= inner_guarded;
                let inner_len = inner_pipes.len();
                for (index, (stages, inner_and)) in inner_pipes.into_iter().enumerate().rev() {
                    let is_last = index + 1 == inner_len;
                    work.push(Work::Pipeline {
                        stages,
                        last: last_pipeline && is_last,
                        and_next: if is_last { and_next } else { inner_and },
                        substituted: true,
                        depth: depth + 1,
                    });
                }
                continue;
            }
        }
        let step = parse_pipeline(&pipeline, &vars, &mut expansion_budget, &mut bodies, last_pipeline, substituted);
        match step.action {
            Action::Noise => {
                if and_next && !is_safe_setup(&step.segment) {
                    fallible_prereq = true;
                }
                vars.extend(step.segment.assignments());
            }
            Action::Control => {
                if control.is_none() && matches!(step.segment.program(), "for" | "while" | "until" | "if" | "case" | "[" | "[[" | "test") {
                    control = Some(matches!(step.segment.program(), "for" | "while" | "until"));
                }
            }
            _ => parsed.push(step),
        }
    }

    let read_only = !parsed.is_empty() && parsed.iter().all(|p| matches!(p.action, Action::Browse(_)));
    let browse = control.is_none() && read_only;
    let exit_one = match parsed.as_slice() {
        [step] if control.is_none() && !guarded && !fallible_prereq && step.last_pipeline && browse && step.ends_in_search => Some(ExitOne::NoMatch),
        [step] if control.is_none() && !guarded && !fallible_prereq && step.last_pipeline && step.stages.len() == 1
            && matches!(step.action.intent(), Intent::Diff | Intent::GitDiff) => {
            Some(if step.segment.is_silent_diff() { ExitOne::SilentDifferences } else { ExitOne::Differences })
        }
        _ => None,
    };
    let outcome = exit_one.filter(|exit_one| {
        exit_code == Some(1) && output.and_then(CommandOutput::recognize) == Some(exit_one.evidence())
    }).map(ExitOne::outcome);
    let lead = select_lead(&parsed);
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
        looped: control == Some(true),
        target: None,
        append: false,
        script: None,
        steps: Vec::new(),
        actions: Vec::new(),
        related: Vec::new(),
        conditional: control == Some(false) || guarded,
    };
    if browse {
        shell.steps = build_browse_steps(parsed, output);
        return Op::Shell(shell);
    }
    let Some(first) = lead else {
        shell.head = command.lines().next().unwrap_or_default().trim().into();
        return Op::Shell(shell);
    };
    shell.head = first.stages[first.main_stage].clone();
    if let Some((target, append)) = &first.target {
        shell.target = Some(target.clone());
        shell.append = *append;
    }
    shell.script = first.script.clone();
    if parsed.len() > 1 {
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

fn parse_pipeline(
    pipeline: &[&str],
    vars: &[(String, String)],
    expansion_budget: &mut usize,
    bodies: &mut std::vec::IntoIter<String>,
    last_pipeline: bool,
    substituted: bool,
) -> Parsed {
    let mut segment = Segment::parse(pipeline[0], vars, expansion_budget);
    segment.substituted = substituted;
    let rest: Vec<_> = pipeline[1..].iter().map(|text| Segment::parse(text, vars, expansion_budget)).collect();
    let mut stages = std::iter::once(&segment).chain(&rest).map(|stage| (stage, (0..stage.heredocs).filter_map(|_| bodies.next()).last()));
    let (first, body) = stages.next().expect("pipeline has a head");
    let mut action = classify(first, body.is_some());
    let mut main_stage = 0;
    let mut script = first.script(body.as_deref());
    let mut sed_ranged = false;
    let preserves_lines = (first.program() == "cat" && !first.has_flag('s', "--squeeze-blank"))
        || (first.program() == "nl" && first.args.iter().any(|arg| arg == "-ba" || arg == "--body-numbering=a"));
    // 管道以优先级最高的非筛选段为主；浏览后接其他程序就不再只读，`curl … | sh` 以执行为主。
    for (index, (stage, body)) in stages.enumerate() {
        script = script.or_else(|| stage.script(body.as_deref()));
        if is_pure_filter(stage) || stage.program() == "tee" || matches!(action, Action::Control) {
            // 仅在紧邻 1:1 行映射的读取段（`cat`/`nl -ba`）后且无后续管段与输入重定向时，`sed -n 'a,bp'` 才对应源文件行号。
            if index == 0 && rest.len() == 1 && preserves_lines && !stage.has_input_redirect {
                if let (Action::Browse(specs), Some(((start, end), files))) = (&mut action, stage.sed_read_range()) {
                    if files.is_empty() {
                        if let [Spec::Read { start: s, end: e, tail: None, whole, .. }] = specs.as_mut_slice() {
                            if s.is_none() || *whole {
                                *s = Some(start);
                                *e = end;
                                *whole = false;
                                sed_ranged = true;
                            }
                        }
                    }
                }
            }
            continue;
        }
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
    } else if written.is_some() && matches!(action, Action::Noise | Action::Browse(_) | Action::Run(
        Intent::Transform | Intent::Inspect | Intent::Count | Intent::Diff | Intent::GitDiff | Intent::GitLog | Intent::GitRead |
        Intent::Git | Intent::Github | Intent::Process
    )) {
        // 只有产出内容的命令，存下来的文件才是目的；运行、构建等重定向的只是日志。
        action = Action::Run(Intent::Write);
    }
    Parsed {
        last_pipeline,
        has_pipeline_rest: !rest.is_empty(),
        order_preserved: rest.is_empty() || sed_ranged,
        ends_in_search: std::iter::once(&segment).chain(&rest).rev()
            .find(|stage| !matches!(stage.program(), "head" | "tail"))
            .is_some_and(Segment::is_search),
        target: written,
        filters: rest.iter().filter_map(filter).collect(),
        main_stage,
        stages: pipeline.iter().copied().zip(std::iter::once(&segment).chain(&rest))
            .map(|(text, stage)| stage.head(text)).collect(),
        segment,
        action,
        script,
    }
}

/// 主动作取优先级最高的步骤，同级取最先出现的；`X=$(…)` 和随后被运行的脚本文件只在没有别的步骤时才当主动作。
fn select_lead(parsed: &[Parsed]) -> Option<&Parsed> {
    let mut later_executed: HashSet<&str> = HashSet::new();
    let mut executed = vec![false; parsed.len()];
    for (index, step) in parsed.iter().enumerate().rev() {
        if let Some((path, _)) = &step.target {
            executed[index] = later_executed.contains(path.trim_start_matches("./"));
        }
        if matches!(step.action, Action::Run(Intent::Run | Intent::Script)) {
            if let Some(file) = step.segment.executed_file() {
                later_executed.insert(file.trim_start_matches("./"));
            }
        }
    }
    parsed.iter().enumerate().rev().max_by_key(|(index, step)| {
        let intent = step.action.intent();
        (!step.segment.substituted && !executed[*index], intent.priority(), step.target.is_some(), step.script.is_some())
    }).map(|(_, step)| step)
}

fn build_browse_steps(parsed: Vec<Parsed>, output: Option<&str>) -> Vec<Step> {
    let count = parsed.iter().map(|p| match &p.action { Action::Browse(specs) => specs.len(), _ => 0 }).sum::<usize>();
    let output = output.filter(|_| count == 1);
    let mut steps = Vec::with_capacity(count);
    for p in parsed {
        let Action::Browse(specs) = p.action else { continue };
        let last = specs.len() - 1;
        for (index, spec) in specs.into_iter().enumerate() {
            steps.push(Step {
                op: spec.build(output, &p.filters, p.has_pipeline_rest, p.order_preserved),
                filters: if index == last { p.filters.clone() } else { Vec::new() },
            });
        }
    }
    steps
}

struct Parsed {
    main_stage: usize,
    stages: Vec<String>,
    last_pipeline: bool,
    has_pipeline_rest: bool,
    order_preserved: bool,
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
    Search { pattern: String, paths: Vec<String>, files_only: bool, counted: bool, mode: browse::SearchMode },
    Find { pattern: Option<String>, paths: Vec<String> },
    List { paths: Vec<String> },
}

impl Spec {
    /// 输出经过下游管道后不再是原始文件或目录内容；搜索的筛选保留命中行格式，计数后的输出不能再按命中解析。
    fn build(self, output: Option<&str>, filters: &[Filter], has_pipeline_rest: bool, order_preserved: bool) -> Op {
        let unfiltered = output.filter(|_| !has_pipeline_rest);
        match self {
            Spec::Read { path, start, end, tail, whole } => {
                let lines = unfiltered.map(|output| line_count(output) as u64);
                match (start, lines) {
                    (Some(start), Some(lines)) => Op::read_range(&path, start, lines, whole.then_some(lines)),
                    (Some(start), None) if !whole && order_preserved => Op::Read { path, start: Some(start), end, total: None, tail: None },
                    _ => Op::Read { path, start: None, end: None, total: None, tail: tail.filter(|_| order_preserved) },
                }
            }
            Spec::Search { pattern, paths, files_only, counted, mode } => browse::search_with_mode(&pattern, paths, files_only, mode,
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
        return stage.positional(&[]).into_iter().find(|file| !matches!(*file, "-" | "/dev/null"))
            .map(|file| (file.into(), stage.has_flag('a', "--append")));
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
            "hits":[{"path":"src-tauri/src/a.rs","lines":[{"line":3,"text":"A"}]}],
            "filters":[{"kind":"exclude","value":"tests.rs"},{"kind":"head","value":20}]}]));

        let steps = parse("sed -n 60,130p a/types.go; cat a.ts b.ts; rg --files -g '*.rs' -g '!target' src; ls -la docs", Some("x"))["steps"].clone();
        assert_eq!(steps.as_array().unwrap().iter().map(|s| s["kind"].as_str().unwrap()).collect::<Vec<_>>(), ["read", "read", "read", "find", "list"]);
        assert_eq!((steps[0]["start"].clone(), steps[0]["end"].clone(), steps[3]["pattern"].clone()), (json!(60), json!(130), json!("*.rs")));

        assert!(parse("cat a.rs | wc -l", Some("100"))["steps"][0].get("end").is_none());
        assert!(parse("rg -c x src/a.rs", Some("11"))["steps"][0].get("matches").is_none());
        assert_eq!(parse("tail -n 5 AGENTS.md", Some("a\nb"))["steps"][0]["tail"], 5);
        assert_eq!(parse("tail -n +20 file.txt", None)["steps"][0]["start"], 20);
        assert_eq!(parse("cat src/a.ts", Some("1\n2\n3\n"))["steps"][0], json!({"kind":"read","path":"src/a.ts","start":1,"end":3,"total":3}));
        let ranged = parse("nl -ba src/a.rs | sed -n '45,190p'", Some("45\ta\n46\tb\n"))["steps"][0].clone();
        assert_eq!((ranged["start"].clone(), ranged["end"].clone(), ranged.get("total")), (json!(45), json!(190), None));
        let sorted = parse("cat src/a.rs | sort | sed -n '2,3p'", Some("x\ny\n"))["steps"][0].clone();
        assert!(sorted.get("start").is_none() && sorted.get("end").is_none() && sorted.get("total").is_none());
        let reordered = parse("cat src/a.rs | sed -n '2,3p' | sort", Some("b\na\n"))["steps"][0].clone();
        assert!(reordered.get("start").is_none() && reordered.get("end").is_none());
        let sed_sorted = parse("sed -n '2,3p' src/a.rs | sort", Some("b\na\n"))["steps"][0].clone();
        assert!(sed_sorted.get("start").is_none() && sed_sorted.get("end").is_none());
        let sed_redir = parse("cat src/a.rs | sed -n '2,3p' < src/b.rs", Some("b\nc\n"))["steps"][0].clone();
        assert!(sed_redir.get("start").is_none() && sed_redir.get("end").is_none());
        assert_eq!(parse("setopt nullglob && ls docs", None)["steps"][0]["kind"], "list");
        assert_eq!(parse("sed -n -e '2,4p' src/main.rs", None)["steps"][0]["start"], 2);

        let no_heading = parse("rg --heading --no-heading x d", Some("d/a.txt:x\nd/b.txt:x\n"));
        assert_eq!((no_heading["steps"][0]["matches"].clone(), no_heading["steps"][0]["files"].clone()), (json!(2), json!(2)));
        assert_eq!(parse("rg --heading '.*' a.txt", Some("first\nsecond\n"))["steps"][0]["matches"], 2);
        let no_filename = parse("rg --heading -I -N '.*' src", Some("config.rs:\n  Line 1: hello\n"))["steps"][0].clone();
        assert!(no_filename.get("hits").is_none());
        let unnumbered_heading = parse("rg --heading -H -N literal a.txt", Some("a.txt\n1:literal\n2:literal\n"))["steps"][0].clone();
        assert_eq!((unnumbered_heading["matches"].clone(), unnumbered_heading["hits"][0]["lines"][0]["text"].clone()), (json!(2), json!("1:literal")));
        assert!(unnumbered_heading["hits"][0]["lines"][0].get("line").is_none());
        let numbered_heading = parse("rg --heading -H -n '^#' AGENTS.md", Some("AGENTS.md\n1:# a\n4:# b\n"))["steps"][0].clone();
        assert_eq!((numbered_heading["matches"].clone(), numbered_heading["hits"][0]["lines"][1]["line"].clone()), (json!(2), json!(4)));

        let git_grep = parse("git -c color.ui=false grep -e needle -- src", Some("src/a.rs:needle\n"));
        assert_eq!((git_grep["steps"][0]["pattern"].clone(), git_grep["steps"][0]["paths"].clone()), (json!("needle"), json!(["src"])));
        assert_eq!(parse("git grep -c needle src", None)["steps"][0]["pattern"], "needle");
    }

    #[test]
    fn command_intents_and_heads() {
        let test = parse(r#"export PATH="$HOME/.cargo/bin:$PATH" && cargo test --offline 2>&1 | grep -E "FAILED|result" | tail -40"#, None);
        assert_eq!((test["intent"].clone(), test["head"].clone()), (json!("test"), json!("cargo test --offline")));

        let git = parse("git -C /Users/me/repo diff HEAD && wc -l a.rs", None);
        assert_eq!((git["intent"].clone(), git["head"].clone(), git["more"].clone()), (json!("git_diff"), json!("git diff HEAD"), json!(1)));
        assert_eq!(parse("git diff --stat HEAD", None)["intent"], "git");
        assert_eq!(parse("git diff --stat -p", None)["intent"], "git_diff");
        assert_eq!(parse("git diff --exit-code", None)["intent"], "git_diff");
        assert_eq!(parse("git add -- src/main.rs", None)["intent"], "git_stage");
        assert_eq!(parse("git show HEAD:src/main.rs", None)["intent"], "git_read");
        assert_eq!(parse("git reset --soft HEAD~1 && git log -n 1", None)["intent"], "git_change");
        assert_eq!(parse("git add --dry-run file", None)["intent"], "inspect");
        assert_eq!(parse("git apply --check fix.patch", None)["intent"], "inspect");
        let git_flow = parse("git status && git add file && git commit -m test && git push", None);
        assert_eq!((git_flow["intent"].clone(), git_flow["related"].clone()), (json!("git_commit"), json!(["git_push", "git_stage"])));

        let tee = parse("cat a.txt | tee b.txt", None);
        assert_eq!((tee["intent"].clone(), tee["target"].clone()), (json!("write"), json!("b.txt")));
        assert_eq!(parse("echo hi | tee out.txt", None)["intent"], "write");
        assert_eq!(parse("cargo test | tee test.log", None)["intent"], "test");
        assert_eq!(parse("ls > files.txt", None)["intent"], "write");
        let redirected = parse("cat input.txt | sort >> output.txt", None);
        assert_eq!((redirected["target"].clone(), redirected["append"].clone()), (json!("output.txt"), json!(true)));
        assert_eq!(parse("cat input.txt |& sort >> output.txt", None)["target"], "output.txt");
        assert_eq!(parse("find src -name -name -delete", None)["intent"], "remove");
        assert_eq!(parse("find src -name -delete -print", None)["intent"], "find");
        assert_eq!(parse("node -r ./preload.js --test", None)["intent"], "test");
        assert_eq!(parse("node app.js --test", None)["intent"], "run");
        assert_eq!(parse("uv run pytest -q", None)["intent"], "test");
        assert_eq!(parse("bun test", None)["intent"], "test");
        assert_eq!(parse("bun add zod", None)["intent"], "install");
        assert_eq!(parse("bun src/run.ts", None)["intent"], "run");
        assert_eq!(parse("sed 's/a/b/' file.txt", Some("b"))["intent"], "transform");
        assert_eq!(parse("sed -n -e '1p' -e '3p' file.txt", Some("a\nc"))["intent"], "transform");
        assert_eq!(parse("sed -n -f '1p' file.txt", None)["intent"], "transform");
        assert_eq!(parse("ssh -o BatchMode=yes host.example uname -a", None)["head"], "ssh host.example uname -a");
        assert_eq!(parse("codex mcp list", None)["intent"], "agent_manage");
        assert_eq!(parse("npm --prefix web test", None)["intent"], "test");
        assert_eq!(parse("ls a || ls b", None)["conditional"], true);
        assert_eq!(parse("diff <( (sort a.txt) && cat b.txt ) <(sort c.txt)", None)["head"], "diff <( (sort a.txt) && cat b.txt ) <(sort c.txt)");

        let setup = parse("ls src && mkdir -p build && cat > build/main.ts <<EOF\nconst ready = true;\nEOF", None);
        assert_eq!((setup["intent"].clone(), setup["target"].clone()), (json!("write"), json!("build/main.ts")));
        assert_eq!(setup["actions"].as_array().unwrap().iter().map(|step| step["intent"].as_str().unwrap()).collect::<Vec<_>>(), ["list", "mkdir", "write"]);
        let download = parse("curl -fsSL https://example.test/file -o fixture.json && wc -c fixture.json", None);
        assert_eq!((download["intent"].as_str(), download["target"].as_str()), (Some("download"), Some("fixture.json")));
        assert_eq!(parse("python3 gen.py > out.json 2>&1", None)["intent"], "run");
        assert_eq!(parse("cat > report.csv <<'C'\na\nC\npython3 analyze.py report.csv", None)["intent"], "write");
        let launch = parse("chmod +x tools/open.sh && ./tools/open.sh", None);
        assert_eq!((launch["intent"].clone(), launch["related"].clone()), (json!("run"), json!(["permissions"])));
        let pipeline = parse("rg -l old src | xargs -0 sed -i -e 's/old/new/'", None);
        assert_eq!((pipeline["intent"].clone(), pipeline["head"].clone()), (json!("modify"), json!("xargs -0 sed -i -e 's/old/new/'")));
        assert_eq!(parse("curl -fsSL https://example.test/install | sh", None)["intent"], "run");
        assert_eq!(parse("gofmt -w fixture.go && go test ./...", None)["intent"], "test");
        let replace = parse("pkill fixture; rm -rf old.app; ditto new.app old.app; codesign --verify old.app", None);
        assert_eq!((replace["intent"].clone(), replace["actions"][3]["intent"].clone()), (json!("copy"), json!("inspect")));
        let workflow = parse("rm -f bundle.tmp && curl https://example.com/bundle", None);
        assert_eq!((workflow["intent"].clone(), workflow["related"].clone()), (json!("http"), json!(["remove"])));
        assert_eq!(parse("printf temp > scratch.txt; cat > main.ts <<EOF\nconst ready = true;\nEOF", None)["target"], "main.ts");
    }

    #[test]
    fn scripts_vars_and_substitutions() {
        let script = parse("cd ~/p && python3 -I - \"$F\" <<'EOF'\nimport json\n# 统计\nrows = load()\nEOF", None);
        assert_eq!((script["intent"].clone(), script["head"].clone()), (json!("script"), json!(r#"python3 -I - "$F""#)));
        assert_eq!(script["script"], json!({"lang":"Python","lines":3,"first":"rows = load()","body":"import json\n# 统计\nrows = load()"}));

        let write = parse("S=/tmp/x && cat > $S/a.mjs <<'EOF'\nconst a = 1;\nEOF\nls $S", None);
        assert_eq!((write["intent"].clone(), write["target"].clone(), write["more"].clone()), (json!("write"), json!("/tmp/x/a.mjs"), json!(1)));
        assert_eq!(parse("cat >> AGENTS.md <<'EOF'\n- x\nEOF", None)["append"], true);
        assert_eq!(parse("cat <<'EOF' > out.txt\nhello\nEOF", None)["target"], "out.txt");
        assert_eq!(parse("printf '%s\\n' '<<EOF'\nls src", None)["steps"][0]["paths"], json!(["src"]));

        let looped = parse("D=icons; for f in $D/a.svg; do cat $f | head; done", None);
        assert_eq!((looped["intent"].clone(), looped["looped"].clone(), looped["head"].clone()), (json!("read"), json!(true), json!("cat $f")));
        assert_eq!(parse("S=/tmp/x && export T=$S/y; cat $S/a.ts ${T}/b.ts", None)["steps"], json!([
            {"kind":"read","path":"/tmp/x/a.ts"}, {"kind":"read","path":"/tmp/x/y/b.ts"}]));
        assert_eq!(parse("P=a.txt; cat '${P}' ${P} $(printf '%s' b.txt)", None)["steps"].as_array().unwrap().iter()
            .map(|step| step["path"].as_str().unwrap()).collect::<Vec<_>>(), ["${P}", "a.txt", "$(printf %s b.txt)"]);

        assert_eq!(parse("APP=$(pgrep -f \"x y\"); ps -p $APP", None)["intent"], "process");
        let sub = parse("J=$(node -e 'const b = 1; console.log(b)')", None);
        assert_eq!((sub["head"].clone(), sub["script"]["body"].clone()), (json!("node -e"), json!("const b = 1; console.log(b)")));
        let piped_sub = parse("X=$(git status | head -3)", None);
        assert_eq!((piped_sub["intent"].clone(), piped_sub["head"].clone()), (json!("git"), json!("git status")));
        assert_eq!(parse("FOO=$(git rev-parse HEAD) cargo test", None)["intent"], "test");
        assert_eq!(parse(r#"X="$(echo a) $(echo b)" && ls src"#, None)["steps"][0]["kind"], "list");
        // 命令替换内赋值的变量不泄漏到外层；赋值求值时仍可见旧值，完成后覆盖外层旧值。
        let isolated: Vec<_> = parse("A=old.txt; X=$(B=inner.txt; cat $B); A=$(cat $A); cat $A $B", None)["steps"]
            .as_array().unwrap().iter().map(|step| step["path"].as_str().unwrap().to_string()).collect();
        assert_eq!(isolated, ["inner.txt", "old.txt", "$A", "$B"]);
        let big = "a".repeat(20_000);
        let capped = parse(&format!("A={big}; cat $A $A $A $A"), None)["steps"].clone();
        assert_eq!(capped[3]["path"], "$A");

        let pipe = parse("ls -1 | head -15 | python3 -I -c 'print(1)'", None);
        assert_eq!((pipe["intent"].clone(), pipe["script"]["body"].clone()), (json!("script"), json!("print(1)")));
        assert!(pipe.get("actions").is_none() && pipe.get("conditional").is_none());
        let uv_script = parse("uv run python -c 'print(1)'", None);
        assert_eq!((uv_script["head"].clone(), uv_script["script"]["lang"].clone(), uv_script["script"]["body"].clone()), (json!("uv run python -c"), json!("Python"), json!("print(1)")));
        assert_eq!(parse("printf x | cat <<A\none\nA\nuv run python <<B\nprint(2)\nB", None)["script"]["body"], "print(2)");
        let helper = parse("cat > run.py <<'PY'\nprint(1)\nPY\npython3 ./run.py", None);
        assert_eq!((helper["intent"].clone(), helper["head"].clone(), helper["script"]["body"].clone()), (json!("run"), json!("python3 ./run.py"), json!("print(1)")));
        assert_eq!(parse("ssh host python3 - <<'PY'\nprint(1)\nPY", None)["script"]["lang"], "Python");
    }

    #[test]
    fn exit_one_outcomes() {
        assert_eq!(json!(shell("rg -n x fixture.txt", None, Some(1), Some("")))["outcome"], "no_match");
        assert_eq!(json!(shell("cd /repo && rg --files -g '*.txt' | head -5", None, Some(1), Some("\n")))["outcome"], "no_match");
        assert_eq!(json!(shell("git grep needle", None, Some(1), Some("")))["outcome"], "no_match");
        assert!(json!(shell("cd /dev/null 2>/dev/null && rg -n x fixture.txt", None, Some(1), Some(""))).get("outcome").is_none());
        assert!(json!(shell("export 1=bad 2>/dev/null && rg -n x fixture.txt", None, Some(1), Some(""))).get("outcome").is_none());
        assert!(json!(shell("export 1=bad >/dev/null 2>&1 && rg -n x fixture.txt", None, Some(1), Some(""))).get("outcome").is_none());
        assert!(json!(shell("cd /dev/null >/dev/null 3>&1 2>&1 && rg -n x fixture.txt", None, Some(1), Some(""))).get("outcome").is_none());
        assert!(json!(shell("false && rg -n x fixture.txt", None, Some(1), Some(""))).get("outcome").is_none());
        assert!(json!(shell("[ -f fixture.txt ] && rg -n x fixture.txt", None, Some(1), Some(""))).get("outcome").is_none());
        assert!(json!(shell("rg -n x fixture.txt || true", None, Some(1), Some(""))).get("outcome").is_none());
        assert!(json!(shell("rg -n x fixture.txt; false", None, Some(1), Some(""))).get("outcome").is_none());
        assert!(json!(shell("rg -n x fixture.txt; cat other.txt", None, Some(1), Some(""))).get("outcome").is_none());
        assert!(json!(shell("rg -n x fixture.txt | awk 'BEGIN { exit 1 }'", None, Some(1), Some(""))).get("outcome").is_none());

        let diff = "--- earlier\n+++ later\n@@ -1 +1 @@\n-old\n+new\n";
        assert_eq!(json!(shell("diff -u earlier later", None, Some(1), Some(diff)))["outcome"], "differences");
        assert_eq!(json!(shell("diff earlier later", None, Some(1), Some("1c1\n< old\n---\n> new\n")))["outcome"], "differences");
        assert_eq!(json!(shell("cmp earlier later", None, Some(1), Some("earlier later differ: char 1, line 1\n")))["outcome"], "differences");
        assert_eq!(json!(shell("cmp -l earlier later", None, Some(1), Some("1 141 142\n")))["outcome"], "differences");
        assert_eq!(json!(shell("cmp -s earlier later", None, Some(1), Some("")))["outcome"], "differences");
        assert_eq!(json!(shell("git diff --quiet", None, Some(1), Some("")))["outcome"], "differences");
        assert_eq!(json!(shell("git diff --quiet --stat", None, Some(1), Some("")))["outcome"], "differences");
        assert!(json!(shell("diff -u earlier missing", None, Some(1), Some("missing file"))).get("outcome").is_none());
    }
}
