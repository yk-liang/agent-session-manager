//! Codex 本地数据源（CLI、IDE、桌面）：~/.codex/sessions/年/月/日/rollout-<时间戳>-<uuid>.jsonl
//!
//! 与 Claude 系完全不同：OpenAI Responses 格式（session_meta + response_item）。
//! 转换为统一对话事件，运行指令与状态通知独立标记为 context。
//! 项目按 session_meta 里的 cwd 分组。

mod context;
#[cfg(test)]
mod context_tests;
pub(crate) mod deferred;
mod history;
#[cfg(test)]
mod history_tests;
mod projects;

use serde::Deserialize;
use serde_json::{json, Value};
#[cfg(test)]
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use super::{AgentSource, ConfigFile, SessionDetail, SessionMeta};

fn codex_home() -> PathBuf {
    if let Ok(v) = std::env::var("CODEX_HOME") {
        if !v.is_empty() {
            return super::expand_tilde(&v);
        }
    }
    dirs::home_dir().unwrap_or_default().join(".codex")
}

fn sessions_root() -> PathBuf {
    codex_home().join("sessions")
}

fn escape_cwd(cwd: &str) -> String {
    super::escape_path_id(cwd)
}

/// 会话来源三态。官方协议中 source 可以是字符串（cli/vscode/exec/
/// appServer/mcp/internal...）或对象（subagent 的 thread_spawn 信息），
/// 且与 thread_source 相互独立。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SourceKind {
    /// 字段缺失（旧版 rollout），按可交互会话处理
    Missing,
    /// cli / vscode：用户可交互、可恢复
    Interactive,
    /// 其余字符串或对象形式：非交互，绝不按旧版交互会话兜底
    NonInteractive,
}

fn classify_source(payload: &Value) -> SourceKind {
    match payload.get("source") {
        None => SourceKind::Missing,
        Some(Value::String(s)) if s == "cli" || s == "vscode" => SourceKind::Interactive,
        Some(_) => SourceKind::NonInteractive,
    }
}

struct ParsedRollout {
    session_id: String,
    cwd: Option<String>,
    version: Option<String>,
    /// 有效历史中最近一次记录的模型；未记录时保持未知，不使用当前配置推测。
    model: Option<String>,
    /// 写入方标识：codex_cli / Codex Desktop 等（不能用于区分交互性）
    originator: Option<String>,
    /// 会话来源三态（官方协议，与 thread_source 独立）
    source: SourceKind,
    /// 线程来源（官方协议，与 source 独立）：user / subagent /
    /// onboarding_checklist / feature 等
    thread_source: Option<String>,
    created_ts: Option<String>,
    paginated: bool,
    history_base: Option<history::HistoryBase>,
    forked_from: Option<String>,
    first_ts: Option<String>,
    last_ts: Option<String>,
    user_count: u32,
    assistant_count: u32,
    real_user_count: u32,
    assistant_text_count: u32,
    first_user_text: Option<String>,
    events: Vec<Value>,
    size_bytes: u64,
}

fn text_of_content(items: &[Value]) -> String {
    let mut text = String::new();
    for item in items {
        let t = item.get("type").and_then(Value::as_str).unwrap_or("");
        if matches!(t, "input_text" | "output_text" | "text" | "summary_text") {
            if let Some(s) = item.get("text").and_then(Value::as_str) {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(s);
            }
        }
    }
    text
}

fn parse_arguments(raw: &str) -> Value {
    serde_json::from_str(raw).unwrap_or_else(|_| json!({ "raw": raw }))
}

fn extract_function_output(payload: &Value) -> (String, bool) {
    // output 字段本身是一段 JSON 字符串，常见形如 {"output": "..."} 或 {"content": "..."}
    let raw = payload.get("output").and_then(Value::as_str).unwrap_or("");
    if let Ok(v) = serde_json::from_str::<Value>(raw) {
        let is_error = v
            .pointer("/metadata/status")
            .and_then(Value::as_str)
            .map(|s| s == "error")
            .unwrap_or(false);
        if let Some(s) = v.get("output").and_then(Value::as_str) {
            return (s.to_string(), is_error);
        }
        if let Some(s) = v.get("content").and_then(Value::as_str) {
            return (s.to_string(), is_error);
        }
        return (v.to_string(), is_error);
    }
    (raw.to_string(), false)
}

/// 把一条 response_item 转成 0..n 个统一事件
fn convert_response_item(
    payload: &Value,
    ts: Option<&str>,
    cwd: Option<&str>,
    out: &mut Vec<Value>,
) {
    let kind = payload.get("type").and_then(Value::as_str).unwrap_or("");
    match kind {
        "message" => {
            let role = payload.get("role").and_then(Value::as_str).unwrap_or("");
            let items = payload.get("content").and_then(Value::as_array);
            let Some(items) = items else { return };
            context::convert_message(role, &text_of_content(items), ts, cwd, out);
        }
        "reasoning" => {
            let summary = payload.get("summary").and_then(Value::as_array);
            let thinking = summary
                .map(|items| text_of_content(items))
                .unwrap_or_default();
            if thinking.trim().is_empty() {
                return;
            }
            out.push(json!({
                "type": "assistant",
                "timestamp": ts,
                "cwd": cwd,
                "message": {
                    "role": "assistant",
                    "content": [{ "type": "thinking", "thinking": thinking }],
                },
            }));
        }
        "function_call" | "custom_tool_call" => {
            let name = payload.get("name").and_then(Value::as_str).unwrap_or("?");
            let call_id = payload.get("call_id").and_then(Value::as_str).unwrap_or("");
            let input = if kind == "function_call" {
                parse_arguments(
                    payload
                        .get("arguments")
                        .and_then(Value::as_str)
                        .unwrap_or("{}"),
                )
            } else {
                json!(payload.get("input").and_then(Value::as_str).unwrap_or(""))
            };
            out.push(json!({
                "type": "assistant",
                "timestamp": ts,
                "cwd": cwd,
                "message": {
                    "role": "assistant",
                    "content": [{ "type": "tool_use", "id": call_id, "name": name, "input": input }],
                },
            }));
        }
        "function_call_output" | "custom_tool_call_output" => {
            let call_id = payload.get("call_id").and_then(Value::as_str).unwrap_or("");
            let (content, is_error) = extract_function_output(payload);
            out.push(json!({
                "type": "user",
                "timestamp": ts,
                "cwd": cwd,
                "message": {
                    "role": "user",
                    "content": [{
                        "type": "tool_result",
                        "tool_use_id": call_id,
                        "content": content,
                        "is_error": is_error,
                    }],
                },
            }));
        }
        _ => {}
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ReadMode {
    Header,
    Summary,
    Events,
    View,
}

#[derive(Clone, Copy, Default)]
struct ReadLimit {
    bytes: Option<u64>,
    ordinal: Option<u64>,
}

fn parse_rollout(path: &Path, collect_events: bool) -> super::ParseOutcome<ParsedRollout> {
    read_rollout(
        path,
        if collect_events {
            ReadMode::Events
        } else {
            ReadMode::Summary
        },
        ReadLimit::default(),
        None,
    )
}

#[derive(Deserialize)]
struct Envelope<'a> {
    #[serde(rename = "type", borrow)]
    kind: Option<&'a serde_json::value::RawValue>,
    #[serde(borrow)]
    timestamp: Option<&'a serde_json::value::RawValue>,
    #[serde(borrow)]
    ordinal: Option<&'a serde_json::value::RawValue>,
    #[serde(borrow)]
    payload: Option<&'a serde_json::value::RawValue>,
}
#[derive(Default, Deserialize)]
struct ToolHeader {
    #[serde(rename = "type")]
    kind: Option<String>,
    id: Option<String>,
    call_id: Option<String>,
    name: Option<String>,
}

#[derive(Deserialize)]
struct ModelContext {
    model: Option<String>,
}

fn read_rollout(
    path: &Path,
    mode: ReadMode,
    limit: ReadLimit,
    mut dedup: Option<&mut history::EventDedup>,
) -> super::ParseOutcome<ParsedRollout> {
    let file = match super::open_jsonl(path) {
        Ok(f) => f,
        Err(e) => return super::ParseOutcome::Failed(format!("无法读取 rollout {:?}: {e}", path)),
    };
    let metadata = file.metadata().ok();
    let size_bytes = metadata.as_ref().map(|m| m.len()).unwrap_or(0);
    let identity = metadata
        .as_ref()
        .map(|m| (m.dev(), m.ino()))
        .unwrap_or_default();
    // history_base 的字节边界限定旧页有效前缀，不能把分页后补写的旧尾部混进当前历史。
    let mut reader =
        BufReader::with_capacity(64 * 1024, file.take(limit.bytes.unwrap_or(u64::MAX)));

    let mut parsed = ParsedRollout {
        session_id: String::new(),
        cwd: None,
        version: None,
        model: None,
        originator: None,
        source: SourceKind::Missing,
        thread_source: None,
        created_ts: None,
        paginated: false,
        history_base: None,
        forked_from: None,
        first_ts: None,
        last_ts: None,
        user_count: 0,
        assistant_count: 0,
        real_user_count: 0,
        assistant_text_count: 0,
        first_user_text: None,
        events: Vec::new(),
        size_bytes,
    };

    let mut bad_lines = 0;
    let mut line = Vec::new();
    let mut offset = 0u64;
    loop {
        line.clear();
        let len = match reader.read_until(b'\n', &mut line) {
            Ok(0) => break,
            Ok(len) => len,
            Err(_) => {
                bad_lines += 1;
                break;
            }
        };
        let line_offset = offset;
        offset += len as u64;
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        // RawValue 校验 JSON，但不分配 event_msg/turn_context 的巨大正文对象。
        let envelope: Envelope = match serde_json::from_slice(&line) {
            Ok(value) => value,
            Err(_) => {
                // 旧日志可能夹杂合法的 JSON 标量；与原解析器一样忽略，只对坏 JSON 告警。
                if serde_json::from_slice::<Value>(&line).is_err() {
                    bad_lines += 1;
                }
                continue;
            }
        };
        let ordinal = envelope
            .ordinal
            .and_then(|v| serde_json::from_str::<u64>(v.get()).ok());
        if limit
            .ordinal
            .is_some_and(|end| ordinal.is_some_and(|n| n >= end))
        {
            continue;
        }
        let kind = envelope
            .kind
            .and_then(|v| serde_json::from_str::<String>(v.get()).ok())
            .unwrap_or_default();
        let rtype = kind.as_str();
        let timestamp = envelope
            .timestamp
            .and_then(|v| serde_json::from_str::<String>(v.get()).ok());
        let ts = timestamp.as_deref();
        if let Some(ts) = ts {
            if parsed.first_ts.is_none() {
                parsed.first_ts = Some(ts.into());
            }
            parsed.last_ts = Some(ts.into());
        }
        let raw_payload = envelope.payload.map(|v| v.get()).unwrap_or("null");
        if matches!(rtype, "turn_context" | "token_usage_record") {
            // 只提取模型字段，跳过上下文中的大段指令；这些元数据不生成对话卡片。
            if let Ok(context) = serde_json::from_str::<ModelContext>(raw_payload) {
                if let Some(model) = context
                    .model
                    .map(|s| s.trim().to_owned())
                    .filter(|s| !s.is_empty())
                {
                    parsed.model = Some(model);
                }
            }
            continue;
        }
        if !matches!(rtype, "session_meta" | "response_item") {
            continue;
        }
        let shallow: ToolHeader = serde_json::from_str(raw_payload).unwrap_or_default();
        let tool = matches!(
            shallow.kind.as_deref(),
            Some(
                "function_call"
                    | "custom_tool_call"
                    | "function_call_output"
                    | "custom_tool_call_output"
            )
        );
        let mut v = Value::Null;
        // 分页重放的比较继续使用规范化 JSON；单页无需哈希去重。
        if dedup.is_some() || !tool || !matches!(mode, ReadMode::Summary | ReadMode::View) {
            v = serde_json::from_slice(&line).unwrap_or(Value::Null);
        }
        if rtype == "response_item" && dedup.as_deref_mut().is_some_and(|seen| !seen.include(&v)) {
            continue;
        }
        let deferred_payload;
        let payload = if tool && mode == ReadMode::Summary {
            if matches!(
                shallow.kind.as_deref(),
                Some("function_call" | "custom_tool_call")
            ) {
                parsed.assistant_count += 1;
            }
            continue;
        } else if tool && mode == ReadMode::View {
            let token = deferred::register(path, identity, line_offset, &line);
            deferred_payload = json!({"type": shallow.kind, "call_id": shallow.call_id, "name": shallow.name, "id": shallow.id, "_deferred":token});
            Some(&deferred_payload)
        } else {
            v.get("payload")
        };
        match rtype {
            "session_meta" => {
                if let Some(p) = payload {
                    parsed.session_id = p
                        .get("id")
                        .or_else(|| p.get("session_id"))
                        .and_then(Value::as_str)
                        .map(|s| s.to_string())
                        .unwrap_or_default();
                    parsed.cwd = p.get("cwd").and_then(Value::as_str).map(|s| s.to_string());
                    parsed.version = p
                        .get("cli_version")
                        .and_then(Value::as_str)
                        .map(|s| s.to_string());
                    parsed.originator = p
                        .get("originator")
                        .and_then(Value::as_str)
                        .map(|s| s.to_string());
                    // source 可以是字符串或对象（subagent 的 thread_spawn），
                    // 三态分类：对象形式明确为非交互，不得落入 Missing 兜底
                    parsed.source = classify_source(p);
                    parsed.thread_source = p
                        .get("thread_source")
                        .and_then(Value::as_str)
                        .map(|s| s.to_string());
                    parsed.created_ts = p
                        .get("timestamp")
                        .and_then(Value::as_str)
                        .or(ts)
                        .map(str::to_string);
                    parsed.paginated =
                        p.get("history_mode").and_then(Value::as_str) == Some("paginated");
                    parsed.history_base =
                        p.get("history_base")
                            .filter(|v| !v.is_null())
                            .and_then(|v| match serde_json::from_value(v.clone()) {
                                Ok(base) => Some(base),
                                Err(_) => {
                                    bad_lines += 1;
                                    None
                                }
                            });
                    parsed.forked_from = p
                        .get("forked_from_id")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                    if mode == ReadMode::Header && !parsed.session_id.is_empty() {
                        break;
                    }
                }
            }
            "response_item" => {
                if let Some(p) = payload {
                    let mut new_events = Vec::new();
                    if tool && mode == ReadMode::View {
                        let token = &p["_deferred"];
                        let call = matches!(
                            shallow.kind.as_deref(),
                            Some("function_call" | "custom_tool_call")
                        );
                        let block = if call {
                            json!({"type":"tool_use","id":shallow.call_id.unwrap_or_default(),"name":shallow.name.unwrap_or_else(|| "?".into()),"deferred":token})
                        } else {
                            json!({"type":"tool_result","tool_use_id":shallow.call_id.unwrap_or_default(),"deferred":token})
                        };
                        let role = if call { "assistant" } else { "user" };
                        new_events.push(json!({"type":role,"timestamp":ts,"message":{"role":role,"content":[block]}}));
                    } else {
                        convert_response_item(p, ts, parsed.cwd.as_deref(), &mut new_events);
                    }
                    if let Some(id) = p.get("id").and_then(Value::as_str) {
                        let position = envelope
                            .ordinal
                            .map(|v| v.get().to_string())
                            .unwrap_or_else(|| ts.unwrap_or_default().to_string());
                        let count = new_events.len();
                        for (part, event) in new_events.iter_mut().enumerate() {
                            event["uuid"] = json!(if count == 1 {
                                format!("{id}@{position}")
                            } else {
                                format!("{id}@{position}:{part}")
                            });
                        }
                    }
                    for ev in &new_events {
                        match ev.get("type").and_then(Value::as_str) {
                            Some("user") => {
                                // 统计含真实文本的用户消息（工具结果不计）
                                if let Some(s) =
                                    ev.pointer("/message/content").and_then(Value::as_str)
                                {
                                    parsed.user_count += 1;
                                    parsed.real_user_count += 1;
                                    if parsed.first_user_text.is_none() {
                                        parsed.first_user_text = Some(s.chars().take(80).collect());
                                    }
                                }
                            }
                            Some("assistant") => {
                                parsed.assistant_count += 1;
                                // 统一结构中正文消息的 content 为字符串，思考/工具为数组
                                if ev
                                    .pointer("/message/content")
                                    .and_then(Value::as_str)
                                    .is_some()
                                {
                                    parsed.assistant_text_count += 1;
                                }
                            }
                            _ => {}
                        }
                    }
                    if matches!(mode, ReadMode::Events | ReadMode::View) {
                        parsed.events.extend(new_events);
                    }
                }
            }
            _ => {} // 其余元数据不生成对话，避免与 response_item 重复。
        }
    }

    // 不用文件名兜底生成 ID：缺少有效 session_meta（含内容损坏）即解析失败。
    // 删除路径按解析出的 ID 匹配，绝不能信任文件名推导的身份
    if parsed.session_id.is_empty() {
        return super::ParseOutcome::Failed(format!(
            "rollout 缺少有效 session_meta，无法解析会话 ID: {:?}",
            path
        ));
    }
    if bad_lines > 0 {
        return super::ParseOutcome::Degraded(parsed, super::degraded_message(path, bad_lines));
    }
    super::ParseOutcome::Ok(parsed)
}

/// 枚举给定会话根目录下的全部 rollout（年/月/日分层）。
/// root 可注入：生产取 sessions_root()，测试直接用临时目录，
/// 不再依赖全局 CODEX_HOME 环境变量重定向
fn all_rollouts_in(root: &Path, warnings: &mut Vec<String>) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for year in super::read_dir_warned(root, warnings) {
        for month in super::read_dir_warned(&year.path(), warnings) {
            for day in super::read_dir_warned(&month.path(), warnings) {
                for f in super::read_dir_warned(&day.path(), warnings) {
                    let name = f.file_name().to_string_lossy().to_string();
                    if name.starts_with("rollout-") && name.ends_with(".jsonl") {
                        let path = f.path();
                        // 零字节文件没有会话身份或正文，也可能尚未写入，扫描时忽略。
                        // 仅跳过已确认的空普通文件；特殊文件和读取失败仍交给解析器告警。
                        if path.metadata().is_ok_and(|m| m.is_file() && m.len() == 0) {
                            continue;
                        }
                        files.push(path);
                    }
                }
            }
        }
    }
    files
}

/// 从 <home>/session_index.jsonl 读取会话标题（thread_name）；
/// 读取失败（非"不存在"）与损坏行进 warnings。home 可注入
fn load_thread_names_in(
    home: &Path,
    warnings: &mut Vec<String>,
) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    let path = home.join("session_index.jsonl");
    // 与转录读取同一套 FIFO 防护：非阻塞打开 + 普通文件复核，
    // 避免 session_index.jsonl 被换成 FIFO 时 read_to_string 永久阻塞
    let mut file = match super::open_jsonl(&path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return map,
        Err(e) => {
            warnings.push(format!("无法读取标题索引 {:?}: {e}", path));
            return map;
        }
    };
    let mut content = String::new();
    if let Err(e) = std::io::Read::read_to_string(&mut file, &mut content) {
        warnings.push(format!("无法读取标题索引 {:?}: {e}", path));
        return map;
    }
    let mut bad_lines = 0u32;
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            bad_lines += 1;
            continue;
        };
        if let (Some(id), Some(name)) = (
            v.get("id").and_then(Value::as_str),
            v.get("thread_name").and_then(Value::as_str),
        ) {
            if !name.trim().is_empty() {
                map.insert(id.to_string(), name.to_string());
            }
        }
    }
    if bad_lines > 0 {
        warnings.push(format!(
            "标题索引 {:?} 有 {bad_lines} 行无法解析，部分会话标题可能缺失",
            path
        ));
    }
    map
}

fn to_meta(p: &ParsedRollout, thread_name: Option<&String>) -> SessionMeta {
    SessionMeta {
        session_id: p.session_id.clone(),
        project_dir: p
            .cwd
            .as_deref()
            .map(escape_cwd)
            .unwrap_or_else(|| "-unknown".to_string()),
        cwd: p.cwd.clone(),
        title: thread_name.cloned().or_else(|| p.first_user_text.clone()),
        model: p.model.clone(),
        model_name: None,
        version: p.version.clone(),
        git_branch: None,
        entrypoint: None,
        first_ts: p.first_ts.clone(),
        last_ts: p.last_ts.clone(),
        user_count: p.user_count,
        assistant_count: p.assistant_count,
        real_user_count: p.real_user_count,
        assistant_text_count: p.assistant_text_count,
        size_bytes: p.size_bytes,
        has_transcript: true,
        stats_pending: false,
        source_files: Vec::new(),
    }
}

/// 官方协议中 source 与 thread_source 是两个独立字段：
/// - source：cli / vscode / exec / appServer / mcp / internal 等字符串，
///   或对象（subagent 的 thread_spawn 信息）。三态判定：只有字段缺失
///   才按旧版交互会话兜底；对象与未知字符串一律非交互
/// - thread_source：user / subagent / onboarding_checklist / feature 等，
///   只有 user（或缺失）是普通用户线程
fn is_relevant_session(p: &ParsedRollout) -> bool {
    let source_ok = p.source != SourceKind::NonInteractive;
    let thread_ok = matches!(p.thread_source.as_deref(), None | Some("user"));
    source_ok && thread_ok
}

/// catalog 实现，root 可注入（见 all_rollouts_in）
fn catalog_in(root: &Path, include_empty: bool) -> Result<super::Catalog, String> {
    catalog_with(root, include_empty, false, &|| false)
}
// 上下文通知不再算作用户消息；仅重建可再生的会话摘要，不触碰用量账本。
const SUMMARY_CACHE_KIND: &str = "codex-history-v3";
fn catalog_with(
    root: &Path,
    include_empty: bool,
    quick: bool,
    cancelled: &dyn Fn() -> bool,
) -> Result<super::Catalog, String> {
    if cancelled() {
        return Err("目录更新已取消".into());
    }
    let mut warnings = Vec::new();
    let thread_names = root
        .parent()
        .map(|home| load_thread_names_in(home, &mut warnings))
        .unwrap_or_default();
    let mut items = Vec::new();
    let groups = history::scan(root, &mut warnings);
    for pages in groups.values() {
        let paths: Vec<_> = pages.iter().map(|page| page.path.clone()).collect();
        if cancelled() {
            return Err("目录更新已取消".into());
        }
        let read = || history::read(pages, ReadMode::Summary).map(|(detail, ws)| (detail.meta, ws));
        let result = if quick {
            Ok((
                super::catalog_cache::peek_summary(SUMMARY_CACHE_KIND, &paths).unwrap_or_else(
                    || {
                        let mut meta = to_meta(&pages.last().unwrap().header, None);
                        meta.source_files = paths.clone();
                        meta.size_bytes = pages.iter().map(|p| p.header.size_bytes).sum();
                        meta.last_ts = paths
                            .iter()
                            .filter_map(|p| {
                                std::fs::metadata(p)
                                    .ok()?
                                    .modified()
                                    .ok()?
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .ok()
                            })
                            .max()
                            .and_then(|t| super::ms_to_iso(t.as_millis() as i64))
                            .or(meta.last_ts);
                        meta.stats_pending = true;
                        meta
                    },
                ),
                vec![],
            ))
        } else {
            super::catalog_cache::summary(SUMMARY_CACHE_KIND, &paths, read)
        };
        match result {
            Ok((mut meta, ws)) => {
                warnings.extend(ws);
                if !include_empty && !meta.stats_pending && meta.real_user_count == 0 {
                    continue;
                }
                if let Some(name) = thread_names.get(&meta.session_id) {
                    meta.title = Some(name.clone());
                }
                let display = meta.cwd.clone().unwrap_or_else(|| "(未知目录)".into());
                items.push((meta, display));
            }
            Err(error) => warnings.push(error),
        }
    }
    let mut catalog = super::assemble_catalog(items);
    let projectless = root
        .parent()
        .map(|home| projects::projectless_ids(home, &mut warnings))
        .unwrap_or_default();
    catalog.projectless_session_ids = catalog
        .sessions
        .iter()
        .filter(|session| projectless.contains(&session.session_id))
        .map(|session| session.session_id.clone())
        .collect();
    catalog.warnings = warnings;
    if cancelled() {
        return Err("目录更新已取消".into());
    }
    Ok(catalog)
}

fn read_session_in(root: &Path, session_id: &str) -> Result<SessionDetail, String> {
    read_session_mode(root, session_id, ReadMode::Events)
}
fn read_session_mode(
    root: &Path,
    session_id: &str,
    mode: ReadMode,
) -> Result<SessionDetail, String> {
    if session_id.len() > 64 || session_id.contains(['/', '\\', '\'']) {
        return Err("非法会话 ID".into());
    }
    let mut warnings = Vec::new();
    let groups = history::scan(root, &mut warnings);
    let pages = groups.get(session_id).ok_or("未找到该 Codex 会话")?;
    let (mut detail, ws) = history::read(pages, mode)?;
    detail.warnings = ws;
    let names = root
        .parent()
        .map(|home| load_thread_names_in(home, &mut warnings))
        .unwrap_or_default();
    if let Some(name) = names.get(session_id) {
        detail.meta.title = Some(name.clone());
    }
    Ok(detail)
}

/// 生产环境的移除方式：移入系统回收站
fn trash_remove(targets: Vec<PathBuf>) -> Result<(), String> {
    trash::delete_all(targets).map_err(|e| format!("移入回收站失败: {e}"))
}

/// 删除前的两轮扫描都必须证明身份集合完整；不可读文件不能按“无关”跳过。
fn deletion_pages(root: &Path, session_id: &str) -> Result<Vec<history::Page>, String> {
    let mut warnings = Vec::new();
    let paths = all_rollouts_in(root, &mut warnings);
    if !warnings.is_empty() {
        return Err(format!(
            "会话目录扫描不完整（{}），已中止删除",
            warnings.join("；")
        ));
    }
    let mut pages = Vec::new();
    for path in paths {
        // 先按项目过滤会掩盖跨项目的重复 ID，无法确认删除目标唯一。
        match parse_rollout(&path, false) {
            super::ParseOutcome::Ok(p) if p.session_id == session_id => {
                pages.push(history::Page { path, header: p });
            }
            super::ParseOutcome::Degraded(p, w) if p.session_id == session_id => {
                return Err(format!(
                    "目标文件解析不完整（{w}），无法完成身份复核，已中止删除"
                ));
            }
            super::ParseOutcome::Failed(w) => {
                return Err(format!("文件身份无法确认（{w}），已中止删除"));
            }
            _ => {}
        }
    }
    pages.sort_by(|a, b| {
        a.header
            .created_ts
            .cmp(&b.header.created_ts)
            .then(a.path.cmp(&b.path))
    });
    Ok(pages)
}

fn same_rollout_identity(a: &ParsedRollout, b: &ParsedRollout) -> bool {
    a.session_id == b.session_id
        && a.cwd == b.cwd
        && a.paginated == b.paginated
        && a.history_base == b.history_base
        && a.forked_from == b.forked_from
        && a.created_ts == b.created_ts
        && a.source == b.source
        && a.thread_source == b.thread_source
}

/// 删除实现（root 与移除方式均可注入，便于测试）。
/// 仅允许单文件或已证明属于同一分页链的整组文件；全部复核完成后一次移入回收站。
fn delete_session_with(
    root: &Path,
    project_dir: &str,
    session_id: &str,
    remove: impl Fn(Vec<PathBuf>) -> Result<(), String>,
) -> Result<(), String> {
    if session_id.len() > 64 || session_id.contains(['/', '\\', '\'']) {
        return Err("非法会话 ID".into());
    }
    let pages = deletion_pages(root, session_id)?;
    let latest = pages.last().ok_or("未找到会话文件")?;
    if pages.len() > 1 && !pages.iter().all(|p| p.header.paginated) {
        return Err("存在多个无法确认归属同一分页链的会话文件，已中止删除".into());
    }
    let (chain, issues) = history::resolve(&pages);
    if !issues.is_empty() || chain.len() != pages.len() {
        return Err("无法确认会话文件属于完整的同一分页链，已中止删除".into());
    }
    if to_meta(&latest.header, None).project_dir != project_dir {
        return Err("会话所属项目与请求不匹配，已中止删除".into());
    }
    let mut targets = Vec::new();
    for page in &pages {
        let target = super::verify_within_root(root, &page.path, super::TargetKind::File)?;
        match parse_rollout(&target, false) {
            super::ParseOutcome::Ok(p) if same_rollout_identity(&p, &page.header) => {
                targets.push(target)
            }
            super::ParseOutcome::Ok(_) => return Err("目标文件身份已变化，已中止删除".into()),
            super::ParseOutcome::Degraded(_, w) | super::ParseOutcome::Failed(w) => {
                return Err(format!(
                    "目标文件解析不完整（{w}），无法完成身份复核，已中止删除"
                ))
            }
        }
    }
    let fresh = deletion_pages(root, session_id)?;
    let (fresh_chain, fresh_issues) = history::resolve(&fresh);
    if fresh.len() != pages.len()
        || fresh
            .iter()
            .zip(&pages)
            .any(|(a, b)| a.path != b.path || !same_rollout_identity(&a.header, &b.header))
        || !fresh_issues.is_empty()
        || fresh_chain.len() != fresh.len()
        || targets
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            != targets.len()
    {
        return Err("会话分页集合已变化，请刷新后重试".into());
    }
    remove(targets)
}

pub struct CodexSource;

impl AgentSource for CodexSource {
    fn id(&self) -> &'static str {
        "codex"
    }
    fn display_name(&self) -> &'static str {
        "Codex"
    }
    fn is_installed(&self) -> bool {
        sessions_root().exists()
    }
    fn data_dir(&self) -> Option<PathBuf> {
        sessions_root().parent().map(|p| p.to_path_buf())
    }
    fn watch_paths(&self, usage: bool) -> super::AgentWatchPaths {
        let mut paths = super::AgentWatchPaths::transcripts("sessions", None);
        if usage {
            paths
                .transcript_dirs
                .push(PathBuf::from("archived_sessions"));
        }
        paths.root_files = vec!["session_index.jsonl", ".codex-global-state.json"];
        paths
    }
    fn catalog(&self, include_empty: bool) -> Result<super::Catalog, String> {
        catalog_in(&sessions_root(), include_empty)
    }
    fn quick_catalog(&self, include_empty: bool) -> Result<super::Catalog, String> {
        catalog_with(&sessions_root(), include_empty, true, &|| false)
    }
    fn supports_fast_catalog(&self) -> bool {
        true
    }
    fn refine_catalog(
        &self,
        include_empty: bool,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<super::Catalog, String> {
        refine(include_empty, cancelled)
    }
    fn read_tool_payload(&self, token: &str) -> Option<Result<Value, String>> {
        // 旧 Codex 令牌无前缀；只识别既有 SHA-256 形式，实际读取仍须命中引用表。
        (token.len() == 64 && token.bytes().all(|b| b.is_ascii_hexdigit()))
            .then(|| deferred::read(token))
    }
    fn read_session_view(
        &self,
        _project_dir: &str,
        session_id: &str,
    ) -> Result<SessionDetail, String> {
        read_session_mode(&sessions_root(), session_id, ReadMode::View)
    }
    fn read_session(&self, _project_dir: &str, session_id: &str) -> Result<SessionDetail, String> {
        read_session_in(&sessions_root(), session_id)
    }
    fn search_events(&self, meta: &SessionMeta) -> Result<Vec<Value>, String> {
        let mut warnings = Vec::new();
        let pages = history::headers(meta.source_files.clone(), &mut warnings);
        if !warnings.is_empty()
            || pages.len() != meta.source_files.len()
            || pages.iter().any(|p| p.header.session_id != meta.session_id)
        {
            return Err("会话来源已变化，请刷新后重试".into());
        }
        let (detail, ws) = history::read(&pages, ReadMode::Events)?;
        if !ws.is_empty() || detail.meta.project_dir != meta.project_dir {
            return Err("会话来源已变化，请刷新后重试".into());
        }
        Ok(detail.events)
    }
    fn delete_session(&self, project_dir: &str, session_id: &str) -> Result<(), String> {
        delete_session_with(&sessions_root(), project_dir, session_id, trash_remove)
    }
    fn resume_command(&self, session_id: &str, _fork: bool) -> Option<String> {
        Some(format!(
            "{} resume {}",
            super::shell_quote("codex"),
            super::shell_quote(session_id)
        ))
    }
    fn config_spec(&self) -> Vec<ConfigFile> {
        vec![ConfigFile {
            key: "config.toml".into(),
            label: "config.toml".into(),
            path: codex_home().join("config.toml"),
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_spec_resolves_config_toml_under_codex_home() {
        // 单一 config.toml，路径由 codex_home() 解析（尊重 CODEX_HOME）
        let spec = CodexSource.config_spec();
        assert_eq!(spec.len(), 1);
        assert_eq!(spec[0].key, "config.toml");
        assert_eq!(spec[0].path, codex_home().join("config.toml"));
    }

    fn rollout_fixture() -> tempfile::NamedTempFile {
        use std::io::Write;
        let mut f = tempfile::Builder::new()
            .prefix("rollout-2026-09-03T00-00-00-")
            .suffix("-abcdef01.jsonl")
            .tempfile()
            .unwrap();
        let lines = [
            r#"{"timestamp":"2026-09-03T01:00:00Z","type":"session_meta","payload":{"id":"abcdef01","cwd":"/tmp/demo","cli_version":"0.30.0"}}"#,
            r#"{"timestamp":"2026-09-03T01:00:01Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"<environment_context>\ncli\n</environment_context>\n帮我写个函数"}]}}"#,
            r#"{"timestamp":"2026-09-03T01:00:02Z","type":"response_item","payload":{"type":"reasoning","summary":[{"type":"summary_text","text":"先分析需求"}]}}"#,
            r#"{"timestamp":"2026-09-03T01:00:03Z","type":"response_item","payload":{"type":"function_call","name":"shell","call_id":"c1","arguments":"{\"cmd\":[\"ls\"]}"}}"#,
            r#"{"timestamp":"2026-09-03T01:00:04Z","type":"response_item","payload":{"type":"function_call_output","call_id":"c1","output":"{\"output\":\"file.txt\"}"}}"#,
            r#"{"timestamp":"2026-09-03T01:00:05Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"完成了"}]}}"#,
            r#"{"timestamp":"2026-09-03T01:00:06Z","type":"event_msg","payload":{"type":"token_count"}}"#,
        ];
        for line in lines {
            writeln!(f, "{line}").unwrap();
        }
        f
    }

    #[test]
    fn search_reads_catalog_locator_with_the_same_event_order_as_detail() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("sessions");
        let source = rollout_fixture();
        let path = root.join("2099/12/31/rollout-search.jsonl");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::copy(source.path(), &path).unwrap();
        let catalog = catalog_in(&root, false).unwrap();
        let meta = &catalog.sessions[0];
        let events = CodexSource.search_events(meta).unwrap();
        let detail = read_session_in(&root, &meta.session_id).unwrap();
        assert_eq!(events, detail.events);
        let mut wrong = meta.clone();
        wrong.session_id = "wrong-session".into();
        assert!(CodexSource.search_events(&wrong).is_err());
    }

    #[test]
    fn classify_source_three_states() {
        use serde_json::json;
        // 字段缺失 = 旧版 rollout
        assert_eq!(classify_source(&json!({})), SourceKind::Missing);
        // 交互来源
        assert_eq!(
            classify_source(&json!({"source":"cli"})),
            SourceKind::Interactive
        );
        assert_eq!(
            classify_source(&json!({"source":"vscode"})),
            SourceKind::Interactive
        );
        // 其余字符串 = 非交互
        assert_eq!(
            classify_source(&json!({"source":"exec"})),
            SourceKind::NonInteractive
        );
        assert_eq!(
            classify_source(&json!({"source":"appServer"})),
            SourceKind::NonInteractive
        );
        // 对象形式（subagent thread_spawn）= 非交互，绝不能落入 Missing
        let obj =
            json!({"source":{"subagent":{"thread_spawn":{"parent_thread_id":"x","depth":1}}}});
        assert_eq!(classify_source(&obj), SourceKind::NonInteractive);
    }

    #[test]
    fn relevance_matrix_source_vs_thread_source() {
        fn with(source: SourceKind, thread_source: Option<&str>) -> ParsedRollout {
            ParsedRollout {
                session_id: "x".into(),
                cwd: None,
                version: None,
                model: None,
                originator: None,
                source,
                thread_source: thread_source.map(String::from),
                created_ts: None,
                paginated: false,
                history_base: None,
                forked_from: None,
                first_ts: None,
                last_ts: None,
                user_count: 0,
                assistant_count: 0,
                real_user_count: 1,
                assistant_text_count: 0,
                first_user_text: None,
                events: Vec::new(),
                size_bytes: 0,
            }
        }
        // 可交互：交互来源 × 用户线程（vscode+user 曾被旧实现误过滤）
        assert!(is_relevant_session(&with(
            SourceKind::Interactive,
            Some("user")
        )));
        // 旧版 rollout 两字段缺失，按可交互处理
        assert!(is_relevant_session(&with(SourceKind::Missing, None)));
        // 非用户线程
        assert!(!is_relevant_session(&with(
            SourceKind::Interactive,
            Some("onboarding_checklist")
        )));
        assert!(!is_relevant_session(&with(
            SourceKind::Missing,
            Some("subagent")
        )));
        // 非交互来源：对象形式 source 即使用户线程也不相关（曾被误判为旧版交互）
        assert!(!is_relevant_session(&with(
            SourceKind::NonInteractive,
            Some("user")
        )));
        assert!(!is_relevant_session(&with(
            SourceKind::NonInteractive,
            None
        )));
    }

    #[test]
    fn rollout_without_session_meta_is_failed_not_filename_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir
            .path()
            .join("rollout-2099-12-31T00-00-00-deadbeef.jsonl");
        // 只有 response_item、没有 session_meta：不得从文件名兜底出 ID
        fs::write(
            &path,
            r#"{"timestamp":"2099-12-31T00:00:01Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"hi"}]}}"#,
        )
        .unwrap();
        let crate::agents::ParseOutcome::Failed(msg) = parse_rollout(&path, false) else {
            panic!("缺少 session_meta 必须判为解析失败");
        };
        assert!(msg.contains("session_meta"), "{msg}");
    }

    #[test]
    fn catalog_ignores_zero_byte_rollouts_without_warnings() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("sessions");
        let day = root.join("2099/12/31");
        fs::create_dir_all(&day).unwrap();
        let empty = day.join("rollout-empty.jsonl");
        fs::write(&empty, "").unwrap();
        let valid = day.join("rollout-valid.jsonl");
        fs::copy(rollout_fixture().path(), &valid).unwrap();

        for include_empty in [false, true] {
            let catalog = catalog_in(&root, include_empty).unwrap();
            assert!(catalog.warnings.is_empty(), "{:?}", catalog.warnings);
            assert_eq!(catalog.sessions.len(), 1);
            assert_eq!(catalog.sessions[0].session_id, "abcdef01");
            assert_eq!(fs::metadata(&empty).unwrap().len(), 0);
        }

        // 首次扫描时为空的文件写入有效会话后，下次扫描必须能发现。
        fs::remove_file(&valid).unwrap();
        fs::copy(rollout_fixture().path(), &empty).unwrap();
        let catalog = catalog_in(&root, false).unwrap();
        assert!(catalog.warnings.is_empty(), "{:?}", catalog.warnings);
        assert_eq!(catalog.sessions.len(), 1);
        assert_eq!(catalog.sessions[0].session_id, "abcdef01");
    }

    #[test]
    fn catalog_warns_on_nonempty_rollouts_without_session_meta() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("sessions");
        let day = root.join("2099/12/31");
        fs::create_dir_all(&day).unwrap();
        let path = day.join("rollout-invalid.jsonl");
        for content in [
            "invalid JSON",
            r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"hi"}]}}"#,
            r#"{"type":"session_meta","payload":{}}"#,
        ] {
            fs::write(&path, content).unwrap();
            for include_empty in [false, true] {
                let catalog = catalog_in(&root, include_empty).unwrap();
                assert!(catalog.sessions.is_empty());
                assert_eq!(catalog.warnings.len(), 1, "{:?}", catalog.warnings);
                assert!(catalog.warnings[0].contains("session_meta"));
            }
        }
    }

    #[test]
    fn catalog_keeps_identified_empty_sessions_when_requested() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("sessions");
        write_delete_fixture(
            &root,
            "2099/12/31/rollout-meta-only.jsonl",
            "empty-id",
            "/tmp/demo",
        );

        for include_empty in [false, true] {
            let catalog = catalog_in(&root, include_empty).unwrap();
            assert!(catalog.warnings.is_empty(), "{:?}", catalog.warnings);
            assert_eq!(catalog.sessions.len(), usize::from(include_empty));
            if include_empty {
                assert_eq!(catalog.sessions[0].session_id, "empty-id");
            }
        }
    }

    #[test]
    fn catalog_warns_on_special_rollout_files() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("sessions");
        let day = root.join("2099/12/31");
        fs::create_dir_all(day.join("rollout-directory.jsonl")).unwrap();
        let fifo = day.join("rollout-fifo.jsonl");
        let c_path = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);
        std::os::unix::fs::symlink(day.join("missing.jsonl"), day.join("rollout-broken.jsonl"))
            .unwrap();

        let catalog = catalog_in(&root, false).unwrap();
        assert!(catalog.sessions.is_empty());
        assert_eq!(catalog.warnings.len(), 3, "{:?}", catalog.warnings);
        assert_eq!(
            catalog
                .warnings
                .iter()
                .filter(|w| w.contains("不是普通文件"))
                .count(),
            2
        );
    }

    #[test]
    fn parses_rollout_into_unified_events() {
        let f = rollout_fixture();
        let (parsed, _) = parse_rollout(f.path(), true).split();
        let Some(p) = parsed else {
            panic!("fixture 应可解析");
        };
        assert_eq!(p.session_id, "abcdef01");
        assert_eq!(p.cwd.as_deref(), Some("/tmp/demo"));
        assert_eq!(p.version.as_deref(), Some("0.30.0"));
        assert_eq!(p.real_user_count, 1);
        assert_eq!(p.assistant_count, 3); // reasoning + tool_use + message
        assert_eq!(p.assistant_text_count, 1); // 只有 message 含正文
        assert_eq!(p.first_user_text.as_deref(), Some("帮我写个函数"));

        // 事件结构可被前端直接渲染
        let types: Vec<&str> = p
            .events
            .iter()
            .map(|e| e.get("type").and_then(Value::as_str).unwrap())
            .collect();
        assert_eq!(
            types,
            vec![
                "context",
                "user",
                "assistant",
                "assistant",
                "user",
                "assistant"
            ]
        );

        let tool_use = &p.events[3]["message"]["content"][0];
        assert_eq!(tool_use["type"], "tool_use");
        assert_eq!(tool_use["name"], "shell");
        assert_eq!(tool_use["input"]["cmd"][0], "ls");

        let tool_result = &p.events[4]["message"]["content"][0];
        assert_eq!(tool_result["type"], "tool_result");
        assert_eq!(tool_result["content"], "file.txt");
    }

    /// 集成测试：注入临时目录作为会话根，走完 catalog/read/delete 全链路。
    /// 不用环境变量、不读写真实 ~/.codex；tempdir 退出即清理，
    /// panic 也不会留下副作用；删除走注入的 remove，不碰真实回收站
    #[test]
    fn integration_full_flow_with_cleanup() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("sessions");
        let (id_main, id_gui, id_sub) = (
            "it-main".to_string(),
            "it-gui".to_string(),
            "it-sub".to_string(),
        );

        let day_dir = root.join("2099").join("12").join("31");
        fs::create_dir_all(&day_dir).unwrap();

        let main_content = [
            format!(r#"{{"timestamp":"2099-12-31T00:00:00Z","type":"session_meta","payload":{{"id":"{id_main}","cwd":"/tmp/codex-it","cli_version":"0.30.0","originator":"codex_cli","source":"cli","thread_source":"user"}}}}"#),
            r#"{"timestamp":"2099-12-31T00:00:01Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"集成测试消息"}]}}"#.to_string(),
            r#"{"timestamp":"2099-12-31T00:00:02Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"收到"}]}}"#.to_string(),
        ]
        .join("\n");
        let main_file = day_dir.join("rollout-2099-12-31T00-00-00-it-a.jsonl");
        fs::write(&main_file, &main_content).unwrap();

        // onboarding 引导会话（GUI 安装流程产生），应被过滤
        let gui_content = [
            format!(r#"{{"timestamp":"2099-12-31T00:00:00Z","type":"session_meta","payload":{{"id":"{id_gui}","cwd":"/tmp/codex-it","originator":"Codex Desktop","source":"vscode","thread_source":"onboarding_checklist"}}}}"#),
            r#"{"timestamp":"2099-12-31T00:00:01Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"引导会话"}]}}"#.to_string(),
        ]
        .join("\n");
        fs::write(
            day_dir.join("rollout-2099-12-31T00-00-00-it-b.jsonl"),
            &gui_content,
        )
        .unwrap();

        // 子代理会话：source 是对象（非字符串）、thread_source=subagent，应被过滤
        let sub_content = [
            format!(r#"{{"timestamp":"2099-12-31T00:00:00Z","type":"session_meta","payload":{{"id":"{id_sub}","cwd":"/tmp/codex-it","originator":"Codex Desktop","source":{{"subagent":{{"thread_spawn":{{"parent_thread_id":"{id_main}","depth":1}}}}}},"thread_source":"subagent"}}}}"#),
            r#"{"timestamp":"2099-12-31T00:00:01Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"子代理任务"}]}}"#.to_string(),
        ]
        .join("\n");
        fs::write(
            day_dir.join("rollout-2099-12-31T00-00-00-it-c.jsonl"),
            &sub_content,
        )
        .unwrap();

        // 标题索引与 sessions 根同级（home），覆盖 load_thread_names_in 路径
        fs::write(
            tmp.path().join("session_index.jsonl"),
            format!(r#"{{"id":"{id_main}","thread_name":"集成测试消息"}}"#),
        )
        .unwrap();

        let catalog = catalog_in(&root, false).unwrap();
        let project_id = escape_cwd("/tmp/codex-it");
        let ours: Vec<_> = catalog
            .sessions
            .iter()
            .filter(|s| s.session_id.starts_with("it-"))
            .collect();
        assert_eq!(ours.len(), 1, "onboarding/subagent 会话应被过滤");
        assert_eq!(ours[0].session_id, id_main);
        assert_eq!(ours[0].title.as_deref(), Some("集成测试消息"));
        let in_project = ours.iter().filter(|s| s.project_dir == project_id).count();
        assert_eq!(in_project, 1, "项目分组应包含该会话");

        let detail = read_session_in(&root, &id_main).unwrap();
        assert_eq!(detail.events.len(), 2);

        let cmd = CodexSource.resume_command(&id_main, false).unwrap();
        assert!(cmd.contains("resume"));
        assert!(cmd.contains(&id_main));

        // 删除全链路：注入的 remove 只作用于临时目录内
        delete_session_with(&root, &project_id, &id_main, |paths| {
            paths
                .into_iter()
                .try_for_each(|p| fs::remove_file(p).map_err(|e| e.to_string()))
        })
        .unwrap();
        assert!(!main_file.exists(), "匹配 id 的会话应被注入方式移除");
    }

    #[test]
    fn delete_refuses_partially_corrupt_rollout() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("sessions");
        // session_meta 身份正确，但后续含损坏行：Degraded 匹配必须被拒绝，
        // 错误发生在 remove 被调用之前，无任何外部副作用
        let day_dir = root.join("2099").join("12").join("31");
        fs::create_dir_all(&day_dir).unwrap();
        let file = day_dir.join("rollout-2099-12-31T00-00-00-c0ffee01.jsonl");
        fs::write(
            &file,
            [
                r#"{"timestamp":"2099-12-31T00:00:00Z","type":"session_meta","payload":{"id":"c0ffee01","cwd":"/tmp/x"}}"#,
                "这一行不是合法 JSON",
            ]
            .join("\n"),
        )
        .unwrap();

        let err = delete_session_with(&root, &escape_cwd("/tmp/x"), "c0ffee01", |_| {
            panic!("部分损坏的 rollout 不得调用 remove")
        })
        .unwrap_err();
        assert!(err.contains("解析不完整"), "{err}");
        assert!(file.exists(), "部分损坏的 rollout 不得删除");
    }

    fn write_delete_fixture(root: &Path, relative_path: &str, id: &str, cwd: &str) -> PathBuf {
        let path = root.join(relative_path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            json!({"type": "session_meta", "payload": {"id": id, "cwd": cwd}}).to_string(),
        )
        .unwrap();
        path
    }

    #[test]
    fn delete_refuses_wrong_project_without_removal() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let file = write_delete_fixture(
            root,
            "2099/12/31/rollout-unrelated-name.jsonl",
            "delete-me",
            "/tmp/codex-delete",
        );
        for _ in 0..2 {
            let err = delete_session_with(root, &escape_cwd("/tmp/other"), "delete-me", |_| {
                panic!("错误项目不得调用 remove")
            })
            .unwrap_err();
            assert!(err.contains("项目"), "{err}");
            assert!(file.exists());
        }
    }

    fn assert_duplicate_deletion_refused(second_cwd: &str) {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let first_cwd = "/tmp/codex-delete";
        let first = write_delete_fixture(
            root,
            "2099/12/30/rollout-first.jsonl",
            "duplicate-id",
            first_cwd,
        );
        let second = write_delete_fixture(
            root,
            "2099/12/31/rollout-second.jsonl",
            "duplicate-id",
            second_cwd,
        );
        for cwd in [first_cwd, second_cwd] {
            let err = delete_session_with(root, &escape_cwd(cwd), "duplicate-id", |_| {
                panic!("重复 ID 不得调用 remove")
            })
            .unwrap_err();
            assert!(err.contains("多个"), "{err}");
            assert!(first.exists());
            assert!(second.exists());
        }
    }

    #[test]
    fn delete_refuses_duplicate_id_in_same_project_without_removal() {
        assert_duplicate_deletion_refused("/tmp/codex-delete");
    }

    #[test]
    fn delete_refuses_duplicate_id_across_projects_without_removal() {
        assert_duplicate_deletion_refused("/tmp/other");
    }

    #[test]
    fn delete_refuses_degraded_match_even_with_complete_candidate() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let complete = write_delete_fixture(
            root,
            "2099/12/30/rollout-complete.jsonl",
            "duplicate-id",
            "/tmp/codex-delete",
        );
        let degraded = write_delete_fixture(
            root,
            "2099/12/31/rollout-degraded.jsonl",
            "duplicate-id",
            "/tmp/other",
        );
        fs::write(
            &degraded,
            format!("{}\ninvalid JSON", fs::read_to_string(&degraded).unwrap()),
        )
        .unwrap();
        let err = delete_session_with(
            root,
            &escape_cwd("/tmp/codex-delete"),
            "duplicate-id",
            |_| panic!("匹配的降级记录存在时不得调用 remove"),
        )
        .unwrap_err();
        assert!(err.contains("解析不完整"), "{err}");
        assert!(complete.exists());
        assert!(degraded.exists());
    }

    #[test]
    fn delete_unique_correct_project_ignores_unrelated_corruption() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let file = write_delete_fixture(
            root,
            "2099/12/31/rollout-unrelated-name.jsonl",
            "delete-me",
            "/tmp/codex-delete",
        );
        let unrelated = write_delete_fixture(
            root,
            "2099/12/30/rollout-other.jsonl",
            "other-id",
            "/tmp/codex-delete",
        );
        let degraded = write_delete_fixture(
            root,
            "2099/12/30/rollout-degraded.jsonl",
            "other-degraded-id",
            "/tmp/codex-delete",
        );
        fs::write(
            &degraded,
            format!("{}\ninvalid JSON", fs::read_to_string(&degraded).unwrap()),
        )
        .unwrap();
        // 无法解析身份的文件既不能当目标，也不能据此证明它与目标无关。
        let failed = root.join("2099/12/30/rollout-delete-me.jsonl");
        fs::write(&failed, r#"{"type":"session_meta","payload":{}}"#).unwrap();
        let err = delete_session_with(root, &escape_cwd("/tmp/codex-delete"), "delete-me", |_| {
            panic!("身份未知时不得调用 remove")
        })
        .unwrap_err();
        assert!(err.contains("身份无法确认"), "{err}");
        assert!(file.exists() && failed.exists());
        fs::remove_file(&failed).unwrap();

        // 某层目录不可遍历，也不能证明扫描结果完整。
        let blocked = root.join("2098");
        fs::write(&blocked, "not a directory").unwrap();
        let err = delete_session_with(root, &escape_cwd("/tmp/codex-delete"), "delete-me", |_| {
            panic!("目录扫描不完整时不得调用 remove")
        })
        .unwrap_err();
        assert!(err.contains("目录扫描不完整"), "{err}");
        fs::remove_file(blocked).unwrap();

        let target = file.canonicalize().unwrap();
        let calls = std::cell::Cell::new(0);
        let project_dir = escape_cwd("/tmp/codex-delete");
        delete_session_with(root, &project_dir, "delete-me", |p| {
            calls.set(calls.get() + 1);
            assert_eq!(p, vec![target.clone()]);
            p.into_iter()
                .try_for_each(|path| fs::remove_file(path).map_err(|e| e.to_string()))
        })
        .unwrap();
        assert_eq!(calls.get(), 1);
        assert!(!file.exists());

        let err = delete_session_with(root, &project_dir, "delete-me", |_| {
            panic!("重复请求不得删除无关文件")
        })
        .unwrap_err();
        assert!(err.contains("未找到"), "{err}");
        assert!(unrelated.exists());
        assert!(degraded.exists());
    }

    /// 真实数据冒烟测试：本机装有 Codex 时验证所有 rollout 都能解析（只读）
    #[test]
    fn real_data_smoke_if_present() {
        let mut warnings = Vec::new();
        let root = sessions_root();
        let files = all_rollouts_in(&root, &mut warnings);
        if files.is_empty() {
            return;
        }
        let catalog = catalog_in(&root, false).unwrap();
        assert!(catalog.warnings.is_empty(), "{:?}", catalog.warnings);
        eprintln!(
            "Codex catalog smoke: {} sessions, {} warnings",
            catalog.sessions.len(),
            catalog.warnings.len()
        );
        let thread_names = root
            .parent()
            .map(|home| load_thread_names_in(home, &mut warnings))
            .unwrap_or_default();
        for f in &files {
            let (parsed, _) = parse_rollout(f, false).split();
            let Some(p) = parsed else {
                panic!("真实 rollout 应可解析: {f:?}");
            };
            assert!(!p.session_id.is_empty(), "session_id 为空: {f:?}");
            let meta = to_meta(&p, thread_names.get(&p.session_id));
            assert!(meta.title.is_none() || !meta.title.as_deref().unwrap().is_empty());
        }
    }
}

pub(crate) fn refine(
    include_empty: bool,
    cancelled: &dyn Fn() -> bool,
) -> Result<super::Catalog, String> {
    catalog_with(&sessions_root(), include_empty, false, cancelled)
}
