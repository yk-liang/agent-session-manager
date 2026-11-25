mod runtime;

use super::escape_cwd;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Copy)]
pub(super) enum ThreadAction {
    Archive,
    Unarchive,
    Delete,
}

impl ThreadAction {
    fn method(self) -> &'static str {
        match self {
            Self::Archive => "thread/archive",
            Self::Unarchive => "thread/unarchive",
            Self::Delete => "thread/delete",
        }
    }
}

/// Unavailable：当前运行时无法完成且未做任何修改，可换用其他运行时；Operation 如实报告给用户。
#[derive(Debug)]
pub(super) enum UpdateError {
    Unavailable(String),
    Operation(String),
}

pub(super) fn update(home: &Path, action: ThreadAction, project: &str, id: &str) -> Result<(), String> {
    let deadline = Instant::now() + REQUEST_TIMEOUT;
    Connection::open(home, &|_| Ok(()), Some(deadline))?.run(|server| {
        server.read_thread(project, id)?;
        server.mutate(action, id)
    })
}

/// 本机 Codex App Server 连接；当前运行时无法完成请求时依次换用其他已安装的运行时。
pub(super) struct Connection<'a> {
    home: &'a Path,
    check: &'a dyn Fn(&mut AppServer) -> Result<(), UpdateError>,
    deadline: Option<Instant>,
    candidates: std::vec::IntoIter<PathBuf>,
    unavailable: Vec<String>,
    server: AppServer,
}

impl<'a> Connection<'a> {
    /// check 在每个新启动的运行时上执行一次，确认它与目录扫描使用同一份数据；
    /// deadline 限制整个操作（含启动和换用运行时），已发出的永久删除除外。
    pub fn open(
        home: &'a Path,
        check: &'a dyn Fn(&mut AppServer) -> Result<(), UpdateError>,
        deadline: Option<Instant>,
    ) -> Result<Self, String> {
        let mut candidates = runtime::executables().into_iter();
        let mut unavailable = Vec::new();
        let server = start(home, check, deadline, &mut candidates, &mut unavailable)?;
        Ok(Self { home, check, deadline, candidates, unavailable, server })
    }

    pub fn run<T>(&mut self, mut work: impl FnMut(&mut AppServer) -> Result<T, UpdateError>) -> Result<T, String> {
        loop {
            match work(&mut self.server) {
                Ok(value) => return Ok(value),
                Err(UpdateError::Operation(error)) => return Err(error),
                Err(UpdateError::Unavailable(error)) => {
                    self.unavailable.push(format!("{}：{error}", self.server.bin.display()));
                    self.server = start(self.home, self.check, self.deadline, &mut self.candidates, &mut self.unavailable)?;
                }
            }
        }
    }
}

fn start(
    home: &Path,
    check: &dyn Fn(&mut AppServer) -> Result<(), UpdateError>,
    deadline: Option<Instant>,
    candidates: &mut std::vec::IntoIter<PathBuf>,
    unavailable: &mut Vec<String>,
) -> Result<AppServer, String> {
    for bin in candidates {
        match AppServer::spawn(&bin, home, deadline).and_then(|mut server| check(&mut server).map(|()| server)) {
            Ok(server) => return Ok(server),
            Err(UpdateError::Unavailable(error)) => unavailable.push(format!("{}：{error}", bin.display())),
            Err(UpdateError::Operation(error)) => return Err(error),
        }
    }
    Err(if unavailable.is_empty() {
        "未找到本机 Codex App Server。请检查 Codex 桌面应用、VS Code 扩展或 CLI 是否已安装。".into()
    } else {
        format!("本机 Codex App Server 无法执行此操作，请更新 Codex。\n{}", unavailable.join("\n"))
    })
}

/// 一个 App Server 进程上的 JSON-RPC 会话：请求依次发送，忽略服务器通知。
pub(super) struct AppServer {
    bin: PathBuf,
    process: Option<Child>,
    input: Box<dyn Write + Send>,
    output: mpsc::Receiver<std::io::Result<String>>,
    deadline: Option<Instant>,
    next_id: u64,
}

impl AppServer {
    fn spawn(bin: &Path, home: &Path, deadline: Option<Instant>) -> Result<Self, UpdateError> {
        let mut child = Command::new(bin)
            .args(["app-server", "--listen", "stdio://"])
            .env("CODEX_HOME", home)
            .current_dir(home)
            .process_group(0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| UpdateError::Unavailable(format!("无法启动：{e}")))?;
        let input = BufWriter::new(child.stdin.take().expect("piped stdin"));
        let output = BufReader::new(child.stdout.take().expect("piped stdout"));
        let mut server = Self::over(bin, input, output);
        server.process = Some(child);
        server.deadline = deadline;
        server.request("initialize", json!({
            "clientInfo": {"name": "agent-session-manager", "version": env!("CARGO_PKG_VERSION")},
            "capabilities": {"experimentalApi": true}
        })).map_err(ResponseError::preflight)?;
        server.send(&json!({"method": "initialized"})).map_err(ResponseError::preflight)?;
        Ok(server)
    }

    fn over(bin: &Path, input: impl Write + Send + 'static, output: impl BufRead + Send + 'static) -> Self {
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            for line in output.lines() {
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        Self { bin: bin.to_owned(), process: None, input: Box::new(input), output: receiver, deadline: None, next_id: 0 }
    }

    /// App Server 实际读写的 SQLite 目录。
    pub fn sqlite_home(&mut self, home: &Path) -> Result<PathBuf, UpdateError> {
        let requirements = self.request("configRequirements/read", json!({})).map_err(ResponseError::preflight)?;
        let config = self.request("config/read", json!({})).map_err(ResponseError::preflight)?;
        let configured = requirements.pointer("/requirements/sqliteHome").and_then(Value::as_str)
            .or_else(|| config.pointer("/config/sqlite_home").and_then(Value::as_str));
        Ok(match configured {
            Some(uri) if uri.starts_with("file:") => tauri::Url::parse(uri).ok()
                .and_then(|url| url.to_file_path().ok())
                .ok_or_else(|| UpdateError::Operation("无法识别 Codex 返回的数据库目录".into()))?,
            Some(path) => home.join(crate::agents::expand_tilde(path)),
            None => super::sqlite_home(home),
        })
    }

    /// 确认线程身份和所属项目与目录扫描一致，返回 App Server 记录的日志路径。
    pub fn read_thread(&mut self, project: &str, id: &str) -> Result<Option<String>, UpdateError> {
        let thread = self.request("thread/read", json!({"threadId": id, "includeTurns": false}))
            .map_err(|error| match error {
                ResponseError::Rejected(message) => UpdateError::Operation(message),
                other => other.preflight(),
            })?;
        let cwd = thread.pointer("/thread/cwd").and_then(Value::as_str);
        if thread.pointer("/thread/id").and_then(Value::as_str) != Some(id)
            || cwd.map(escape_cwd).as_deref().unwrap_or("-unknown") != project
        {
            return Err(UpdateError::Operation("Codex 会话身份或所属项目已变化，请刷新列表后重试".into()));
        }
        Ok(thread.pointer("/thread/path").and_then(Value::as_str).map(str::to_owned))
    }

    pub fn mutate(&mut self, action: ThreadAction, id: &str) -> Result<(), UpdateError> {
        // 已发出的永久删除必须等服务器提交完成，不能因超时结束进程。
        let timeout = (!matches!(action, ThreadAction::Delete)).then_some(REQUEST_TIMEOUT);
        self.request_within(action.method(), json!({"threadId": id}), timeout)
            .map(drop)
            .map_err(|error| match error {
                ResponseError::Unsupported => UpdateError::Unavailable(error.message()),
                ResponseError::Rejected(message) => UpdateError::Operation(message),
                ResponseError::Transport(message) => UpdateError::Operation(format!(
                    "无法确认 Codex 会话操作结果，请刷新列表。\n{message}"
                )),
                ResponseError::Timeout => UpdateError::Operation(error.message()),
            })
    }

    fn request(&mut self, method: &str, params: Value) -> Result<Value, ResponseError> {
        self.request_within(method, params, Some(REQUEST_TIMEOUT))
    }

    fn request_within(&mut self, method: &str, params: Value, timeout: Option<Duration>) -> Result<Value, ResponseError> {
        let deadline = timeout.map(|timeout| {
            let limit = Instant::now() + timeout;
            self.deadline.map_or(limit, |deadline| deadline.min(limit))
        });
        // 超过期限后不再发出新请求，避免修改已发出却无从确认结果。
        if deadline.is_some_and(|deadline| deadline <= Instant::now()) {
            return Err(ResponseError::Timeout);
        }
        self.next_id += 1;
        let id = self.next_id;
        self.send(&json!({"id": id, "method": method, "params": params}))?;
        loop {
            let line = match deadline {
                Some(deadline) => self.output.recv_timeout(deadline.saturating_duration_since(Instant::now()))
                    .map_err(|error| match error {
                        RecvTimeoutError::Timeout => ResponseError::Timeout,
                        RecvTimeoutError::Disconnected => ResponseError::exited(),
                    })?,
                None => self.output.recv().map_err(|_| ResponseError::exited())?,
            };
            let line = line.map_err(|e| {
                ResponseError::Transport(format!("无法读取 Codex App Server 响应：{e}"))
            })?;
            let message: Value = serde_json::from_str(&line).map_err(|e| {
                ResponseError::Transport(format!("Codex App Server 返回了无效响应：{e}"))
            })?;
            if message.get("id").and_then(Value::as_u64) != Some(id) || message.get("method").is_some() {
                continue;
            }
            if let Some(error) = message.get("error") {
                if error.get("code").and_then(Value::as_i64) == Some(-32601) {
                    return Err(ResponseError::Unsupported);
                }
                return Err(ResponseError::Rejected(
                    error.get("message").and_then(Value::as_str).unwrap_or("Codex App Server 请求失败").into(),
                ));
            }
            return message.get("result").cloned()
                .ok_or_else(|| ResponseError::Transport("Codex App Server 响应缺少结果".into()));
        }
    }

    fn send(&mut self, message: &Value) -> Result<(), ResponseError> {
        writeln!(self.input, "{message}")
            .and_then(|()| self.input.flush())
            .map_err(|e| ResponseError::Transport(format!("无法向 Codex App Server 发送请求：{e}")))
    }
}

impl Drop for AppServer {
    fn drop(&mut self) {
        if let Some(child) = &mut self.process {
            // npm launches the native server as a child that inherits both pipes.
            if unsafe { libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL) } != 0 {
                let _ = child.kill();
            }
            let _ = child.wait();
        }
    }
}

enum ResponseError {
    Transport(String),
    Rejected(String),
    Unsupported,
    Timeout,
}

impl ResponseError {
    fn exited() -> Self {
        Self::Transport("Codex App Server 已退出".into())
    }

    fn message(self) -> String {
        match self {
            Self::Transport(message) | Self::Rejected(message) => message,
            Self::Unsupported => "当前版本不支持此操作".into(),
            Self::Timeout => "Codex 会话操作超时，请刷新列表确认结果".into(),
        }
    }

    /// 修改前的握手和读取失败说明该运行时不可用，超时除外。
    fn preflight(self) -> UpdateError {
        match self {
            Self::Timeout => UpdateError::Operation(self.message()),
            other => UpdateError::Unavailable(other.message()),
        }
    }
}

#[cfg(test)]
mod tests;
