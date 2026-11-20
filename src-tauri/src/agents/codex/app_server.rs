mod runtime;

use super::{codex_home, escape_cwd};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
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

pub(super) fn update(action: ThreadAction, project: &str, id: &str) -> Result<(), String> {
    update_using(&runtime::executables(), &codex_home(), action, project, id)
}

fn update_using(
    executables: &[PathBuf],
    home: &Path,
    action: ThreadAction,
    project: &str,
    id: &str,
) -> Result<(), String> {
    let mut errors = Vec::new();
    for bin in executables {
        match update_at(bin, home, action, project, id) {
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
    std::thread::scope(|scope| {
        let (tx, rx) = mpsc::sync_channel(1);
        scope.spawn(move || {
            let _ = tx.send(update_thread(input, output, action, project, id));
        });
        let result = rx.recv_timeout(Duration::from_secs(30));
        // npm launches the native server as a child that inherits both pipes.
        if unsafe { libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL) } != 0 {
            let _ = child.kill();
        }
        let _ = child.wait();
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
) -> Result<(), UpdateError> {
    request(&mut input, &mut output, 1, "initialize", json!({
            "clientInfo": {"name": "agent-session-manager", "version": env!("CARGO_PKG_VERSION")},
            "capabilities": {"experimentalApi": true}
    })).map_err(|error| UpdateError::Unavailable(error.message()))?;
    send(&mut input, &json!({"method": "initialized"}))
        .map_err(|error| UpdateError::Unavailable(error.message()))?;
    let thread = request(
        &mut input,
        &mut output,
        2,
        "thread/read",
        json!({"threadId": id, "includeTurns": false}),
    )
    .map_err(|error| match error {
        ResponseError::Rejected(message) => UpdateError::Operation(message),
        other => UpdateError::Unavailable(other.message()),
    })?;
    let cwd = thread.pointer("/thread/cwd").and_then(Value::as_str);
    if thread.pointer("/thread/id").and_then(Value::as_str) != Some(id)
        || cwd.map(escape_cwd).as_deref().unwrap_or("-unknown") != project
    {
        return Err(UpdateError::Operation(
            "Codex 会话身份或所属项目已变化，请刷新列表后重试".into(),
        ));
    }
    request(
        &mut input,
        &mut output,
        3,
        action.method(),
        json!({"threadId": id}),
    )
    .map_err(|error| match error {
        ResponseError::Unsupported => UpdateError::Unavailable(error.message()),
        ResponseError::Rejected(message) => UpdateError::Operation(message),
        ResponseError::Transport(message) => UpdateError::Operation(format!(
            "无法确认 Codex 会话操作结果，请刷新列表。\n{message}"
        )),
    })?;
    Ok(())
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
