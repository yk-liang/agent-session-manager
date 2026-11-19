//! 仅向固定公开目录发起匿名 HTTPS GET 请求，不携带任何本机会话、模型列表、凭据或配置。
use std::{
    io::Read,
    process::{Command, Stdio},
};

pub const MODELS_DEV_URL: &str = "https://models.dev/api.json";
pub const LITELLM_URL: &str =
    "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json";
const MAX_CATALOG_BYTES: usize = 64 * 1024 * 1024;

pub(super) fn build_curl_args(url: &str) -> Result<Vec<&'static str>, String> {
    let target = match url {
        MODELS_DEV_URL => MODELS_DEV_URL,
        LITELLM_URL => LITELLM_URL,
        _ => return Err("仅允许访问固定的公开模型定价目录".into()),
    };
    // `--disable` 必须是首个参数，确保忽略本机 ~/.curlrc 中的任何自定义头、代理或参数。
    Ok(vec![
        "--disable",
        "--proto",
        "=https",
        "--location",
        "--max-redirs",
        "0",
        "--fail",
        "--silent",
        "--show-error",
        "--compressed",
        "--connect-timeout",
        "15",
        "--max-time",
        "45",
        "-H",
        "Accept: application/json",
        "-A",
        "AgentSessionManager-Pricing/1",
        "--",
        target,
    ])
}

pub fn fetch_public_catalog(url: &str) -> Result<Vec<u8>, String> {
    let args = build_curl_args(url)?;
    let mut child = Command::new("/usr/bin/curl")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("无法启动网络请求：{error}"))?;

    let mut stdout = child.stdout.take().ok_or("无法读取参考价响应流")?;
    let mut stderr = child.stderr.take();
    let stderr_handle = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut reader) = stderr.take() {
            let _ = reader.by_ref().take(4096).read_to_end(&mut buf);
        }
        String::from_utf8_lossy(&buf).trim().to_owned()
    });

    let mut raw = Vec::new();
    let mut chunk = [0u8; 65536];
    let mut exceeded = false;
    loop {
        match stdout.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                if raw.len().saturating_add(n) > MAX_CATALOG_BYTES {
                    exceeded = true;
                    let _ = child.kill();
                    break;
                }
                raw.extend_from_slice(&chunk[..n]);
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stderr_handle.join();
                return Err(format!("读取参考价失败：{error}"));
            }
        }
    }

    let status = child
        .wait()
        .map_err(|error| format!("等待网络请求结束失败：{error}"))?;
    let stderr_text = stderr_handle.join().unwrap_or_default();
    if exceeded {
        return Err("参考价目录体积超过 64 MiB 上限".into());
    }
    if !status.success() {
        let detail = if stderr_text.is_empty() {
            format!("退出码 {}", status.code().unwrap_or(-1))
        } else {
            stderr_text
        };
        return Err(format!("下载参考价失败：{detail}"));
    }
    if raw.is_empty() {
        return Err("参考价目录响应为空".into());
    }
    Ok(raw)
}
