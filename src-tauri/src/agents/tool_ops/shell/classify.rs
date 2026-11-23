//! 按程序给命令段归类：读、搜、找、列拆成文件操作，其余给出意图。
use super::syntax::{tokens, unquote, words, Token};
use super::{Action, Filter, Intent, Spec};

/// 一段命令的参数（已去引号、去掉前导环境变量和包装命令）与重定向。
pub(super) struct Segment {
    pub args: Vec<String>,
    /// 去掉的前导变量赋值。
    assigned: Vec<String>,
    /// 原文的词，保留引号，用于还原标题。
    raw: Vec<Raw>,
    pub target: Option<String>,
    pub append: bool,
    pub heredoc: bool,
}

enum Raw {
    Word(String),
    /// 指向 /dev/null、文件描述符复制和 heredoc 标记的重定向不进标题。
    Redirect { text: String, noise: bool },
}

impl Segment {
    pub fn parse(text: &str, vars: &[(String, String)]) -> Segment {
        let mut segment = Segment { args: Vec::new(), assigned: Vec::new(), raw: Vec::new(), target: None, append: false, heredoc: false };
        for token in tokens(text) {
            match token {
                Token::Word(word) => {
                    segment.args.push(unquote(word, vars));
                    segment.raw.push(Raw::Word(word.into()));
                }
                Token::Redirect { op, target } => {
                    let target = target.map(|target| unquote(target, vars));
                    let heredoc = op.starts_with("<<") && !op.starts_with("<<<");
                    let null = target.as_deref() == Some("/dev/null");
                    if op.contains('>') && !null && target.is_some() {
                        segment.append = op.contains(">>");
                        segment.target = target.clone();
                    }
                    segment.heredoc |= heredoc;
                    let text = match &target {
                        Some(_) if !heredoc => format!("{op} {}", target.as_deref().unwrap_or_default()),
                        _ => op.to_string(),
                    };
                    segment.raw.push(Raw::Redirect { text, noise: heredoc || null || target.is_none() });
                }
            }
        }
        let skip = segment.args.iter().take_while(|arg| is_assignment(arg) && !arg.contains("=$(")).count();
        segment.assigned = segment.args.drain(..skip).collect();
        let mut raw_skip = skip;
        segment.raw.retain(|raw| match raw {
            Raw::Word(_) if raw_skip > 0 => {
                raw_skip -= 1;
                false
            }
            _ => true,
        });
        if let Some(inner) = segment.args.first().and_then(|arg| arg.split_once("=$(")).map(|(_, inner)| inner.trim_end_matches(')').to_owned()) {
            segment.args.splice(..1, words(&inner, vars));
        }
        loop {
            match segment.args.first().map(String::as_str) {
                Some("sudo" | "time" | "env" | "command" | "exec" | "nohup" | "caffeinate" | "do" | "then" | "else" | "{") => {
                    segment.args.remove(0);
                }
                Some("timeout") if segment.args.len() > 2 => {
                    segment.args.drain(..2);
                }
                Some(arg) if is_assignment(arg) => {
                    segment.args.remove(0);
                }
                _ => break,
            }
        }
        segment
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
    pub fn inline_code(&self) -> Option<&str> {
        script_lang(self.program())?;
        ["-c", "-e"].iter().find_map(|flag| option(&self.args, flag))
    }

    /// 标题用的原文：去掉前导环境变量、无关重定向、heredoc 标记、内联脚本和 `git -C`。
    pub fn head(&self, text: &str) -> String {
        let mut words = Vec::new();
        let mut raw = self.raw.iter().peekable();
        while let Some(item) = raw.next() {
            match item {
                Raw::Word(word) if (word == "-C" && self.program() == "git") => {
                    raw.next();
                }
                Raw::Word(word) if (word == "-c" || word == "-e") && self.inline_code().is_some() => {
                    words.push(word.clone());
                    break;
                }
                Raw::Word(word) => words.push(word.clone()),
                Raw::Redirect { text, noise: false } => words.push(text.clone()),
                Raw::Redirect { .. } => {}
            }
        }
        let head = words.join(" ");
        if head.is_empty() { text.trim().into() } else { head }
    }
}

fn is_assignment(arg: &str) -> bool {
    arg.split_once('=').is_some_and(|(name, _)| !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !name.starts_with(|c: char| c.is_ascii_digit()))
}

pub(super) fn option<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    args.iter().position(|arg| arg == flag).and_then(|i| args.get(i + 1)).map(String::as_str)
}

pub(super) fn script_lang(program: &str) -> Option<&'static str> {
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
    let program = segment.program();
    let args = &segment.args;
    let read = |path: &str| Spec::Read { path: path.into(), start: None, end: None, tail: None, whole: false };
    match program {
        "" | "cd" | "export" | "set" | "source" | "." | "echo" | "printf" | "true" | "false" | ":" | "pwd" | "umask"
        | "unset" | "local" | "read" | "shift" | "exit" | "return" | "break" | "continue" | "[" | "[[" | "test" | "trap"
            if segment.target.is_none() || program == "cd" => Action::Noise,
        "for" | "while" | "until" | "if" | "then" | "else" | "elif" | "fi" | "do" | "done" | "case" | "esac" | "{"
        | "}" | "(" | ")" | "function" => Action::Control,
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
        "rg" | "grep" | "egrep" | "fgrep" | "ag" | "ack" => {
            let positional = segment.positional(GREP_VALUES);
            let explicit = option(args, "-e");
            let (pattern, paths) = match explicit {
                Some(pattern) => (pattern, &positional[..]),
                None => match positional.split_first() {
                    Some((pattern, paths)) => (*pattern, paths),
                    None => return Action::Run(Intent::Other),
                },
            };
            let recursive = program != "grep" && program != "egrep" && program != "fgrep" || segment.has_flag('r', "--recursive")
                || segment.has_flag('R', "--dereference-recursive");
            if paths.is_empty() && !recursive {
                return Action::Run(Intent::Other);
            }
            Action::Browse(vec![Spec::Search {
                pattern: pattern.into(),
                paths: paths.iter().map(|p| p.to_string()).collect(),
                files_only: segment.has_flag('l', "--files-with-matches") || segment.has_flag('L', "--files-without-match"),
                counted: segment.has_flag('c', "--count") || args.iter().any(|arg| arg == "--count-matches"),
            }])
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
        "sed" => {
            let positional = segment.positional(&["-e", "-f"]);
            let Some((script, files)) = positional.split_first().filter(|(_, files)| !files.is_empty()) else {
                return Action::Run(Intent::Other);
            };
            let range = segment.has_flag('n', "--quiet").then(|| sed_range(script)).flatten();
            Action::Browse(files.iter().map(|path| match range {
                Some((start, end)) => Spec::Read { path: path.to_string(), start: Some(start), end, tail: None, whole: false },
                None => read(path),
            }).collect())
        }
        "cat" | "bat" | "nl" | "less" | "more" | "head" | "tail" => {
            let files = segment.positional(if matches!(program, "head" | "tail") { &["-n", "-c"] } else { &[] });
            if files.is_empty() {
                return Action::Run(Intent::Other);
            }
            let count = option(args, "-n").and_then(|n| n.trim_start_matches(['-', '+']).parse().ok())
                .or_else(|| args.iter().find_map(|arg| arg.strip_prefix('-')?.parse().ok()));
            let single = files.len() == 1;
            Action::Browse(files.iter().map(|path| match program {
                "head" => Spec::Read { path: path.to_string(), start: Some(1), end: Some(count.unwrap_or(10)), tail: None, whole: false },
                "tail" => Spec::Read { path: path.to_string(), start: None, end: None, tail: Some(count.unwrap_or(10)), whole: false },
                "cat" if single => Spec::Read { path: path.to_string(), start: Some(1), end: None, tail: None, whole: true },
                _ => read(path),
            }).collect())
        }
        "wc" => Action::Run(Intent::Count),
        "git" => {
            let subcommand = segment.positional(&["-C", "-c", "--git-dir", "--work-tree"]).first().copied().unwrap_or_default();
            if subcommand == "grep" {
                let positional = segment.positional(&["-C", "-c", "-e", "-f", "-A", "-B", "--max-count", "-m"]);
                let paths = positional.iter().skip(2).filter(|p| **p != "--").map(|p| p.to_string()).collect();
                return match positional.get(1) {
                    Some(pattern) => Action::Browse(vec![Spec::Search {
                        pattern: pattern.to_string(),
                        paths,
                        files_only: segment.has_flag('l', "--files-with-matches") || segment.has_flag('L', "--files-without-match"),
                        counted: segment.has_flag('c', "--count"),
                    }]),
                    None => Action::Run(Intent::Git),
                };
            }
            Action::Run(match subcommand {
                "diff" | "show" => Intent::GitDiff,
                "log" | "blame" | "reflog" | "shortlog" => Intent::GitLog,
                "add" | "commit" => Intent::GitCommit,
                _ => Intent::Git,
            })
        }
        "gh" => Action::Run(Intent::Github),
        "ssh" | "scp" | "rsync" if program != "rsync" || args.iter().skip(1).any(|arg| arg.contains(':') && !arg.starts_with('-')) => Action::Run(Intent::Remote),
        "curl" | "wget" | "http" | "xh" => Action::Run(Intent::Http),
        "ego-browser" | "playwright-cli" | "agent-browser" => Action::Run(Intent::Browser),
        "sqlite3" | "psql" | "mysql" | "redis-cli" => Action::Run(Intent::Database),
        "mkdir" => Action::Run(Intent::Mkdir),
        "rm" | "rmdir" | "trash" => Action::Run(Intent::Remove),
        "cp" | "ditto" | "rsync" => Action::Run(Intent::Copy),
        "mv" => Action::Run(Intent::Move),
        "ps" | "pgrep" | "lsof" | "top" => Action::Run(Intent::Process),
        "kill" | "pkill" | "killall" => Action::Run(Intent::Kill),
        "sleep" | "wait" => Action::Run(Intent::Wait),
        "open" => Action::Run(Intent::Open),
        _ if script_lang(program).is_some() && (has_body || segment.inline_code().is_some() || args.iter().any(|arg| arg == "-")) => {
            Action::Run(Intent::Script)
        }
        "python" | "python3" | "node" | "deno" | "bun" | "ruby" | "perl" | "bash" | "sh" | "zsh" => Action::Run(Intent::Run),
        _ => Action::Run(build_intent(program, &segment.positional(&[])).unwrap_or(Intent::Other)),
    }
}

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
        "tsc" | "make" | "xcodebuild" | "gradle" | "mvn" | "gofmt" | "rustfmt" | "prettier" | "eslint" => Some(Intent::Build),
        "npm" | "pnpm" | "yarn" | "bun" | "cargo" | "go" | "swift" | "npx" | "uv" | "uvx" => Some(
            if task.contains("test") || matches!(task.as_str(), "vitest" | "jest" | "playwright") {
                Intent::Test
            } else if matches!(task.as_str(), "install" | "ci" | "i" | "add" | "fetch" | "get" | "sync" | "mod") {
                Intent::Install
            } else if matches!(task.as_str(), "run" | "exec" | "dev" | "start" | "preview" | "serve") {
                Intent::Run
            } else if ["build", "check", "lint", "tsc", "vet", "fmt", "format", "clippy", "prettier", "eslint", "bundle", "pack", "compile", "tauri"]
                .iter().any(|word| task.contains(word)) {
                Intent::Build
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
    Some((start.parse().ok()?, if end == "$" { None } else { Some(end.parse().ok()?) }))
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
    let count = || option(&segment.args, "-n")
        .or_else(|| segment.args.iter().find_map(|arg| arg.strip_prefix('-').filter(|n| n.parse::<u64>().is_ok())))
        .and_then(|n| n.parse().ok()).unwrap_or(10);
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
