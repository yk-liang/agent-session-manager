//! 按程序给命令段归类：读、搜、找、列拆成文件操作，其余给出意图。
use super::super::browse::{is_file, LineMode, SearchMode};
use super::syntax::{substitution, tokens, unquote, Token};
use super::{Action, Filter, Intent, Script, Spec};

/// 一段命令的参数（已去引号、去掉前导环境变量和包装命令）与重定向。
pub(super) struct Segment {
    pub args: Vec<String>,
    /// 去掉的前导变量赋值。
    assigned: Vec<String>,
    /// 原文的词，保留引号，用于还原标题。
    raw: Vec<Raw>,
    pub target: Option<String>,
    pub append: bool,
    pub heredocs: usize,
    /// 重定向隐藏了 stderr（如 `2>/dev/null`）：前置命令失败时输出可能为空。
    suppresses_stderr: bool,
    /// 该段自带输入重定向（如 `< other.txt`），不从上游管道读入。
    pub has_input_redirect: bool,
    /// 该命令段由展开 `NAME=$(…)` 得到，选主动作时优先级低于直接执行的命令。
    pub substituted: bool,
}

enum Raw {
    Word(String),
    /// 指向 /dev/null、文件描述符复制和 heredoc 标记的重定向不进标题。
    Redirect { text: String, noise: bool },
}

impl Segment {
    pub fn parse(text: &str, vars: &[(String, String)], budget: &mut usize) -> Segment {
        let mut segment = Segment {
            args: Vec::new(), assigned: Vec::new(), raw: Vec::new(), target: None,
            append: false, heredocs: 0, suppresses_stderr: false, has_input_redirect: false, substituted: false,
        };
        let mut stdout_null = false;
        for token in tokens(text) {
            match token {
                Token::Word(word) => {
                    segment.args.push(unquote(word, vars, budget));
                    segment.raw.push(Raw::Word(word.into()));
                }
                Token::Redirect { op, target } => {
                    let target = target.map(|target| unquote(target, vars, budget));
                    let heredoc = op.starts_with("<<") && !op.starts_with("<<<");
                    let null = target.as_deref() == Some("/dev/null");
                    if op.starts_with('<') || op.starts_with("0<") {
                        segment.has_input_redirect = true;
                    }
                    if (op.starts_with('2') || op.starts_with("&>"))
                        && (null || op.ends_with(">&-") || (op == "2>&1" && stdout_null))
                    {
                        segment.suppresses_stderr = true;
                    }
                    if op.starts_with(['>', '&']) || op.starts_with("1>") {
                        stdout_null = null || op.ends_with(">&-");
                        if !null && target.is_some() {
                            segment.append = op.contains(">>");
                            segment.target = target.clone();
                        }
                    }
                    segment.heredocs += usize::from(heredoc && target.is_some());
                    let text = match &target {
                        Some(target) if !heredoc => format!("{op} {target}"),
                        _ => op.to_string(),
                    };
                    segment.raw.push(Raw::Redirect { text, noise: heredoc || null || target.is_none() });
                }
            }
        }
        let skip = segment.args.iter().take_while(|arg| is_assignment(arg)).count();
        segment.assigned = segment.args.drain(..skip).collect();
        // 包装命令不改变要做的事，参数和标题里都去掉；`command -v` 是查询，不是包装。
        let mut wrappers = 0;
        loop {
            let count = match segment.args.first().map(String::as_str) {
                Some("command") if segment.args.get(1).is_some_and(|arg| arg.starts_with('-')) => break,
                Some("sudo" | "time" | "env" | "command" | "exec" | "nohup" | "caffeinate" | "do" | "then" | "else" | "{") => 1,
                Some("timeout") if segment.args.len() > 2 => 2,
                Some(arg) if is_assignment(arg) => 1,
                _ => break,
            };
            segment.args.drain(..count);
            wrappers += count;
        }
        let mut drop_words = skip + wrappers;
        segment.raw.retain(|raw| match raw {
            Raw::Word(_) if drop_words > 0 => {
                drop_words -= 1;
                false
            }
            _ => true,
        });
        segment
    }

    /// 去掉前 `skip` 个参数后的命令：`uv run`、`npx` 这类包装转交给的程序。
    fn shifted(&self, skip: usize) -> Segment {
        Segment {
            args: self.args[skip..].to_vec(),
            assigned: Vec::new(),
            raw: Vec::new(),
            target: self.target.clone(),
            append: self.append,
            heredocs: self.heredocs,
            suppresses_stderr: self.suppresses_stderr,
            has_input_redirect: self.has_input_redirect,
            substituted: false,
        }
    }

    /// 连续展开最多 4 层包装命令（如 `uv run npx ...`），返回最终被执行程序的起始下标。
    fn runner_offset(&self) -> usize {
        let mut offset = 0;
        for _ in 0..4 {
            let Some(skip) = runner_skip_at(&self.args[offset..]) else { break };
            offset += skip;
        }
        offset
    }

    /// 单独一段的变量赋值（含 `export`），后续步骤里的 `$NAME` 据此展开。
    pub fn assignments(&self) -> Vec<(String, String)> {
        let args = match self.program() {
            "" => &self.assigned[..],
            "export" => &self.args[1..],
            _ => return Vec::new(),
        };
        args.iter().filter(|arg| is_assignment(arg))
            .filter_map(|arg| arg.split_once('=').map(|(name, value)| (name.to_owned(), value.to_owned())))
            .collect()
    }

    pub fn program(&self) -> &str {
        self.args.first().map_or("", |arg| arg.rsplit('/').next().unwrap_or(arg))
    }

    fn git_subcommand(&self) -> usize {
        let mut index = 1;
        while let Some(arg) = self.args.get(index) {
            if !arg.starts_with('-') {
                break;
            }
            index += if matches!(arg.as_str(), "-C" | "-c" | "--git-dir" | "--work-tree" | "--namespace" | "--config-env") { 2 } else { 1 };
        }
        index.min(self.args.len())
    }

    pub fn is_search(&self) -> bool {
        matches!(self.program(), "grep" | "egrep" | "fgrep" | "rg")
            || self.program() == "git" && self.args.get(self.git_subcommand()).is_some_and(|arg| arg == "grep")
    }

    pub fn is_silent_diff(&self) -> bool {
        match self.program() {
            "cmp" => self.has_flag('s', "--silent") || self.has_flag('s', "--quiet"),
            "git" => {
                let index = self.git_subcommand();
                if self.args.get(index).map(String::as_str) != Some("diff") {
                    return false;
                }
                let mut opts = self.args.get(index + 1..).unwrap_or_default().iter().take_while(|arg| arg.as_str() != "--");
                let has_quiet = opts.clone().any(|arg| arg == "--quiet");
                let has_patch = opts.any(|arg| matches!(arg.as_str(), "-p" | "-u" | "--patch"));
                has_quiet && !has_patch
            }
            _ => false,
        }
    }

    /// `sed -n 'a,bp'` 形式的行范围（管道后段作用于 `nl`/`cat` 时同样适用）；返回范围与显式文件路径。
    pub fn sed_read_range(&self) -> Option<((u64, Option<u64>), Vec<&str>)> {
        if self.program() != "sed" || self.has_flag('i', "--in-place") || !self.has_flag('n', "--quiet") {
            return None;
        }
        let args = &self.args;
        let options = args.iter().skip(1).take_while(|arg| arg.as_str() != "--");
        if options.clone().any(|arg| arg.starts_with("-f") || arg.starts_with("--file")) {
            return None;
        }
        let positional = self.positional(&["-e", "--expression"]);
        let explicit = options.filter(|arg| matches!(arg.as_str(), "-e" | "--expression")).count();
        let (script, files) = match explicit {
            0 => (positional.first().copied(), positional.get(1..).unwrap_or_default()),
            1 => (option(args, "-e").or_else(|| option(args, "--expression")), &positional[..]),
            _ => return None,
        };
        Some((sed_range(script?)?, files.to_vec()))
    }

    pub fn positional(&self, value_flags: &[&str]) -> Vec<&str> {
        let mut out = Vec::new();
        let mut args = self.args.iter().skip(1);
        let mut options = true;
        while let Some(arg) = args.next() {
            if options && arg == "--" {
                options = false;
            } else if options && value_flags.contains(&arg.as_str()) {
                args.next();
            } else if !(options && arg.starts_with('-') && arg.len() > 1) {
                out.push(arg.as_str());
            }
        }
        out
    }

    pub fn has_flag(&self, short: char, long: &str) -> bool {
        self.args.iter().skip(1).any(|arg| arg == long
            || (arg.starts_with('-') && !arg.starts_with("--") && arg[1..].contains(short)))
    }

    /// `-c`/`-e` 后的内联脚本。
    fn inline_code(&self) -> Option<&str> {
        let (program, args) = self.interpreter_args();
        script_lang(program)?;
        ["-c", "-e"].iter().find_map(|flag| option(args, flag))
    }

    /// 运行的脚本文件：解释器后的第一个参数，或直接按路径执行的程序。
    pub fn executed_file(&self) -> Option<&str> {
        let (program, args) = self.interpreter_args();
        if script_lang(program).is_some() {
            const VALUES: &[&str] = &["-W", "-X", "-r", "--require", "--import", "--loader", "--conditions", "--env-file"];
            let mut rest = args.iter().skip(1);
            while let Some(arg) = rest.next() {
                if matches!(arg.as_str(), "-c" | "-e" | "-m" | "--eval" | "-p" | "--print") {
                    return None;
                }
                if arg == "--" {
                    return rest.next().map(String::as_str).filter(|arg| *arg != "-");
                }
                if VALUES.contains(&arg.as_str()) {
                    rest.next();
                } else if !arg.starts_with('-') {
                    return Some(arg.as_str());
                }
            }
            return None;
        }
        args.first().map(String::as_str).filter(|program| program.contains('/'))
    }

    /// 交给 `ssh` 的正文由远程命令里的解释器执行，没写解释器就是远端 shell。
    fn script_language(&self) -> Option<&'static str> {
        let (program, args) = self.interpreter_args();
        if program != "ssh" {
            return script_lang(program);
        }
        Some(args.iter().skip(1).flat_map(|arg| arg.split_whitespace())
            .find_map(|word| script_lang(word.rsplit('/').next().unwrap_or(word)).filter(|lang| *lang != "Shell"))
            .unwrap_or("Shell"))
    }

    pub fn script(&self, body: Option<&str>) -> Option<Script> {
        body.or_else(|| self.inline_code()).filter(|body| !body.trim().is_empty())
            .map(|body| Script::new(self.script_language(), body))
    }

    fn interpreter_args(&self) -> (&str, &[String]) {
        let args = &self.args[self.runner_offset()..];
        (args.first().and_then(|arg| arg.rsplit('/').next()).unwrap_or_default(), args)
    }

    pub fn response_file(&self) -> Option<&str> {
        let flag = match self.program() { "curl" => "--output", "wget" => "--output-document", _ => return None };
        let short = if self.program() == "curl" { "-o" } else { "-O" };
        let mut args = self.args.iter().skip(1);
        while let Some(arg) = args.next() {
            if arg == "--" { break; }
            if matches!(arg.as_str(), "-H" | "--header" | "-d" | "--data" | "-X" | "--request" | "-u" | "--user") {
                args.next();
            } else if let Some(path) = if arg == flag || arg == short { args.next().map(String::as_str) }
                else { arg.split_once('=').filter(|(key, _)| *key == flag).map(|(_, value)| value) } {
                return (!matches!(path, "-" | "/dev/null" | "")).then_some(path);
            }
        }
        None
    }

    /// 标题用的原文：去掉前导环境变量、无关重定向、heredoc 标记、内联脚本和 `git -C`。
    pub fn head(&self, text: &str) -> String {
        let mut words = Vec::new();
        let mut raw = self.raw.iter().peekable();
        let ssh = self.program() == "ssh";
        let mut ssh_options = false;
        let mut first_word = true;
        let script_offset = self.runner_offset();
        let first_script_arg = self.inline_code().and_then(|_| self.args.iter().enumerate().skip(script_offset + 1)
            .find_map(|(index, arg)| matches!(arg.as_str(), "-c" | "-e").then_some(index)));
        let mut word_index = 0;
        while let Some(item) = raw.next() {
            if matches!(item, Raw::Word(_)) {
                word_index += 1;
            }
            match item {
                Raw::Word(word) if (word == "-C" && self.program() == "git") => {
                    raw.next();
                }
                // `ssh` 的连接选项只是噪音，标题只留主机和远程命令。
                Raw::Word(word) if ssh_options && word == "--" => ssh_options = false,
                Raw::Word(word) if ssh_options && word.starts_with('-') => {
                    if matches!(word.as_str(), "-o" | "-i" | "-p" | "-F" | "-J" | "-l" | "-L" | "-R" | "-D" | "-W" | "-b" | "-c" | "-E" | "-m" | "-S") {
                        raw.next();
                    }
                }
                Raw::Word(word) if Some(word_index - 1) == first_script_arg => {
                    words.push(word.clone());
                    break;
                }
                Raw::Word(word) => {
                    ssh_options = ssh && first_word;
                    first_word = false;
                    words.push(word.clone());
                }
                Raw::Redirect { text, noise: false } => words.push(text.clone()),
                Raw::Redirect { .. } => {}
            }
        }
        let head = words.join(" ");
        if head.is_empty() { text.trim().into() } else { head }
    }
}

fn runner_skip_at(args: &[String]) -> Option<usize> {
    let program = args.first().map_or("", |arg| arg.rsplit('/').next().unwrap_or(arg));
    let sub = args.get(1).map(String::as_str);
    let (from, values): (usize, &[&str]) = match program {
        "uv" | "poetry" | "pdm" | "hatch" if sub == Some("run") => (2, &["--with", "-p", "--python", "--project", "--directory", "--from", "--extra", "--group", "--env-file", "--index"]),
        "npm" | "pnpm" if matches!(sub, Some("exec" | "dlx")) => (2, &["--package", "-p"]),
        "rustup" if sub == Some("run") => (3, &[]),
        "npx" | "bunx" | "pnpx" | "uvx" => (1, &["--package", "-p", "--from", "--with", "--python"]),
        "xargs" => (1, &["-E", "-I", "-J", "-L", "-n", "-P", "-s", "-d", "-a", "--replace", "--max-procs", "--max-args", "--delimiter", "--arg-file"]),
        _ => return None,
    };
    let mut index = from;
    while let Some(arg) = args.get(index) {
        if arg == "--" { return (index + 1 < args.len()).then_some(index + 1); }
        if !arg.starts_with('-') {
            return Some(index);
        }
        index += if values.contains(&arg.as_str()) { 2 } else { 1 };
    }
    None
}

/// 整段全是变量赋值且最后一项是 `NAME=$(…)` 时，拆出前面的赋值、当前赋值的变量名和替换内的命令。
pub(super) fn unwrap_substitution<'a>(text: &'a str, vars: &[(String, String)], budget: &mut usize) -> Option<(Vec<(String, String)>, String, &'a str)> {
    let mut words = Vec::new();
    for token in tokens(text) {
        match token {
            Token::Word(word) => words.push(word),
            Token::Redirect { op, .. } if op.contains('>') && !op.starts_with('2') => return None,
            Token::Redirect { .. } => {}
        }
    }
    let (&last, earlier) = words.split_last()?;
    if !words.iter().all(|word| word.split_once('=').is_some_and(|(name, _)| is_assignment_name(name))) {
        return None;
    }
    let (name, value) = last.split_once('=')?;
    let inner = substitution(value)?;
    let assignments = earlier.iter().filter_map(|word| {
        let (name, value) = word.split_once('=')?;
        Some((name.to_owned(), unquote(value, vars, budget)))
    }).collect();
    Some((assignments, name.to_owned(), inner))
}

/// 不会静默退出非零跳过 `&&` 后续步骤的纯设置命令；隐藏 stderr 时都可能静默失败。
pub(super) fn is_safe_setup(segment: &Segment) -> bool {
    !segment.suppresses_stderr && matches!(
        segment.program(),
        "" | "cd" | "export" | "setopt" | "shopt" | "umask" | "unset" | "local" | "true" | ":" | "pwd" | "echo" | "printf"
    )
}

fn is_assignment_name(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !name.starts_with(|c: char| c.is_ascii_digit())
}

fn is_assignment(arg: &str) -> bool {
    arg.split_once('=').is_some_and(|(name, _)| is_assignment_name(name))
}

fn option<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    args.iter().position(|arg| arg == flag).and_then(|i| args.get(i + 1)).map(String::as_str)
}

fn script_lang(program: &str) -> Option<&'static str> {
    Some(match program {
        "python" | "python3" => "Python",
        "node" | "deno" | "bun" | "ego-browser" => "JavaScript",
        "bash" | "sh" | "zsh" | "ssh" => "Shell",
        "ruby" => "Ruby",
        "perl" => "Perl",
        "osascript" => "AppleScript",
        "swift" => "Swift",
        "sqlite3" | "psql" | "mysql" => "SQL",
        _ => return None,
    })
}

const GREP_VALUES: &[&str] = &["-e", "-f", "-m", "-A", "-B", "-C", "-g", "--glob", "-t", "--type", "-T", "--type-not",
    "--type-add", "-M", "--max-columns", "-j", "--threads", "--max-count", "--context", "--after-context",
    "--before-context", "--include", "--exclude", "--exclude-dir"];

pub(super) fn classify(segment: &Segment, has_body: bool) -> Action {
    classify_inner(segment, has_body, true)
}

fn classify_inner(segment: &Segment, has_body: bool, unwrap_runners: bool) -> Action {
    let program = segment.program();
    let args = &segment.args;
    let read = |path: &str| Spec::Read { path: path.into(), start: None, end: None, tail: None, whole: false };
    // `-V`、`-h` 只对 HTTP 客户端和解释器表示版本与帮助；`ls -h` 这类仍按原意。
    let version_or_help = args.len() == 2 && match args[1].as_str() {
        "--version" | "--help" => true,
        "-V" | "-h" => matches!(program, "curl" | "wget" | "http" | "xh") || script_lang(program).is_some(),
        _ => false,
    };
    if version_or_help {
        return Action::Run(Intent::Inspect);
    }
    match program {
        "" | "cd" | "export" | "set" | "setopt" | "shopt" | "source" | "." | "echo" | "printf" | "true" | ":" | "pwd" | "umask"
        | "unset" | "local" | "read" | "shift" | "exit" | "return" | "break" | "continue" | "trap"
            if segment.target.is_none() || program == "cd" => Action::Noise,
        "for" | "while" | "until" | "if" | "then" | "else" | "elif" | "fi" | "do" | "done" | "case" | "esac" | "{"
        | "}" | "(" | ")" | "function" | "[" | "[[" | "test" => Action::Control,
        "echo" | "printf" | "cat" | "tee" if segment.target.is_some() || (program == "tee" && has_body) => {
            Action::Run(Intent::Write)
        }
        "rg" if args.iter().any(|arg| arg == "--files") => {
            let globs = args.windows(2).filter(|w| w[0] == "-g" || w[0] == "--glob").map(|w| w[1].as_str())
                .filter(|glob| !glob.starts_with('!')).collect::<Vec<_>>();
            Action::Browse(vec![Spec::Find {
                pattern: (!globs.is_empty()).then(|| globs.join(" ")),
                paths: segment.positional(GREP_VALUES).iter().map(|p| p.to_string()).collect(),
            }])
        }
        "rg" | "grep" | "egrep" | "fgrep" | "ag" | "ack" => match grep_spec(segment, program == "rg", matches!(program, "grep" | "egrep" | "fgrep")) {
            Some(spec) => Action::Browse(vec![spec]),
            None => Action::Run(Intent::Other),
        },
        "find" if find_actions(args).any(|(index, arg)| arg == "-delete"
            || matches!(arg, "-exec" | "-execdir") && args.get(index + 1).is_some_and(|next| matches!(next.as_str(), "rm" | "rmdir"))) => {
            Action::Run(Intent::Remove)
        }
        "find" => {
            let paths = args[1..].iter().take_while(|arg| !arg.starts_with(['-', '(', '!', '\\'])).cloned().collect();
            let names: Vec<_> = args.windows(2).enumerate()
                .filter(|(i, w)| matches!(w[0].as_str(), "-name" | "-iname" | "-path")
                    && !(*i > 0 && matches!(args[i - 1].as_str(), "-not" | "!")))
                .map(|(_, w)| w[1].as_str()).collect();
            Action::Browse(vec![Spec::Find { pattern: (!names.is_empty()).then(|| names.join(" ")), paths }])
        }
        "fd" | "fdfind" => {
            let positional = segment.positional(&["-e", "-t", "-d", "--max-depth", "-E", "--exclude", "--extension", "--type"]);
            Action::Browse(vec![Spec::Find {
                pattern: positional.first().map(|p| p.to_string()),
                paths: positional.iter().skip(1).map(|p| p.to_string()).collect(),
            }])
        }
        "ls" | "tree" | "eza" | "exa" => Action::Browse(vec![Spec::List {
            paths: segment.positional(&["-L", "-I", "--ignore"]).iter().map(|p| p.to_string()).collect(),
        }]),
        "sed" if segment.has_flag('i', "--in-place") => Action::Run(Intent::Modify),
        "sed" => match segment.sed_read_range().filter(|(_, files)| !files.is_empty()) {
            Some(((start, end), files)) => Action::Browse(files.into_iter().map(|path| Spec::Read {
                path: path.to_string(), start: Some(start), end, tail: None, whole: false,
            }).collect()),
            None => Action::Run(Intent::Transform),
        },
        "cat" | "bat" | "nl" | "less" | "more" | "head" | "tail" => {
            let files = segment.positional(if matches!(program, "head" | "tail") { &["-n", "-c"] } else { &[] });
            if files.is_empty() {
                return Action::Run(Intent::Other);
            }
            let (from_line, count) = line_limit(args);
            let single = files.len() == 1;
            Action::Browse(files.iter().map(|path| match program {
                "head" => Spec::Read { path: path.to_string(), start: Some(1), end: Some(count.unwrap_or(10)), tail: None, whole: false },
                "tail" => Spec::Read { path: path.to_string(), start: from_line, end: None, tail: from_line.is_none().then_some(count.unwrap_or(10)), whole: false },
                "cat" if single => Spec::Read { path: path.to_string(), start: Some(1), end: None, tail: None, whole: true },
                _ => read(path),
            }).collect())
        }
        "wc" => Action::Run(Intent::Count),
        "git" => {
            let index = segment.git_subcommand();
            let subcommand = args.get(index).map(String::as_str).unwrap_or_default();
            if subcommand == "grep" {
                let grep = segment.shifted(index);
                if let Some(spec) = grep_spec(&grep, false, false) {
                    return Action::Browse(vec![spec]);
                }
            }
            let arguments = args.get(index + 1..).unwrap_or_default();
            let opts = || arguments.iter().take_while(|arg| arg.as_str() != "--");
            Action::Run(match subcommand {
                "show" if opts().any(|arg| !arg.starts_with('-') && arg.split_once(':').is_some_and(|(_, path)| !path.is_empty())) => Intent::GitRead,
                // `--quiet` 是静默比较（退出码 1 表示有差异），即使带 `--stat` 也保留 diff 意图。
                "diff" if opts().any(|arg| arg == "--quiet") => Intent::GitDiff,
                // 只看统计或文件名的 diff 不输出补丁，除非另外要了补丁。
                "diff" if !opts().any(|arg| matches!(arg.as_str(), "-p" | "-u" | "--patch"))
                    && opts().any(|arg| matches!(arg.as_str(), "--check" | "--stat" | "--shortstat" | "--numstat" | "--name-only"
                    | "--name-status" | "--summary" | "--dirstat") || arg.starts_with("--stat=") || arg.starts_with("--dirstat=")) => Intent::Git,
                "diff" | "show" => Intent::GitDiff,
                "log" | "blame" | "reflog" | "shortlog" => Intent::GitLog,
                "commit" => Intent::GitCommit,
                "add" if opts().any(|arg| matches!(arg.as_str(), "-n" | "--dry-run")) => Intent::Inspect,
                "apply" if opts().any(|arg| matches!(arg.as_str(), "--check" | "--stat" | "--numstat" | "--summary")) => Intent::Inspect,
                "add" => Intent::GitStage,
                "stash" if matches!(arguments.first().map(String::as_str), Some("list" | "show")) => Intent::GitLog,
                "reset" | "restore" | "merge" | "rebase" | "cherry-pick" | "revert" | "stash" | "clone" | "pull" | "checkout" | "switch"
                | "init" | "rm" | "mv" | "apply" | "am" => Intent::GitChange,
                "push" => Intent::GitPush,
                _ => Intent::Git,
            })
        }
        "diff" | "cmp" => Action::Run(Intent::Diff),
        "gh" => Action::Run(Intent::Github),
        "ssh" if args.iter().any(|arg| arg == "-G") => Action::Run(Intent::Inspect),
        "ssh" | "scp" | "rsync" if program != "rsync" || args.iter().skip(1).any(|arg| arg.contains(':') && !arg.starts_with('-')) => Action::Run(Intent::Remote),
        "curl" | "wget" | "http" | "xh" => Action::Run(Intent::Http),
        "ego-browser" | "playwright-cli" | "agent-browser" if matches!(args.get(1).map(String::as_str), Some("skill" | "help")) => Action::Run(Intent::Inspect),
        "ego-browser" | "playwright-cli" | "agent-browser" => Action::Run(Intent::Browser),
        "sqlite3" | "psql" | "mysql" | "redis-cli" => Action::Run(Intent::Database),
        "mkdir" => Action::Run(Intent::Mkdir),
        "rm" | "rmdir" | "trash" => Action::Run(Intent::Remove),
        "cp" | "ditto" | "rsync" | "install" => Action::Run(Intent::Copy),
        "ln" => Action::Run(Intent::Link),
        "mv" => Action::Run(Intent::Move),
        "chmod" | "chown" | "chgrp" | "chflags" => Action::Run(Intent::Permissions),
        "ps" | "pgrep" | "lsof" | "top" => Action::Run(Intent::Process),
        "kill" | "pkill" | "killall" => Action::Run(Intent::Kill),
        "sleep" | "wait" => Action::Run(Intent::Wait),
        "open" => Action::Run(Intent::Open),
        "codesign" if args.iter().skip(1).take_while(|arg| arg.as_str() != "--")
            .any(|arg| matches!(arg.as_str(), "-s" | "--sign" | "--remove-signature") || arg.starts_with("--sign=")) => Action::Run(Intent::Sign),
        "codesign" => Action::Run(Intent::Inspect),
        "tar" => Action::Run(Intent::Archive),
        "magick" | "convert" | "potrace" | "sips" => Action::Run(Intent::Transform),
        "docker" | "podman" if matches!(args.get(1).map(String::as_str), Some("build" | "run" | "exec")) => {
            Action::Run(if args[1] == "build" { Intent::Build } else { Intent::Run })
        }
        "which" | "whereis" | "type" | "command" | "stat" | "file" | "du" | "df" | "strings" | "objdump" | "otool" | "nm"
        | "shasum" | "sha256sum" | "sha1sum" | "md5" | "md5sum" | "cksum" | "uname" | "sw_vers" | "hostname" | "whoami" | "id"
        | "date" | "printenv" | "mdls" | "xattr" | "dig" | "nslookup" | "host" => Action::Run(Intent::Inspect),
        "jq" | "yq" | "awk" | "gawk" | "cut" | "tr" | "sort" | "uniq" | "column" | "iconv" | "base64" | "xxd" | "od" => {
            Action::Run(Intent::Transform)
        }
        "codex" | "claude" | "opencode" | "agy" | "pi" | "gemini" | "qoder" | "aider" | "cursor-agent" | "amp" => {
            let info = matches!(args.get(1).map(String::as_str), Some("help" | "version" | "changelog")) || args.iter().skip(1).take_while(|arg| *arg != "--")
                .any(|arg| matches!(arg.as_str(), "--help" | "--version" | "-h" | "-V" | "--list-models"));
            // 只有交给 Agent 一个任务才算调用 Agent；登录、配置、插件、模型和服务管理只是管理命令行工具本身。
            let manage = matches!(args.get(1).map(String::as_str), Some("auth" | "login" | "logout" | "config" | "mcp" | "models" | "list" | "plugin"
                | "plugins" | "update" | "upgrade" | "install" | "uninstall" | "remove" | "api" | "serve" | "doctor" | "stats" | "export" | "import"
                | "sessions" | "completion" | "features" | "debug" | "logs"));
            Action::Run(if info { Intent::Inspect } else if manage { Intent::AgentManage } else { Intent::Agent })
        }
        "sh" | "bash" | "zsh" if args.get(1).is_some_and(|arg| arg == "-n") => Action::Run(Intent::Build),
        "node" if node_options(args).any(|arg| matches!(arg, "--check" | "-c")) => Action::Run(Intent::Build),
        "node" if node_options(args).any(|arg| arg == "--test") => Action::Run(Intent::Test),
        "perl" | "ruby" if segment.has_flag('i', "--in-place") => Action::Run(Intent::Modify),
        _ if script_lang(program).is_some() && (has_body || segment.inline_code().is_some() || args.iter().any(|arg| arg == "-")) => {
            Action::Run(Intent::Script)
        }
        "python" | "python3" | "node" | "deno" | "ruby" | "perl" | "bash" | "sh" | "zsh" => Action::Run(Intent::Run),
        _ if program.ends_with(".sh") && args.first().is_some_and(|path| path.contains('/')) => Action::Run(Intent::Run),
        _ => match unwrap_runners.then(|| segment.runner_offset()).filter(|&offset| offset > 0) {
            Some(offset) => classify_inner(&segment.shifted(offset), has_body, false),
            None => Action::Run(build_intent(program, &segment.positional(TASK_VALUES)).unwrap_or(Intent::Other)),
        },
    }
}

fn grep_spec(segment: &Segment, is_rg: bool, need_paths_or_recursive: bool) -> Option<Spec> {
    let args = &segment.args;
    let positional = segment.positional(GREP_VALUES);
    let (pattern, paths) = match option(args, "-e") {
        Some(pattern) => (pattern, &positional[..]),
        None => {
            let (pattern, paths) = positional.split_first()?;
            (*pattern, paths)
        }
    };
    if need_paths_or_recursive && paths.is_empty()
        && !segment.has_flag('r', "--recursive") && !segment.has_flag('R', "--dereference-recursive")
    {
        return None;
    }
    let mut mode = SearchMode::default();
    for arg in args.iter().skip(1).take_while(|arg| *arg != "--") {
        match arg.as_str() {
            "--heading" if is_rg => mode.heading = true,
            "--no-heading" if is_rg => mode.heading = false,
            "-H" | "--with-filename" => mode.with_filename = Some(true),
            "-I" | "--no-filename" if is_rg => mode.with_filename = Some(false),
            "-h" | "--no-filename" if !is_rg => mode.with_filename = Some(false),
            "-n" | "--line-number" => mode.line_numbers = LineMode::Numbered,
            "-N" | "--no-line-number" if is_rg => mode.line_numbers = LineMode::Unnumbered,
            _ if arg.starts_with('-') && !arg.starts_with("--") => {
                for ch in arg[1..].chars() {
                    match ch {
                        'H' => mode.with_filename = Some(true),
                        'I' if is_rg => mode.with_filename = Some(false),
                        'h' if !is_rg => mode.with_filename = Some(false),
                        'n' => mode.line_numbers = LineMode::Numbered,
                        'N' if is_rg => mode.line_numbers = LineMode::Unnumbered,
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    if is_rg && (mode.with_filename == Some(false) || (paths.len() == 1 && is_file(paths[0]) && mode.with_filename != Some(true))) {
        mode.heading = false;
    }
    Some(Spec::Search {
        pattern: pattern.into(),
        paths: paths.iter().map(|p| (*p).to_string()).collect(),
        files_only: segment.has_flag('l', "--files-with-matches") || segment.has_flag('L', "--files-without-match"),
        counted: segment.has_flag('c', "--count") || (is_rg && args.iter().any(|arg| arg == "--count-matches")),
        mode,
    })
}

fn line_limit(args: &[String]) -> (Option<u64>, Option<u64>) {
    let lines = option(args, "-n");
    let from_line = lines.and_then(|n| n.strip_prefix('+')).and_then(|n| n.parse().ok());
    let count = lines.and_then(|n| n.trim_start_matches(['-', '+']).parse().ok())
        .or_else(|| args.iter().find_map(|arg| arg.strip_prefix('-')?.parse().ok()));
    (from_line, count)
}

/// `find` 的动作谓词，跳过 `-name` 这类谓词的取值。
fn find_actions(args: &[String]) -> impl Iterator<Item = (usize, &str)> {
    const VALUES: &[&str] = &["-name", "-iname", "-path", "-ipath", "-wholename", "-regex", "-iregex", "-lname", "-newer", "-type",
        "-maxdepth", "-mindepth", "-size", "-mtime", "-mmin", "-atime", "-user", "-group", "-perm"];
    let mut actions = Vec::new();
    let mut index = 1;
    while let Some(arg) = args.get(index) {
        actions.push((index, arg.as_str()));
        index += if VALUES.contains(&arg.as_str()) { 2 } else { 1 };
    }
    actions.into_iter()
}

/// `node` 在入口脚本之前的选项；脚本之后的都是脚本自己的参数。
fn node_options(args: &[String]) -> impl Iterator<Item = &str> {
    const VALUES: &[&str] = &["-r", "--require", "--import", "-e", "--eval", "-p", "--print", "-C", "--conditions",
        "--env-file", "--loader", "--test-name-pattern", "--test-reporter"];
    let mut options = Vec::new();
    let mut index = 1;
    while let Some(arg) = args.get(index) {
        if arg == "--" || !arg.starts_with('-') {
            break;
        }
        options.push(arg.as_str());
        index += if VALUES.contains(&arg.as_str()) { 2 } else { 1 };
    }
    options.into_iter()
}

/// 包管理器和构建工具在子命令前可带的取值选项。
const TASK_VALUES: &[&str] = &["--prefix", "-C", "--dir", "--cwd", "--workspace", "-w", "--filter", "-F", "--manifest-path", "--target-dir", "--config", "-Z"];

/// 构建、测试和依赖安装按子命令区分；`npm run` 看脚本名。
fn build_intent(program: &str, positional: &[&str]) -> Option<Intent> {
    let runner = matches!(program, "npm" | "pnpm" | "yarn" | "bun");
    let task = match positional {
        [run, task, ..] if runner && *run == "run" => task,
        [task, ..] => task,
        [] => "",
    }.to_ascii_lowercase();
    match program {
        "pytest" | "vitest" | "jest" => Some(Intent::Test),
        "playwright" if task == "install" => Some(Intent::Install),
        "playwright" => Some(Intent::Test),
        "tsc" | "make" | "xcodebuild" | "swiftc" | "rustc" | "gradle" | "mvn" | "gofmt" | "rustfmt" | "prettier" | "eslint" | "latexmk" | "pdflatex" | "xelatex" => Some(Intent::Build),
        "npm" | "pnpm" | "yarn" | "bun" | "cargo" | "go" | "swift" | "uv" | "vite" | "next" | "astro" | "tauri" => Some(
            if task.contains("test") || matches!(task.as_str(), "vitest" | "jest" | "playwright") {
                Intent::Test
            } else if matches!(task.as_str(), "install" | "ci" | "i" | "add" | "fetch" | "get" | "sync" | "mod") {
                Intent::Install
            } else if matches!(task.as_str(), "run" | "exec" | "dev" | "start" | "preview" | "serve") {
                Intent::Run
            } else if ["build", "check", "lint", "tsc", "vet", "fmt", "format", "clippy", "prettier", "eslint", "bundle", "pack", "compile", "tauri"]
                .iter().any(|word| task.contains(word)) {
                Intent::Build
            } else if program == "bun" {
                Intent::Run
            } else {
                Intent::Other
            },
        ),
        _ => None,
    }
}

/// `12,40p`、`12p`、`12,$p`。
fn sed_range(script: &str) -> Option<(u64, Option<u64>)> {
    let range = script.strip_suffix('p')?;
    let (start, end) = range.split_once(',').unwrap_or((range, range));
    let start = start.parse::<u64>().ok().filter(|&start| start > 0)?;
    Some((start, if end == "$" { None } else { Some(end.parse::<u64>().ok()?.max(start)) }))
}

/// 只对输入做筛选、截取或变换、不产生其他副作用的管道段。
pub(super) fn is_pure_filter(segment: &Segment) -> bool {
    segment.target.is_none() && match segment.program() {
        "head" | "tail" | "grep" | "egrep" | "fgrep" | "rg" | "wc" | "sort" | "uniq" | "cut" | "awk" | "jq" | "tr" | "column"
        | "cat" | "nl" | "less" | "more" | "fold" | "base64" | "xxd" | "od" | "strings" | "tac" | "rev" | "paste" | "fmt" => true,
        "sed" => !segment.has_flag('i', "--in-place"),
        _ => false,
    }
}

pub(super) fn filter(segment: &Segment) -> Option<Filter> {
    let count = || line_limit(&segment.args).1.unwrap_or(10);
    match segment.program() {
        "head" => Some(Filter::Head(count())),
        "tail" => Some(Filter::Tail(count())),
        "grep" | "rg" | "egrep" => {
            let pattern = option(&segment.args, "-e").or_else(|| segment.positional(GREP_VALUES).first().copied())?.to_owned();
            Some(if segment.has_flag('v', "--invert-match") { Filter::Exclude(pattern) } else { Filter::Include(pattern) })
        }
        "wc" => Some(Filter::Count),
        _ => None,
    }
}
