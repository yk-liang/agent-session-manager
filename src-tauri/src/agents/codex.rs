//! 读取 Codex CLI、IDE 扩展和桌面端的本地会话，包括归档。运行指令和状态通知作为上下文保留。

mod app_server;
mod catalog;
#[cfg(test)]
mod compressed_tests;
mod context;
#[cfg(test)]
mod context_tests;
pub(crate) mod deferred;
mod deletion;
mod exec;
mod history;
#[cfg(test)]
mod history_tests;
mod projects;
mod response;
mod storage;
mod thread;
mod tool_output;
mod tools;
mod wrappers;

use serde::Deserialize;
use serde_json::{json, value::RawValue, Value};
#[cfg(test)]
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use super::deletion::{DeletePolicy, Outcome, PreserveUsage};
use super::{AgentSource, CatalogFilter, ConfigFile, SessionDetail, SessionMeta};
use app_server::ThreadAction;
pub use thread::ThreadMeta;
use thread::ThreadHeader;

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

/// 只读的线程历史库，用量统计从中补充 rollout 未记录的模型调用失败。
pub(crate) const HISTORY_DB: &str = "thread_history_1.sqlite";

pub(crate) fn history_database(home: &Path) -> PathBuf {
    sqlite_home(home).join(HISTORY_DB)
}

pub(crate) fn sqlite_home(home: &Path) -> PathBuf {
    std::fs::read_to_string(home.join("config.toml"))
        .ok()
        .and_then(|text| toml::from_str::<toml::Value>(&text).ok())
        .and_then(|config| config.get("sqlite_home")?.as_str().map(str::to_owned))
        .or_else(|| std::env::var("CODEX_SQLITE_HOME").ok())
        .filter(|value| !value.trim().is_empty())
        .map(|value| home.join(super::expand_tilde(value.trim())))
        .unwrap_or_else(|| home.to_path_buf())
}

fn escape_cwd(cwd: &str) -> String {
    let normalized = super::workspace_groups::normalized(Path::new(cwd));
    super::escape_path_id(&normalized.to_string_lossy())
}

#[derive(Clone)]
struct ParsedRollout {
    session_id: String,
    cwd: Option<String>,
    version: Option<String>,
    git_branch: Option<String>,
    /// 有效历史中最近一次记录的模型；未记录时保持未知，不使用当前配置推测。
    model: Option<String>,
    /// 写入方标识：codex_cli / Codex Desktop 等（不能用于区分交互性）
    originator: Option<String>,
    source: Option<Value>,
    thread: ThreadHeader,
    created_ts: Option<String>,
    paginated: bool,
    history_base: Option<history::HistoryBase>,
    forked_from: Option<String>,
    attached_thread: bool,
    /// 子智能体分叉时，序号小于该值的记录是复制来的父会话上下文。
    fork_start: Option<u64>,
    /// 父会话上下文生成的事件数；仅读取正文时统计。
    inherited: usize,
    first_ts: Option<String>,
    last_ts: Option<String>,
    user_count: u32,
    assistant_count: u32,
    real_user_count: u32,
    assistant_text_count: u32,
    has_activity: bool,
    first_user_text: Option<String>,
    events: Vec<Value>,
    size_bytes: u64,
    /// 正文字节数；压缩页只读文件头时未知，完整读取后补齐，供分页边界与删除复核使用。
    decoded_size_bytes: Option<u64>,
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

#[cfg(test)]
fn parse_rollout(path: &Path, collect_events: bool) -> super::ParseOutcome<ParsedRollout> {
    let mut activity = wrappers::Activity::default();
    let mut parsed = read_rollout(
        path,
        if collect_events {
            ReadMode::Events
        } else {
            ReadMode::Summary
        },
        ReadLimit::default(),
        None,
        &mut activity,
    );
    match &mut parsed {
        super::ParseOutcome::Ok(rollout) | super::ParseOutcome::Degraded(rollout, _) => {
            activity.remove_redundant_scripts(&mut rollout.events, &mut rollout.inherited);
        }
        super::ParseOutcome::Failed(_) => {}
    }
    parsed
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
struct ToolHeader<'a> {
    #[serde(rename = "type")]
    kind: Option<String>,
    id: Option<String>,
    call_id: Option<String>,
    name: Option<String>,
    #[serde(borrow)]
    output: Option<&'a RawValue>,
    #[serde(borrow)]
    arguments: Option<&'a RawValue>,
    #[serde(borrow)]
    input: Option<&'a RawValue>,
    #[serde(borrow)]
    internal_chat_message_metadata_passthrough: Option<&'a RawValue>,
}

#[derive(Deserialize)]
struct ThreadId {
    id: Option<String>,
}

#[derive(Deserialize)]
struct ExecMetadata {
    turn_id: String,
}

#[derive(Deserialize)]
struct ModelContext {
    model: Option<String>,
}

#[derive(Deserialize)]
struct RuntimeEvent<'a> {
    #[serde(rename = "type")]
    kind: String,
    thread_id: Option<String>,
    turn_id: Option<String>,
    thread_settings: Option<ModelContext>,
    reason: Option<String>,
    error: Option<RuntimeError>,
    #[serde(borrow)]
    item: Option<&'a RawValue>,
    started_at_ms: Option<i64>,
}

#[derive(Deserialize)]
struct RuntimeError {
    message: Option<String>,
}

#[derive(Deserialize)]
struct Compacted {
    message: Option<String>,
}

fn update_model(model: &mut Option<String>, next: Option<String>) {
    if let Some(next) = next.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()) {
        *model = Some(next);
    }
}

fn read_rollout(
    path: &Path,
    mode: ReadMode,
    limit: ReadLimit,
    mut dedup: Option<&mut history::EventDedup>,
    activity: &mut wrappers::Activity,
) -> super::ParseOutcome<ParsedRollout> {
    let file = match storage::Reader::open(path) {
        Ok(f) => f,
        Err(e) => return super::ParseOutcome::Failed(format!("无法读取 rollout {:?}: {e}", path)),
    };
    let size_bytes = file.metadata().len();
    let identity = (file.metadata().dev(), file.metadata().ino());
    let decoded_size_bytes = file.decoded_size_hint();
    // history_base 的字节边界限定旧页有效前缀，不能把分页后补写的旧尾部混进当前历史。
    let mut reader =
        BufReader::with_capacity(64 * 1024, file.take(limit.bytes.unwrap_or(u64::MAX)));

    let mut parsed = ParsedRollout {
        session_id: String::new(),
        cwd: None,
        version: None,
        git_branch: None,
        model: None,
        originator: None,
        source: None,
        thread: ThreadHeader::default(),
        created_ts: None,
        paginated: false,
        history_base: None,
        forked_from: None,
        attached_thread: false,
        fork_start: None,
        inherited: 0,
        first_ts: None,
        last_ts: None,
        user_count: 0,
        assistant_count: 0,
        real_user_count: 0,
        assistant_text_count: 0,
        has_activity: false,
        first_user_text: None,
        events: Vec::new(),
        size_bytes,
        decoded_size_bytes,
    };

    let mut bad_lines = 0;
    let mut seen_meta = false;
    let mut copies_parent = false;
    let mut in_fork_copy = false;
    let mut line = Vec::new();
    let mut offset = 0u64;
    loop {
        line.clear();
        let len = match reader.read_until(b'\n', &mut line) {
            Ok(0) => break,
            Ok(len) => len,
            Err(error) => {
                return super::ParseOutcome::Failed(format!(
                    "无法读取或解压 rollout {:?}: {error}", path
                ));
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
                // 旧日志可包含合法的 JSON 标量，跳过这些非事件记录。
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
        if let (Some(start), Some(n), true) = (parsed.fork_start, ordinal, copies_parent) {
            if n < start {
                in_fork_copy = true;
            } else if in_fork_copy {
                in_fork_copy = false;
                parsed.inherited = parsed.events.len();
            }
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
        let collect_events = matches!(mode, ReadMode::Events | ReadMode::View);
        match rtype {
            "turn_context" | "token_usage_record" => {
                if let Ok(context) = serde_json::from_str::<ModelContext>(raw_payload) {
                    update_model(&mut parsed.model, context.model);
                }
                continue;
            }
            "event_msg" => {
                if let Ok(event) = serde_json::from_str::<RuntimeEvent>(raw_payload) {
                    if matches!(event.kind.as_str(), "task_started" | "task_complete" | "turn_started" | "turn_completed" | "turn_aborted" | "error") {
                        parsed.has_activity = true;
                    }
                    match event.kind.as_str() {
                        "thread_settings_applied"
                            if event.thread_id.as_deref().is_none_or(|id| id == parsed.session_id) =>
                        {
                            update_model(&mut parsed.model, event.thread_settings.and_then(|s| s.model));
                        }
                        "turn_aborted" if collect_events => {
                            let text = context::turn_aborted(
                                event.reason.as_deref(),
                                event.error.as_ref().and_then(|e| e.message.as_deref()),
                            );
                            parsed.events.push(json!({"type":"system","timestamp":ts,"cwd":parsed.cwd,
                                "content":text}));
                        }
                        "item_completed" if mode != ReadMode::Header => {
                            let Some(item) = event.item.filter(|item| serde_json::from_str::<tools::ItemHeader>(item.get())
                                .is_ok_and(|header| header.is_tool() && !header.id.is_some_and(|id| activity.calls.contains(id))))
                            else {
                                continue;
                            };
                            parsed.has_activity = true;
                            if !collect_events {
                                continue;
                            }
                            if dedup.as_deref_mut().is_some_and(|seen| {
                                !seen.include(&serde_json::from_slice(&line).unwrap_or(Value::Null))
                            }) {
                                continue;
                            }
                            let item: Value = serde_json::from_str(item.get()).unwrap_or(Value::Null);
                            let view = mode == ReadMode::View;
                            let blocks = tools::blocks(&item, view);
                            if blocks.is_empty() {
                                continue;
                            }
                            if let (Some(turn), Some(start)) = (event.turn_id, event.started_at_ms) {
                                activity.record_execution(turn, start, &item);
                            }
                            let token = view.then(|| deferred::register(path, identity, line_offset, &line));
                            for (index, mut block) in blocks.into_iter().enumerate() {
                                if let Some(token) = &token {
                                    block["deferred"] = json!(format!("{token}#{index}"));
                                }
                                let role = if block["type"] == "tool_use" { "assistant" } else { "user" };
                                let mut event = json!({"type":role,"timestamp":ts,
                                    "uuid":format!("{}:{index}", item["id"].as_str().unwrap_or_default()),
                                    "message":{"role":role,"content":[block]}});
                                if let (Some(model), "assistant") = (&parsed.model, role) {
                                    event["message"]["model"] = json!(model);
                                }
                                parsed.events.push(event);
                            }
                        }
                        _ => {}
                    }
                }
                continue;
            }
            "compacted" => {
                if collect_events {
                    let data = serde_json::from_str::<Compacted>(raw_payload).ok();
                    parsed.events.push(context::compaction(
                        data.as_ref().and_then(|p| p.message.as_deref()), ts, parsed.cwd.as_deref()));
                }
                continue;
            }
            "session_meta" if seen_meta => {
                // 分叉副本以父会话的 session_meta 开头；Guardian 等子智能体只有预置任务，没有父会话副本。
                copies_parent |= parsed.forked_from.is_some()
                    && serde_json::from_str::<ThreadId>(raw_payload).is_ok_and(|meta| meta.id == parsed.forked_from);
                continue;
            }
            "session_meta" => seen_meta = true,
            "response_item" if mode != ReadMode::Header => {}
            _ => continue,
        }
        let shallow: ToolHeader<'_> = serde_json::from_str(raw_payload).unwrap_or_default();
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
        parsed.has_activity |= tool || matches!(
            shallow.kind.as_deref(),
            Some("local_shell_call" | "local_shell_call_output" | "web_search_call" | "tool_search_call")
        );
        if collect_events {
            match (shallow.kind.as_deref(), &shallow.call_id) {
                (Some("function_call" | "custom_tool_call"), Some(id)) => {
                    activity.calls.insert(id.clone());
                    if shallow.name.as_deref() == Some("exec") {
                        let metadata = shallow.internal_chat_message_metadata_passthrough
                            .and_then(|raw| serde_json::from_str::<ExecMetadata>(raw.get()).ok());
                        let input = shallow.input.and_then(|input| serde_json::from_str::<String>(input.get()).ok());
                        if let (Some(metadata), Some(at), Some(input)) = (metadata, super::timestamp(ts), input) {
                            activity.start_exec(id, metadata.turn_id, at.timestamp_millis(), &input, parsed.cwd.as_deref());
                        }
                    }
                }
                (Some("custom_tool_call_output"), Some(id)) => {
                    if let (Some(at), Some(output)) = (super::timestamp(ts), shallow.output) {
                        activity.finish_exec(id, at.timestamp_millis(), output);
                    }
                }
                _ => {}
            }
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
                    parsed.git_branch = p.pointer("/git/branch").and_then(Value::as_str).map(str::to_owned);
                    parsed.cwd = p.get("cwd").and_then(Value::as_str).map(|s| s.to_string());
                    parsed.version = p
                        .get("cli_version")
                        .and_then(Value::as_str)
                        .map(|s| s.to_string());
                    parsed.originator = p
                        .get("originator")
                        .and_then(Value::as_str)
                        .map(|s| s.to_string());
                    parsed.source = p.get("source").cloned();
                    parsed.thread = ThreadHeader::read(p);
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
                    parsed.attached_thread = p.get("attached_thread").and_then(Value::as_bool).unwrap_or(false);
                    parsed.fork_start = p.get("subagent_history_start_ordinal").and_then(Value::as_u64);
                    if mode == ReadMode::Header {
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
                            let name = shallow.name.unwrap_or_else(|| "?".into());
                            let mut block = json!({"type":"tool_use","id":shallow.call_id.unwrap_or_default(),"name":name,"deferred":token});
                            let input = match (shallow.arguments, shallow.input) {
                                (Some(arguments), _) => serde_json::from_str::<String>(arguments.get()).ok()
                                    .and_then(|arguments| serde_json::from_str::<Value>(&arguments).ok()),
                                (None, Some(input)) => serde_json::from_str::<Value>(input.get()).ok(),
                                _ => None,
                            };
                            let ops = input.map(|input| tools::describe(&name, &input)).unwrap_or_default();
                            if !ops.is_empty() {
                                block["ops"] = json!(ops.into_iter().map(super::tool_ops::Op::summary).collect::<Vec<_>>());
                            }
                            block
                        } else {
                            let status = shallow.output.map(tool_output::raw_status).unwrap_or_default();
                            let mut block = json!({"type":"tool_result","tool_use_id":shallow.call_id.unwrap_or_default(),"deferred":token});
                            status.annotate(&mut block, shallow.kind.as_deref() == Some("function_call_output"));
                            block
                        };
                        let role = if call { "assistant" } else { "user" };
                        new_events.push(json!({"type":role,"timestamp":ts,"message":{"role":role,"content":[block]}}));
                    } else {
                        response::convert(p, ts, parsed.cwd.as_deref(), &mut new_events);
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
                    for ev in &mut new_events {
                        match ev.get("type").and_then(Value::as_str) {
                            Some("agent") => {
                                parsed.user_count += 1;
                                parsed.real_user_count += 1;
                            }
                            Some("user") => {
                                let content = &ev["message"]["content"];
                                if super::context::has_user_content(content) {
                                    parsed.user_count += 1;
                                    parsed.real_user_count += 1;
                                }
                                if let Some(s) = super::context::first_text(content) {
                                    if parsed.first_user_text.is_none() {
                                        parsed.first_user_text = Some(s.chars().take(80).collect());
                                    }
                                }
                            }
                            Some("assistant") => {
                                parsed.has_activity = true;
                                if collect_events {
                                    if let Some(model) = &parsed.model {
                                        ev["message"]["model"] = json!(model);
                                    }
                                }
                                parsed.assistant_count += 1;
                                if ev
                                    .pointer("/message/content")
                                    .is_some_and(super::context::has_text)
                                {
                                    parsed.assistant_text_count += 1;
                                }
                            }
                            _ => {}
                        }
                    }
                    if collect_events {
                        parsed.events.extend(new_events);
                    }
                }
            }
            _ => {} // 其余元数据不生成对话，避免与 response_item 重复。
        }
    }

    if in_fork_copy {
        parsed.inherited = parsed.events.len();
    }
    if mode != ReadMode::Header {
        if limit.bytes.is_some_and(|end| offset < end) {
            return super::ParseOutcome::Failed(format!(
                "历史分页被截断，读取边界超出解压后日志大小：{:?}",
                path
            ));
        }
        if limit.bytes.is_none() {
            parsed.decoded_size_bytes = Some(offset);
        }
    }

    // 会话 ID 必须来自 session_meta，删除时也以该身份为准。
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

/// 从 <home>/session_index.jsonl 读取会话标题（thread_name）；
/// 读取失败（非"不存在"）与损坏行进 warnings。home 可注入
fn load_thread_names_in(
    home: &Path,
    warnings: &mut Vec<String>,
) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    let path = home.join("session_index.jsonl");
    // 与会话日志读取同一套 FIFO 防护：非阻塞打开 + 普通文件复核，
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
        title: thread_name.cloned().or_else(|| p.first_user_text.clone()).or_else(|| p.thread.nickname.clone()),
        model: p.model.clone(),
        version: p.version.clone(),
        client: p.originator.clone(),
        git_branch: p.git_branch.clone(),
        entrypoint: None,
        codex: Some(p.thread.meta.clone()),
        subagent: None,
        first_ts: p.first_ts.clone(),
        last_ts: p.last_ts.clone(),
        user_count: p.user_count,
        assistant_count: p.assistant_count,
        real_user_count: p.real_user_count,
        assistant_text_count: p.assistant_text_count,
        size_bytes: Some(p.size_bytes),
        has_transcript: true,
        archived: false,
        stats_pending: false,
        source_files: Vec::new(),
    }
}

#[cfg(test)]
fn catalog_in(root: &Path, include_empty: bool) -> Result<super::Catalog, String> {
    let filter = CatalogFilter { include_empty, ..Default::default() };
    catalog::Scan::new(root, filter, &|| false)?.read(false, &|| false)
}

fn read_session_in(root: &Path, session_id: &str) -> Result<SessionDetail, String> {
    read_session_mode(root, session_id, ReadMode::Events)
}
fn read_session_mode(
    root: &Path,
    session_id: &str,
    mode: ReadMode,
) -> Result<SessionDetail, String> {
    if !valid_thread_id(session_id) {
        return Err("会话 ID 无效".into());
    }
    let mut warnings = Vec::new();
    let paths = storage::rollouts(root, true, &mut warnings, &|| false)?;
    let pages = history::read_headers(&paths, &mut warnings, &|| false)?;
    let groups = history::group_pages(pages);
    let pages = groups.get(session_id).ok_or("未找到该 Codex 会话")?;
    let (mut detail, ws) = history::read(pages, mode)?;
    thread::Relations::new(&groups, thread::spawn_edges_warned(root, &mut warnings))
        .apply(session_id, &mut detail.meta);
    if detail.inherited > 0 {
        let parent = pages[0].header.forked_from.as_ref().and_then(|id| groups.get(id));
        // 父会话缺少分页时可能只剩部分历史，此时保留子会话自带的副本。
        if let Some(Ok((parent, ws))) = parent.map(|pages| history::read(pages, mode)) {
            if ws.is_empty() {
                history::adopt_parent_history(&mut detail, parent.events);
            }
        }
    }
    detail.meta.archived = storage::is_archived(root, &pages.last().unwrap().path);
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

fn valid_thread_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn same_rollout_identity(a: &ParsedRollout, b: &ParsedRollout) -> bool {
    a.session_id == b.session_id
        && a.cwd == b.cwd
        && a.paginated == b.paginated
        && a.history_base == b.history_base
        && a.forked_from == b.forked_from
        && a.attached_thread == b.attached_thread
        && a.created_ts == b.created_ts
        && a.source == b.source
        && a.thread == b.thread
}

pub(crate) fn transcript_session_id(path: &PathBuf) -> Result<Option<String>, String> {
    if std::fs::metadata(path)
        .map_err(|error| format!("无法读取 rollout {path:?}：{error}"))?
        .len() == 0
    {
        return Ok(None);
    }
    let (header, warnings) = history::read_header(path)?;
    match header {
        Some(header) => Ok(Some(header.session_id)),
        None => Err(warnings.into_iter().next()
            .unwrap_or_else(|| format!("无法确认 rollout {path:?} 的会话 ID"))),
    }
}

pub struct CodexSource;

pub(crate) fn archive_snapshot() -> Result<Vec<super::ArchiveChange>, String> {
    let root = sessions_root();
    let mut warnings = Vec::new();
    let paths = storage::rollouts(&root, true, &mut warnings, &|| false)?;
    let groups = history::group_pages(history::read_headers(&paths, &mut warnings, &|| false)?);
    Ok(groups.into_values().filter_map(|pages| {
        let page = pages.last()?;
        Some(super::ArchiveChange {
            session: super::search::SessionRef {
                session_id: page.header.session_id.clone(),
                project_dir: page.header.cwd.as_deref().map(escape_cwd).unwrap_or_else(|| "-unknown".into()),
            },
            archived: storage::is_archived(&root, &page.path),
        })
    }).collect())
}

impl AgentSource for CodexSource {
    fn id(&self) -> &'static str {
        "codex"
    }
    fn display_name(&self) -> &'static str {
        "Codex"
    }
    fn is_installed(&self) -> bool {
        let root = sessions_root();
        root.exists() || storage::archived_root(&root).exists()
    }
    fn data_dir(&self) -> Option<PathBuf> {
        sessions_root().parent().map(|p| p.to_path_buf())
    }
    fn watch_paths(&self) -> super::AgentWatchPaths {
        let mut paths = super::AgentWatchPaths::transcripts("sessions", None);
        paths.extra_transcript_suffixes.push(".jsonl.zst");
        paths.transcript_dirs.push(PathBuf::from(storage::ARCHIVED_SESSIONS));
        paths.root_files = vec!["session_index.jsonl", ".codex-global-state.json", thread::STATE_DB, thread::STATE_WAL];
        paths
    }
    fn watch_locations(&self) -> Vec<(PathBuf, super::AgentWatchPaths)> {
        let home = codex_home();
        let sqlite = sqlite_home(&home);
        let mut locations = vec![(home.clone(), self.watch_paths())];
        if sqlite != home {
            locations.push((sqlite, super::AgentWatchPaths {
                root_files: vec![thread::STATE_DB, thread::STATE_WAL],
                ..Default::default()
            }));
        }
        locations
    }
    fn scan_catalog(
        &self,
        filter: CatalogFilter,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<super::Catalog, String> {
        catalog::Scan::new(&sessions_root(), filter, cancelled)?.read(false, cancelled)
    }
    fn prepare_catalog(
        &self,
        filter: CatalogFilter,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(super::Catalog, Option<super::CatalogRefinement>), String> {
        let scan = catalog::Scan::new(&sessions_root(), filter, cancelled)?;
        let catalog = scan.read(true, cancelled)?;
        let refinement = if catalog.sessions.iter().any(|session| session.stats_pending) {
            let refine: super::CatalogRefinement =
                Box::new(move |cancelled| scan.read(false, cancelled));
            Some(refine)
        } else {
            None
        };
        Ok((catalog, refinement))
    }
    fn read_tool_payload(&self, token: &str) -> Option<Result<Value, String>> {
        deferred::read(token)
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
        let pages = history::read_headers(&meta.source_files, &mut warnings, &|| false)?;
        if !warnings.is_empty()
            || pages.len() != meta.source_files.len()
            || pages.iter().any(|p| p.header.session_id != meta.session_id)
        {
            return Err("会话来源已变化，请刷新后重试".into());
        }
        let (mut detail, ws) = history::read(&pages, ReadMode::Events)?;
        if !ws.is_empty() || detail.meta.project_dir != meta.project_dir {
            return Err("会话来源已变化，请刷新后重试".into());
        }
        Ok(detail.events.split_off(detail.inherited))
    }
    fn delete_policy(&self) -> DeletePolicy {
        DeletePolicy::Permanent
    }
    fn supports_archive(&self) -> bool {
        true
    }
    fn set_session_archived(
        &self,
        project_dir: &str,
        session_id: &str,
        archived: bool,
    ) -> Result<(), String> {
        let action = if archived { ThreadAction::Archive } else { ThreadAction::Unarchive };
        app_server::update(&codex_home(), action, project_dir, session_id)
    }
    fn delete_sessions(
        &self,
        sessions: &[super::search::SessionRef],
        preserve: &mut dyn PreserveUsage,
        outcome: &mut Outcome<'_>,
    ) -> Result<(), String> {
        deletion::delete(&codex_home(), sessions, preserve, outcome)
    }
    fn resume_command(&self, session_id: &str, _fork: bool) -> Option<String> {
        if !valid_thread_id(session_id) {
            return None;
        }
        let root = sessions_root();
        let mut warnings = Vec::new();
        let paths = storage::rollouts(&root, true, &mut warnings, &|| false).ok()?;
        let groups = history::group_pages(history::read_headers(&paths, &mut warnings, &|| false).ok()?);
        let pages = groups.get(session_id)?;
        let latest = pages.last()?;
        let relations = thread::Relations::new(&groups, thread::spawn_edges(root.parent()?).ok()?);
        if !latest.header.thread.meta.can_resume || relations.is_subagent(session_id)
            || storage::is_archived(&root, &latest.path) {
            return None;
        }
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
            r#"{"timestamp":"2026-09-03T01:00:00Z","type":"session_meta","payload":{"id":"abcdef01","cwd":"/tmp/demo","originator":"Codex Desktop","cli_version":"0.30.0","git":{"branch":"main"}}}"#,
            r#"{"timestamp":"2026-09-03T01:00:01Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"<environment_context>\ncli\n</environment_context>\n帮我写个函数"},{"type":"input_text","text":"<image name=fixture>"},{"type":"input_image","image_url":"data:image/png;base64,AAAA"},{"type":"input_text","text":"</image>"}]}}"#,
            r#"{"timestamp":"2026-09-03T01:00:01.100Z","type":"event_msg","payload":{"type":"thread_settings_applied","thread_id":"abcdef01","thread_settings":{"model":"fixture-model"}}}"#,
            r#"{"timestamp":"2026-09-03T01:00:02Z","type":"response_item","payload":{"type":"reasoning","summary":[{"type":"summary_text","text":"先分析需求"}],"content":[{"type":"reasoning_text","text":"核对正文"}]}}"#,
            r#"{"timestamp":"2026-09-03T01:00:03Z","type":"response_item","payload":{"type":"function_call","name":"shell","call_id":"c1","arguments":"{\"cmd\":[\"ls\"]}"}}"#,
            r#"{"timestamp":"2026-09-03T01:00:04Z","type":"response_item","payload":{"type":"function_call_output","call_id":"c1","output":"Chunk ID: abc\nWall time: 0.1 seconds\nProcess exited with code 2\nFinal output:\nfile.txt"}}"#,
            r#"{"timestamp":"2026-09-03T01:00:05Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"完成了"}]}}"#,
            r#"{"timestamp":"2026-09-03T01:00:06Z","type":"session_meta","payload":{"id":"parent","cwd":"/tmp/other"}}"#,
            r#"{"timestamp":"2026-09-03T01:00:07Z","type":"response_item","payload":{"type":"web_search_call","status":"completed","action":{"type":"search","query":"fixture"}}}"#,
            r#"{"timestamp":"2026-09-03T01:00:08Z","type":"compacted","payload":{"message":"压缩正文"}}"#,
            r#"{"timestamp":"2026-09-03T01:00:09Z","type":"event_msg","payload":{"type":"turn_aborted","reason":"interrupted"}}"#,
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
    fn codex_thread_source_retains_resume_and_parent_capabilities() {
        for (payload, source, subagent, can_resume) in [
            (json!({}), None, false, true),
            (json!({"source":"vscode","thread_source":"user"}), Some("vscode"), false, true),
            (json!({"source":"exec"}), Some("exec"), false, true),
            (json!({"source":"cli","originator":"codex_exec"}), Some("exec"), false, true),
            (json!({"source":"appServer"}), Some("appServer"), false, true),
            (json!({"source":"mcp"}), Some("mcp"), false, true),
            (json!({"source":"vscode","thread_source":"onboarding_checklist"}), Some("vscode"), false, false),
            (json!({"source":{"subagent":{"thread_spawn":{"parent_thread_id":"parent","depth":1}}}}), Some("subagent"), true, false),
            (json!({"thread_source":"subagent"}), None, true, false),
        ] {
            let header = ThreadHeader::read(&payload);
            assert_eq!(header.meta.source.as_deref(), source);
            assert_eq!(header.subagent, subagent);
            assert_eq!(header.meta.can_resume, can_resume);
        }
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
        write_rollout_fixture(
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
        assert_eq!(to_meta(&p, None).client.as_deref(), Some("Codex Desktop"));
        assert_eq!(p.real_user_count, 1);
        assert_eq!(p.git_branch.as_deref(), Some("main"));
        assert_eq!(p.model.as_deref(), Some("fixture-model"));
        assert_eq!(p.assistant_count, 4);
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
                "assistant",
                "assistant",
                "context",
                "system",
            ]
        );

        assert_eq!(p.events[1]["message"]["content"][0]["text"], "帮我写个函数");
        assert_eq!(p.events[1]["message"]["content"][1]["type"], "input_image");

        assert_eq!(p.events[2]["message"]["content"][0]["thinking"], "先分析需求\n\n核对正文");
        assert_eq!(p.events[2]["message"]["model"], "fixture-model");
        assert_eq!(p.events[6]["message"]["content"][0]["name"], "web_search");
        assert_eq!(p.events[6]["message"]["content"][0]["ops"][0]["queries"], json!(["fixture"]));
        assert_eq!(p.events[7]["message"]["content"], "压缩正文");
        assert_eq!(p.events[8]["content"], "轮次已中断");

        let tool_use = &p.events[3]["message"]["content"][0];
        assert_eq!(tool_use["type"], "tool_use");
        assert_eq!(tool_use["name"], "shell");
        assert_eq!(tool_use["input"]["cmd"][0], "ls");

        let tool_result = &p.events[4]["message"]["content"][0];
        assert_eq!(tool_result["type"], "tool_result");
        assert_eq!(tool_result["is_error"], true);
        assert!(tool_result["content"].as_str().unwrap().ends_with("file.txt"));
    }

    /// 集成测试：注入临时目录作为会话根，走完 catalog/read/delete 全链路。
    /// 不用环境变量、不读写真实 ~/.codex；tempdir 退出即清理，
    /// panic 也不会留下副作用；删除走注入的 remove，不碰真实废纸篓
    #[test]
    fn archive_filter_preserves_session_identity_and_history() {
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
            r#"{"timestamp":"2099-12-31T00:00:01Z","type":"response_item","payload":{"type":"message","id":"msg-task","role":"user","content":[{"type":"input_text","text":"集成测试消息"}]}}"#.to_string(),
            r#"{"timestamp":"2099-12-31T00:00:02Z","type":"response_item","payload":{"type":"message","id":"msg-reply","role":"assistant","content":[{"type":"output_text","text":"收到"}]}}"#.to_string(),
        ]
        .join("\n");
        let main_file = day_dir.join("rollout-2099-12-31T00-00-00-it-a.jsonl");
        fs::write(&main_file, &main_content).unwrap();

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

        // 分叉副本只保留精简后的父会话历史；详情改用父会话的完整记录。
        let sub_content = [
            format!(r#"{{"timestamp":"2099-12-31T00:00:00Z","ordinal":0,"type":"session_meta","payload":{{"id":"{id_sub}","cwd":"/tmp/codex-it","originator":"Codex Desktop","source":{{"subagent":{{"thread_spawn":{{"parent_thread_id":"{id_main}","depth":1}}}}}},"thread_source":"subagent","forked_from_id":"{id_main}","subagent_history_start_ordinal":3}}}}"#),
            format!(r#"{{"timestamp":"2099-12-31T00:00:00Z","ordinal":1,"type":"session_meta","payload":{{"id":"{id_main}","cwd":"/tmp/codex-it"}}}}"#),
            r#"{"timestamp":"2099-12-31T00:00:02Z","ordinal":2,"type":"response_item","payload":{"type":"message","id":"msg-reply","role":"assistant","content":[{"type":"output_text","text":"收到"}]}}"#.to_string(),
            r#"{"timestamp":"2099-12-31T00:00:03Z","ordinal":3,"type":"response_item","payload":{"type":"agent_message","id":"amsg-task","author":"/root","recipient":"/root/sub","content":[{"type":"input_text","text":"Message Type: NEW_TASK\nTask name: /root/sub\nSender: /root\nPayload:\n"},{"type":"encrypted_content","encrypted_content":"gAAAAABrAAXIAAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8gISIjJCUmJygpKissLS4vMDEyMzQ1Njc4OTo7PD0-Pw=="}]}}"#.to_string(),
            r#"{"timestamp":"2099-12-31T00:00:04Z","ordinal":4,"type":"response_item","payload":{"type":"function_call","name":"send_message","call_id":"call-report","arguments":"{\"target\":\"/root\",\"message\":\"gAAAAABrAAXIAAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8gISIjJCUmJygpKissLS4vMDEyMzQ1Njc4OTo7PD0-Pw==\"}"}}"#.to_string(),
            r#"{"timestamp":"2099-12-31T00:00:05Z","ordinal":5,"type":"response_item","payload":{"type":"agent_message","id":"amsg-done","author":"/root/sub/leaf","recipient":"/root/sub","content":[{"type":"input_text","text":"Message Type: FINAL_ANSWER\nTask name: /root/sub\nSender: /root/sub/leaf\nPayload:\n子智能体完成"}]}}"#.to_string(),
            format!(r#"{{"timestamp":"2099-12-31T00:00:06Z","ordinal":6,"type":"response_item","payload":{{"type":"function_call","name":"send_message","call_id":"call-plain","arguments":"{{\"target\":\"/root\",\"message\":\"gAAAAA{}==\"}}"}}}}"#, "B".repeat(92)),
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
        assert_eq!(ours.len(), 3);
        let main = ours.iter().find(|meta| meta.session_id == id_main).unwrap();
        assert_eq!(main.title.as_deref(), Some("集成测试消息"));
        let in_project = ours.iter().filter(|s| s.project_dir == project_id).count();
        assert_eq!(in_project, 3);

        let detail = read_session_in(&root, &id_main).unwrap();
        assert_eq!(detail.events.len(), 2);
        let sub = read_session_in(&root, &id_sub).unwrap();
        assert_eq!((sub.events.len(), sub.inherited), (6, 2));
        assert_eq!(sub.events[..2], detail.events[..]);
        assert_eq!(sub.events[2]["agent"], json!({"kind": "task", "from": "/root", "to": "/root/sub", "encrypted": true}));
        assert_eq!(sub.events[2]["message"]["content"], json!([]));
        assert_eq!(sub.events[3]["message"]["content"][0]["ops"][0]["encrypted"], true);
        assert_eq!(sub.events[4]["agent"]["kind"], "result");
        assert_eq!(sub.events[4]["message"]["content"][0]["text"], "子智能体完成");
        assert!(sub.events[5]["message"]["content"][0]["ops"][0].get("encrypted").is_none(), "非规范补位的明文不能当成密文");
        let sub_meta = ours.iter().find(|meta| meta.session_id == id_sub).unwrap();
        assert_eq!(CodexSource.search_events(sub_meta).unwrap(), sub.events[2..]);
        assert!(!sub.meta.codex.unwrap().can_resume);
        assert_eq!(sub.meta.subagent.unwrap().parent_id.as_deref(), Some(id_main.as_str()));

        let archived = storage::archived_root(&root);
        fs::create_dir(&archived).unwrap();
        fs::rename(&main_file, archived.join(main_file.file_name().unwrap())).unwrap();
        let active = catalog_in(&root, false).unwrap();
        assert_eq!(active.sessions.len(), 2);
        assert!(!active.sessions.iter().find(|s| s.session_id == id_sub).unwrap().subagent.as_ref().unwrap().orphan);

        let filter = CatalogFilter { include_archived: true, ..Default::default() };
        let scan = catalog::Scan::new(&root, filter, &|| false).unwrap();
        for quick in [true, false] {
            let catalog = scan.read(quick, &|| false).unwrap();
            assert_eq!(catalog.sessions.len(), 3);
            let archived = catalog.sessions.iter().find(|s| s.session_id == id_main).unwrap();
            assert!(archived.archived);
            assert_eq!(catalog.projects[0].session_count, 3);
            assert_eq!(CodexSource.search_events(archived).unwrap(), detail.events);
        }
        let archived_detail = read_session_in(&root, &id_main).unwrap();
        assert!(archived_detail.meta.archived);
        assert_eq!(archived_detail.events, detail.events);
    }

    fn write_rollout_fixture(root: &Path, relative_path: &str, id: &str, cwd: &str) -> PathBuf {
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
    fn real_data_smoke_if_present() {
        let mut warnings = Vec::new();
        let root = sessions_root();
        let files = storage::rollouts(&root, false, &mut warnings, &|| false).unwrap();
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
                panic!("真实 rollout 应可解析：{f:?}");
            };
            assert!(!p.session_id.is_empty(), "session_id 为空：{f:?}");
            let meta = to_meta(&p, thread_names.get(&p.session_id));
            assert!(meta.title.is_none() || !meta.title.as_deref().unwrap().is_empty());
        }
    }
}
