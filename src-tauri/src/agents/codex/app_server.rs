mod runtime;

use super::{codex_home, escape_cwd};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{mpsc, Mutex, TryLockError};
use std::time::Duration;

#[derive(Clone, Copy)]
pub(super) enum ThreadAction {
    Archive,
    Unarchive,
    Delete,
}

impl ThreadAction {
    fn method(&self) -> &'static str {
        match self {
            Self::Archive => "thread/archive",
            Self::Unarchive => "thread/unarchive",
            Self::Delete => "thread/delete",
        }
    }
}

pub(super) type Mutation<'a> = dyn FnMut() -> Result<(), String> + 'a;
pub(super) type Preserve<'a> =
    dyn Fn(Option<&str>, Option<&str>, &mut Mutation<'_>) -> Result<(), String> + Sync + 'a;

pub(super) fn file_url_path(uri: &str) -> Option<PathBuf> {
    tauri::Url::parse(uri).ok()?.to_file_path().ok()
}

pub(super) fn update(action: ThreadAction, project: &str, id: &str) -> Result<(), String> {
    update_using(
        &runtime::executables(),
        &codex_home(),
        action,
        project,
        id,
        None,
    )
}

pub(super) fn delete_verified(
    home: &Path,
    project: &str,
    id: &str,
    preserve: &Preserve<'_>,
) -> Result<(), String> {
    update_using(
        &runtime::executables(),
        home,
        ThreadAction::Delete,
        project,
        id,
        Some(preserve),
    )
}

fn update_using(
    executables: &[PathBuf],
    home: &Path,
    action: ThreadAction,
    project: &str,
    id: &str,
    preserve: Option<&Preserve<'_>>,
) -> Result<(), String> {
    let mut errors = Vec::new();
    for bin in executables {
        match update_at(bin, home, action, project, id, preserve) {
            Ok(()) => return Ok(()),
            Err(UpdateError::Unavailable(error)) => {
                errors.push(format!("{}：{error}", bin.display()))
            }
            Err(UpdateError::Operation(error)) => return Err(error),
        }
    }
    if errors.is_empty() {
        return Err(
            "未找到本机 Codex App Server。请检查 Codex 桌面应用、VS Code 扩展或 CLI 是否已安装。"
                .into(),
        );
    }
    Err(format!(
        "本机 Codex App Server 无法执行此操作，请更新 Codex。\n{}",
        errors.join("\n")
    ))
}

#[derive(Debug)]
enum UpdateError {
    Unavailable(String),
    Operation(String),
}

fn update_at(
    bin: &Path,
    home: &Path,
    action: ThreadAction,
    project: &str,
    id: &str,
    preserve: Option<&Preserve<'_>>,
) -> Result<(), UpdateError> {
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
    let deletion = Mutex::new(());
    std::thread::scope(|scope| {
        let (tx, rx) = mpsc::sync_channel(1);
        let deletion = &deletion;
        scope.spawn(move || {
            let _ = tx.send(update_thread(
                input, output, action, project, id, preserve, deletion,
            ));
        });
        let limit = if matches!(action, ThreadAction::Delete) {
            180
        } else {
            30
        };
        let mut result = rx.recv_timeout(Duration::from_secs(limit));
        // 已发出的永久删除必须等服务器完成索引提交。
        let preflight = if result.is_err() {
            match deletion.try_lock() {
                Ok(guard) => Some(guard),
                Err(TryLockError::WouldBlock) => {
                    result = rx.recv().map_err(|_| mpsc::RecvTimeoutError::Disconnected);
                    None
                }
                Err(TryLockError::Poisoned(error)) => Some(error.into_inner()),
            }
        } else {
            None
        };
        // npm launches the native server as a child that inherits both pipes.
        if unsafe { libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL) } != 0 {
            let _ = child.kill();
        }
        let _ = child.wait();
        drop(preflight);
        result.map_err(|e| {
            UpdateError::Operation(
                match e {
                    mpsc::RecvTimeoutError::Timeout => "Codex 会话操作超时，请刷新列表确认结果",
                    mpsc::RecvTimeoutError::Disconnected => "Codex App Server 连接意外关闭",
                }
                .into(),
            )
        })?
    })
}

fn update_thread(
    mut input: impl Write,
    mut output: impl BufRead,
    action: ThreadAction,
    project: &str,
    id: &str,
    preserve: Option<&Preserve<'_>>,
    deletion: &Mutex<()>,
) -> Result<(), UpdateError> {
    request(&mut input, &mut output, 1, "initialize", json!({
            "clientInfo": {"name": "agent-session-manager", "version": env!("CARGO_PKG_VERSION")},
            "capabilities": {"experimentalApi": true}
    })).map_err(|error| UpdateError::Unavailable(error.message()))?;
    send(&mut input, &json!({"method": "initialized"}))
        .map_err(|error| UpdateError::Unavailable(error.message()))?;
    let mut request_id = 2;
    let sqlite = if preserve.is_some() {
        let requirements = request(
            &mut input,
            &mut output,
            request_id,
            "configRequirements/read",
            json!({}),
        )
        .map_err(|error| UpdateError::Unavailable(error.message()))?;
        request_id += 1;
        let config = request(
            &mut input,
            &mut output,
            request_id,
            "config/read",
            json!({}),
        )
        .map_err(|error| UpdateError::Unavailable(error.message()))?;
        request_id += 1;
        requirements
            .pointer("/requirements/sqliteHome")
            .and_then(Value::as_str)
            .or_else(|| {
                config
                    .pointer("/config/sqlite_home")
                    .and_then(Value::as_str)
            })
            .map(str::to_owned)
    } else {
        None
    };
    let thread = request(
        &mut input,
        &mut output,
        request_id,
        "thread/read",
        json!({"threadId": id, "includeTurns": false}),
    )
    .map_err(|error| match error {
        ResponseError::Rejected(message) => UpdateError::Operation(message),
        other => UpdateError::Unavailable(other.message()),
    })?;
    request_id += 1;
    let cwd = thread.pointer("/thread/cwd").and_then(Value::as_str);
    if thread.pointer("/thread/id").and_then(Value::as_str) != Some(id)
        || cwd.map(escape_cwd).as_deref().unwrap_or("-unknown") != project
    {
        return Err(UpdateError::Operation(
            "Codex 会话身份或所属项目已变化，请刷新列表后重试".into(),
        ));
    }
    let mut mutate = || {
        let _in_flight = matches!(action, ThreadAction::Delete)
            .then(|| deletion.lock().unwrap_or_else(|error| error.into_inner()));
        request(
            &mut input,
            &mut output,
            request_id,
            action.method(),
            json!({"threadId": id}),
        )
        .map(|_| ())
        .map_err(|error| match error {
            ResponseError::Unsupported => UpdateError::Unavailable(error.message()),
            ResponseError::Rejected(message) => UpdateError::Operation(message),
            ResponseError::Transport(message) => UpdateError::Operation(format!(
                "无法确认 Codex 会话操作结果，请刷新列表。\n{message}"
            )),
        })
    };
    let Some(preserve) = preserve else {
        return mutate();
    };
    let mut mutation_error = None;
    let result = preserve(
        sqlite.as_deref(),
        thread.pointer("/thread/path").and_then(Value::as_str),
        &mut || match mutate() {
            Ok(()) => Ok(()),
            Err(error) => {
                let message = match &error {
                    UpdateError::Unavailable(message) | UpdateError::Operation(message) => {
                        message.clone()
                    }
                };
                mutation_error = Some(error);
                Err(message)
            }
        },
    );
    result.map_err(|error| mutation_error.unwrap_or(UpdateError::Operation(error)))
}

fn request(
    input: &mut impl Write,
    output: &mut impl BufRead,
    id: u64,
    method: &str,
    params: Value,
) -> Result<Value, ResponseError> {
    send(
        input,
        &json!({"id": id, "method": method, "params": params}),
    )?;
    receive(output, id)
}

fn send(input: &mut impl Write, message: &Value) -> Result<(), ResponseError> {
    writeln!(input, "{message}")
        .and_then(|()| input.flush())
        .map_err(|e| ResponseError::Transport(format!("无法向 Codex App Server 发送请求：{e}")))
}

enum ResponseError {
    Transport(String),
    Rejected(String),
    Unsupported,
}

impl ResponseError {
    fn message(self) -> String {
        match self {
            Self::Transport(message) | Self::Rejected(message) => message,
            Self::Unsupported => "当前版本不支持此操作".into(),
        }
    }
}

fn receive(output: &mut impl BufRead, id: u64) -> Result<Value, ResponseError> {
    for line in output.lines() {
        let line = line.map_err(|e| {
            ResponseError::Transport(format!("无法读取 Codex App Server 响应：{e}"))
        })?;
        let message: Value = serde_json::from_str(&line).map_err(|e| {
            ResponseError::Transport(format!("Codex App Server 返回了无效响应：{e}"))
        })?;
        if message.get("id").and_then(Value::as_u64) != Some(id) || message.get("method").is_some()
        {
            continue;
        }
        if let Some(error) = message.get("error") {
            if error.get("code").and_then(Value::as_i64) == Some(-32601) {
                return Err(ResponseError::Unsupported);
            }
            return Err(ResponseError::Rejected(
                error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("Codex App Server 请求失败")
                    .into(),
            ));
        }
        return message
            .get("result")
            .cloned()
            .ok_or_else(|| ResponseError::Transport("Codex App Server 响应缺少结果".into()));
    }
    Err(ResponseError::Transport("Codex App Server 已退出".into()))
}

#[cfg(test)]
mod tests;
