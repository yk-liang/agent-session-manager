//! Pi 数据源：$PI_CODING_AGENT_DIR（默认 ~/.pi/agent）/sessions/--<cwd>--/<started>_<id>.jsonl
//!
//! 格式要点（参考 agent-session-cleaner）：
//! - 首行是 header：{type:"session", version, id, timestamp, cwd}
//! - 其余行是树形 entry（version>=2）：/fork 会留下废弃分支，
//!   真正的对话 = 从最后一行（叶节点）沿 parentId 回溯
//! - version 1 是线性顺序
//! - 标题来自 session_info entry 的 name（最后一个生效），回退首条用户消息
//! - 无官方删除命令，直接移文件进回收站（校验必须在本目录树内）

mod context;
#[cfg(test)]
mod context_tests;

use serde_json::Value;
use std::collections::HashMap;
#[cfg(test)]
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use super::{ms_to_iso, AgentSource, ConfigFile, SessionDetail, SessionMeta};

fn pi_home() -> PathBuf {
    if let Ok(v) = std::env::var("PI_CODING_AGENT_DIR") {
        if !v.is_empty() {
            return super::expand_tilde(&v);
        }
    }
    dirs::home_dir()
        .unwrap_or_default()
        .join(".pi")
        .join("agent")
}

fn sessions_dir() -> PathBuf {
    pi_home().join("sessions")
}

fn is_valid_session_id(id: &str) -> bool {
    let ok = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-');
    !id.is_empty()
        && id.len() <= 128
        && id.chars().all(ok)
        && id
            .chars()
            .next()
            .map(|c| c.is_ascii_alphanumeric())
            .unwrap_or(false)
        && id
            .chars()
            .next_back()
            .map(|c| c.is_ascii_alphanumeric())
            .unwrap_or(false)
}

struct ParsedPi {
    id: String,
    /// 会话文件所在的物理项目目录名；读/删操作的权威定位键
    project_dir: String,
    cwd: Option<String>,
    /// header 时间戳（RFC3339）
    created: Option<String>,
    /// session_info 的 name（最后一个生效）
    name: Option<String>,
    first_user_text: Option<String>,
    /// 活跃分支上的对话（统一事件结构）
    events: Vec<Value>,
    real_user_count: u32,
    assistant_text_count: u32,
    size_bytes: u64,
    mtime_ms: i64,
}

/// 从 message.content 提取人说的话：字符串直接用；块列表只取 text 块
fn spoken_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(parts) => {
            let mut texts = Vec::new();
            for p in parts {
                if p.get("type").and_then(Value::as_str) == Some("text") {
                    if let Some(t) = p.get("text").and_then(Value::as_str) {
                        if !t.trim().is_empty() {
                            texts.push(t.to_string());
                        }
                    }
                }
            }
            texts.join("\n")
        }
        _ => String::new(),
    }
}

fn parse_pi_session(path: &Path, collect_events: bool) -> super::ParseOutcome<ParsedPi> {
    let file = match super::open_jsonl(path) {
        Ok(f) => f,
        Err(e) => {
            return super::ParseOutcome::Failed(format!("无法读取 Pi 会话文件 {:?}: {e}", path))
        }
    };
    let size_bytes = file.metadata().map(|m| m.len()).unwrap_or(0);
    let mtime_ms = file
        .metadata()
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);

    let reader = BufReader::new(file);
    let mut lines = reader.lines();

    // 首行 header
    let mut parsed = ParsedPi {
        id: String::new(),
        project_dir: path
            .parent()
            .and_then(Path::file_name)
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default(),
        cwd: None,
        created: None,
        name: None,
        first_user_text: None,
        events: Vec::new(),
        real_user_count: 0,
        assistant_text_count: 0,
        size_bytes,
        mtime_ms,
    };
    let header_line = match lines.next() {
        Some(Ok(l)) => l,
        Some(Err(e)) => {
            return super::ParseOutcome::Failed(format!("读取 {:?} 首行失败: {e}", path))
        }
        None => return super::ParseOutcome::Failed(format!("Pi 会话文件为空: {:?}", path)),
    };
    let header: Value = match serde_json::from_str(header_line.trim()) {
        Ok(v) => v,
        Err(_) => {
            return super::ParseOutcome::Failed(format!("Pi 会话 header 不是合法 JSON: {:?}", path))
        }
    };
    if header.get("type").and_then(Value::as_str) != Some("session") {
        return super::ParseOutcome::Failed(format!("Pi 会话 header 类型异常: {:?}", path));
    }
    parsed.id = header
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    parsed.cwd = header.get("cwd").and_then(Value::as_str).map(String::from);
    parsed.created = header
        .get("timestamp")
        .and_then(Value::as_str)
        .map(String::from);
    let version = header.get("version").and_then(Value::as_i64).unwrap_or(1);

    // All records participate in ancestry; only the active branch supplies dialogue/context.
    let mut parents: HashMap<String, String> = HashMap::new();
    let mut spoken: HashMap<String, Vec<Value>> = HashMap::new();
    let mut leaf = String::new();
    let mut linear: Vec<Value> = Vec::new();
    let bad_lines = super::for_each_jsonl_line(lines, |v| {
        let id = v["id"].as_str().unwrap_or("").to_string();
        if version >= 2 && !id.is_empty() {
            parents.insert(id.clone(), v["parentId"].as_str().unwrap_or("").to_string());
            leaf = id.clone();
            spoken.remove(&id);
        }
        if v["type"] == "session_info" {
            if let Some(name) = v["name"].as_str() {
                parsed.name = (!name.trim().is_empty()).then(|| name.trim().to_string());
            }
        }
        let events = context::entry(&v, parsed.cwd.as_deref(), collect_events);
        if version < 2 {
            linear.extend(events);
        } else if !id.is_empty() && !events.is_empty() {
            spoken.insert(id, events);
        }
    });
    let chain = if version < 2 {
        linear
    } else {
        let mut chain = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut id = leaf;
        while !id.is_empty() && seen.insert(id.clone()) {
            if let Some(events) = spoken.remove(&id) {
                chain.push(events);
            }
            id = parents.get(&id).cloned().unwrap_or_default();
        }
        chain.into_iter().rev().flatten().collect()
    };
    for event in chain {
        if event["type"] == "user" {
            parsed.real_user_count += 1;
            if parsed.first_user_text.is_none() {
                parsed.first_user_text = Some(
                    event["message"]["content"]
                        .as_str()
                        .unwrap_or("")
                        .chars()
                        .take(80)
                        .collect(),
                );
            }
        } else if event["type"] == "assistant" {
            parsed.assistant_text_count += 1;
        }
        if collect_events {
            parsed.events.push(event);
        }
    }

    if parsed.id.is_empty() {
        // 旧 header 可能没有 id：用文件名 <started>_<id>.jsonl 的尾段兜底
        if let Some(stem) = path.file_stem().map(|s| s.to_string_lossy().to_string()) {
            parsed.id = stem
                .rsplit_once('_')
                .map(|(_, id)| id.to_string())
                .unwrap_or_else(|| stem.to_string());
        }
    }
    if parsed.id.is_empty() {
        return super::ParseOutcome::Failed(format!("无法解析 Pi 会话 ID: {:?}", path));
    }
    if bad_lines > 0 {
        return super::ParseOutcome::Degraded(parsed, super::degraded_message(path, bad_lines));
    }
    super::ParseOutcome::Ok(parsed)
}

fn all_session_files_in(root: &Path, warnings: &mut Vec<String>) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for project in super::read_dir_warned(root, warnings) {
        for entry in super::read_dir_warned(&project.path(), warnings) {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.ends_with(".jsonl") {
                files.push(entry.path());
            }
        }
    }
    files
}

fn all_session_files(warnings: &mut Vec<String>) -> Vec<PathBuf> {
    all_session_files_in(&sessions_dir(), warnings)
}

struct SessionMatches {
    sessions: Vec<(PathBuf, ParsedPi)>,
    warnings: Vec<String>,
}

impl SessionMatches {
    /// 删除必须确认整个项目已完整扫描，无法解析的文件也可能包含同一个 ID。
    fn require_complete(self, project_dir: &str) -> Result<Vec<(PathBuf, ParsedPi)>, String> {
        if let Some(warning) = self.warnings.first() {
            return Err(format!(
                "无法完整扫描 Pi 项目 {project_dir}，为避免会话身份歧义已中止：{warning}"
            ));
        }
        Ok(self.sessions)
    }
}

fn matching_sessions(
    root: &Path,
    project_dir: &str,
    session_id: &str,
    collect_events: bool,
) -> Result<SessionMatches, String> {
    if project_dir.is_empty()
        || project_dir == "."
        || project_dir == ".."
        || project_dir.contains(['/', '\\'])
    {
        return Err("非法项目目录名".to_string());
    }
    let project = root.join(project_dir);
    let mut warnings = Vec::new();
    let mut matches = Vec::new();
    for entry in super::read_dir_warned(&project, &mut warnings) {
        let path = entry.path();
        if !entry.file_name().to_string_lossy().ends_with(".jsonl") {
            continue;
        }
        match parse_pi_session(&path, collect_events) {
            super::ParseOutcome::Ok(parsed) => {
                if parsed.id == session_id && parsed.project_dir == project_dir {
                    matches.push((path, parsed));
                }
            }
            super::ParseOutcome::Degraded(parsed, warning) => {
                warnings.push(warning);
                if parsed.id == session_id && parsed.project_dir == project_dir {
                    matches.push((path, parsed));
                }
            }
            super::ParseOutcome::Failed(warning) => warnings.push(warning),
        }
    }
    Ok(SessionMatches {
        sessions: matches,
        warnings,
    })
}

/// 破坏性路径专用的身份验证：只信任文件头内嵌的 id，
/// 绝不做文件名兜底——否则缺少内嵌 id 的文件改个名就能匹配删除请求。
/// 同时逐行扫描后续内容：任何读取失败或无法解析的行都说明文件
/// 部分损坏，拒绝删除——破坏性路径要求完整解析成功，
/// 不能只凭 header 身份正确就放行
fn strict_session_id(path: &Path) -> Result<String, String> {
    let file = super::open_jsonl(path).map_err(|e| format!("无法读取 {:?}: {e}", path))?;
    let mut lines = BufReader::new(file).lines();
    let header_line = match lines.next() {
        Some(Ok(l)) => l,
        Some(Err(e)) => return Err(format!("读取 {:?} 首行失败: {e}", path)),
        None => return Err(format!("Pi 会话文件为空: {:?}", path)),
    };
    let header: Value = match serde_json::from_str(header_line.trim()) {
        Ok(v) => v,
        Err(_) => return Err(format!("Pi 会话 header 不是合法 JSON: {:?}", path)),
    };
    if header.get("type").and_then(Value::as_str) != Some("session") {
        return Err(format!("Pi 会话 header 类型异常: {:?}", path));
    }
    let id = header
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if id.is_empty() {
        return Err(format!(
            "{:?} 的 header 缺少内嵌 id，拒绝按文件名推导执行删除",
            path
        ));
    }
    // 全量复核：任一坏行/读取失败即视为部分损坏，中止删除
    for line in lines {
        let line = line.map_err(|e| format!("读取 {:?} 失败，无法完成身份复核: {e}", path))?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if serde_json::from_str::<Value>(line).is_err() {
            return Err(format!(
                "{:?} 含无法解析的行，无法完成身份复核，已中止",
                path
            ));
        }
    }
    Ok(id)
}

/// 生产环境的移除方式：移入系统回收站
fn trash_remove(target: PathBuf) -> Result<(), String> {
    trash::delete_all([target]).map_err(|e| format!("移入回收站失败: {e}"))
}

/// 删除实现（root 与移除方式均可注入，便于测试）。
/// Pi 没有删除命令；流程：
/// 解析匹配（项目 + 会话 ID 都要一致）→ 严格身份复核（只认 header 内嵌 id）
/// → 根目录/类型校验 → 移除。project_dir 参与匹配，避免不同项目下
/// 同名 ID 的会话被误删。
fn delete_session_with(
    root: &Path,
    project_dir: &str,
    session_id: &str,
    remove: impl Fn(PathBuf) -> Result<(), String>,
) -> Result<(), String> {
    if !is_valid_session_id(session_id) {
        return Err("非法会话 ID".into());
    }
    let mut matches =
        matching_sessions(root, project_dir, session_id, false)?.require_complete(project_dir)?;
    if matches.len() > 1 {
        return Err(format!(
            "项目 {project_dir} 中找到多个 ID 为 {session_id} 的 Pi 会话，无法安全删除"
        ));
    }
    let Some((path, _)) = matches.pop() else {
        return Err("未找到会话文件".to_string());
    };
    // 解析用的文件名兜底 id 不得作为删除依据
    let strict = strict_session_id(&path)?;
    if strict != session_id {
        return Err(format!("{:?} 内嵌 id 与待删会话不一致，已中止", path));
    }
    let target = super::verify_within_root(root, &path, super::TargetKind::File)?;
    let mut confirmed =
        matching_sessions(root, project_dir, session_id, false)?.require_complete(project_dir)?;
    if confirmed.len() != 1 {
        return Err(format!(
            "项目 {project_dir} 中 ID 为 {session_id} 的 Pi 会话在删除前发生变化，已中止"
        ));
    }
    let (confirmed_path, _) = confirmed.pop().unwrap();
    let confirmed_target =
        super::verify_within_root(root, &confirmed_path, super::TargetKind::File)?;
    if confirmed_target != target || strict_session_id(&confirmed_path)? != session_id {
        return Err("Pi 会话身份在删除前发生变化，已中止".to_string());
    }
    remove(target)
}

fn delete_session_in(root: &Path, project_dir: &str, session_id: &str) -> Result<(), String> {
    delete_session_with(root, project_dir, session_id, trash_remove)
}

/// 读取实现（root 可注入，便于测试）。会话 ID 与项目都要一致，
/// 跨项目同名 ID 不会读到错误的会话。
fn read_session_in(
    root: &Path,
    project_dir: &str,
    session_id: &str,
) -> Result<SessionDetail, String> {
    if !is_valid_session_id(session_id) {
        return Err("非法会话 ID".into());
    }
    // 只读详情沿用目录的容错策略，允许浏览已解析的正文；扫描告警由目录展示。
    // Degraded 仍参与 ID 匹配，因此已识别出的重复会话必须继续拒绝读取。
    let mut matches = matching_sessions(root, project_dir, session_id, true)?.sessions;
    if matches.len() > 1 {
        return Err(format!(
            "项目 {project_dir} 中找到多个 ID 为 {session_id} 的 Pi 会话，无法唯一读取"
        ));
    }
    let Some((_, parsed)) = matches.pop() else {
        return Err("未找到该 Pi 会话".to_string());
    };
    Ok(SessionDetail {
        warnings: Vec::new(),
        meta: to_meta(&parsed),
        events: parsed.events,
    })
}

fn to_meta(p: &ParsedPi) -> SessionMeta {
    SessionMeta {
        session_id: p.id.clone(),
        project_dir: p.project_dir.clone(),
        cwd: p.cwd.clone(),
        title: p.name.clone().or_else(|| p.first_user_text.clone()),
        model: None,
        model_name: None,
        version: None,
        git_branch: None,
        entrypoint: None,
        first_ts: p.created.clone(),
        last_ts: ms_to_iso(p.mtime_ms).or_else(|| p.created.clone()),
        user_count: p.real_user_count,
        assistant_count: p.assistant_text_count,
        real_user_count: p.real_user_count,
        assistant_text_count: p.assistant_text_count,
        size_bytes: p.size_bytes,
        has_transcript: true,
        stats_pending: false,
        source_files: Vec::new(),
    }
}

pub struct PiSource;

impl AgentSource for PiSource {
    fn id(&self) -> &'static str {
        "pi"
    }
    fn display_name(&self) -> &'static str {
        "Pi"
    }
    fn is_installed(&self) -> bool {
        sessions_dir().exists()
    }
    fn data_dir(&self) -> Option<PathBuf> {
        Some(pi_home())
    }
    fn watch_paths(&self, _usage: bool) -> super::AgentWatchPaths {
        super::AgentWatchPaths::transcripts("sessions", None)
    }
    fn catalog(&self, include_empty: bool) -> Result<super::Catalog, String> {
        let mut warnings = Vec::new();
        let mut items = Vec::new();
        for path in all_session_files(&mut warnings) {
            // 列表阶段用轻量解析（不保留事件正文）
            let result = super::catalog_cache::summary("pi-context-v2", &[path.clone()], || {
                let (parsed, warnings) = parse_pi_session(&path, false).split();
                let p = parsed.ok_or_else(|| warnings.join("; "))?;
                let mut meta = to_meta(&p);
                meta.source_files.push(path.clone());
                Ok((meta, warnings))
            });
            let (meta, ws) = match result {
                Ok(result) => result,
                Err(error) => {
                    warnings.push(error);
                    continue;
                }
            };
            warnings.extend(ws);
            if !include_empty && meta.real_user_count == 0 {
                continue;
            }
            let display = meta
                .cwd
                .clone()
                .filter(|d| !d.is_empty())
                .unwrap_or_else(|| "(未知目录)".to_string());
            items.push((meta, display));
        }
        let mut catalog = super::assemble_catalog(items);
        catalog.warnings = warnings;
        Ok(catalog)
    }
    fn read_session(&self, project_dir: &str, session_id: &str) -> Result<SessionDetail, String> {
        read_session_in(&sessions_dir(), project_dir, session_id)
    }
    fn search_events(&self, meta: &SessionMeta) -> Result<Vec<Value>, String> {
        let path = meta.source_files.first().ok_or("缺少会话文件定位信息")?;
        let (parsed, _) = parse_pi_session(path, true).split();
        let p = parsed.ok_or("无法解析该 Pi 会话")?;
        if p.id != meta.session_id || p.project_dir != meta.project_dir {
            return Err("会话来源已变化，请刷新后重试".into());
        }
        Ok(p.events)
    }
    fn delete_session(&self, project_dir: &str, session_id: &str) -> Result<(), String> {
        delete_session_in(&sessions_dir(), project_dir, session_id)
    }
    fn resume_command(&self, session_id: &str, _fork: bool) -> Option<String> {
        Some(format!(
            "{} --session-id {}",
            super::shell_quote("pi"),
            super::shell_quote(session_id)
        ))
    }
    fn config_spec(&self) -> Vec<ConfigFile> {
        vec![ConfigFile {
            key: "settings.json".into(),
            label: "settings.json".into(),
            path: pi_home().join("settings.json"),
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session_of(path: &Path) -> ParsedPi {
        let (parsed, _) = parse_pi_session(path, true).split();
        match parsed {
            Some(p) => p,
            None => panic!("解析失败"),
        }
    }

    #[test]
    fn session_id_validation() {
        assert!(is_valid_session_id("abc-123_def.456"));
        assert!(!is_valid_session_id(""));
        assert!(!is_valid_session_id("a/b"));
        assert!(!is_valid_session_id("-abc"));
        assert!(!is_valid_session_id("abc-"));
    }

    #[test]
    fn config_spec_uses_official_settings_file() {
        let spec = PiSource.config_spec();
        assert_eq!(spec.len(), 1);
        assert_eq!(spec[0].key, "settings.json");
        assert_eq!(spec[0].path, pi_home().join("settings.json"));
    }

    #[test]
    fn search_uses_the_same_active_branch_and_event_order_as_detail() {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("proj");
        fs::create_dir_all(&project).unwrap();
        let path = project.join("session-good123.jsonl");
        fs::write(&path, [
            r#"{"type":"session","id":"good123","version":2,"cwd":"/tmp/demo"}"#,
            r#"{"type":"message","id":"u","message":{"role":"user","content":"question"}}"#,
            r#"{"type":"message","id":"discarded","parentId":"u","message":{"role":"assistant","content":"discarded branch"}}"#,
            r#"{"type":"message","id":"active","parentId":"u","message":{"role":"assistant","content":"active branch"}}"#,
        ].join("\n")).unwrap();
        let detail = read_session_in(root.path(), "proj", "good123").unwrap();
        let mut meta = detail.meta;
        meta.source_files.push(path);
        let events = PiSource.search_events(&meta).unwrap();
        assert_eq!(events, detail.events);
        assert!(!events
            .iter()
            .any(|event| event.to_string().contains("discarded branch")));
    }

    #[test]
    fn delete_refuses_filename_derived_id() {
        let tmp = tempfile::tempdir().unwrap();
        let proj = tmp.path().join("proj");
        fs::create_dir_all(&proj).unwrap();
        // header 无 id：解析会按文件名兜底出 "fallback123"，
        // 但删除绝不能信任这个推导结果
        let file = proj.join("1700000000_fallback123.jsonl");
        fs::write(
            &file,
            [
                r#"{"type":"session","timestamp":"2026-01-01T00:00:00Z","version":1}"#,
                r#"{"type":"message","message":{"role":"user","content":"hello"}}"#,
            ]
            .join("\n"),
        )
        .unwrap();

        let err = delete_session_in(tmp.path(), "proj", "fallback123").unwrap_err();
        assert!(err.contains("内嵌 id"), "{err}");
        assert!(file.exists(), "不得基于文件名推导执行删除");
    }

    #[test]
    fn delete_allows_header_embedded_id() {
        let tmp = tempfile::tempdir().unwrap();
        let proj = tmp.path().join("proj");
        fs::create_dir_all(&proj).unwrap();
        let file = proj.join("1700000000_good123.jsonl");
        fs::write(
            &file,
            [
                r#"{"type":"session","id":"good123","timestamp":"2026-01-01T00:00:00Z","version":1}"#,
                r#"{"type":"message","message":{"role":"user","content":"hello"}}"#,
            ]
            .join("\n"),
        )
        .unwrap();

        delete_session_with(tmp.path(), "proj", "good123", |p| {
            fs::remove_file(p).map_err(|e| e.to_string())
        })
        .unwrap();
        assert!(!file.exists(), "内嵌 id 的会话应可正常删除");
    }

    #[test]
    fn read_and_delete_are_scoped_by_project() {
        // 两个项目下各有一个 id 相同（same-id）的会话：读取/删除必须按项目定位，
        // 绝不能命中另一个项目里的同名会话
        let tmp = tempfile::tempdir().unwrap();
        let mk = |proj: &str, body: &str| {
            let dir = tmp.path().join(proj);
            fs::create_dir_all(&dir).unwrap();
            let file = dir.join("1700000000_dup.jsonl");
            fs::write(
                &file,
                [
                    r#"{"type":"session","id":"dup","version":2}"#.to_string(),
                    format!(
                        r#"{{"type":"message","id":"m1","parentId":"","message":{{"role":"user","content":"{body}"}}}}"#
                    ),
                ]
                .join("\n"),
            )
            .unwrap();
            file
        };
        let file_a = mk("projA", "项目A的问题");
        let file_b = mk("projB", "项目B的问题");
        let proj_a = "projA";
        let proj_b = "projB";

        // 读取按项目定位：各自拿到自己项目的内容
        let da = read_session_in(tmp.path(), &proj_a, "dup").unwrap();
        assert_eq!(da.events[0]["message"]["content"], "项目A的问题");
        let db = read_session_in(tmp.path(), &proj_b, "dup").unwrap();
        assert_eq!(db.events[0]["message"]["content"], "项目B的问题");

        // 删除按项目定位：删 A 只动 A，B 完好
        delete_session_with(tmp.path(), &proj_a, "dup", |p| {
            fs::remove_file(p).map_err(|e| e.to_string())
        })
        .unwrap();
        assert!(!file_a.exists(), "应删除项目 A 的会话");
        assert!(file_b.exists(), "不得误删项目 B 的同名会话");
    }

    #[test]
    fn read_healthy_session_with_unrelated_corrupt_file() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("proj");
        fs::create_dir(&project).unwrap();
        fs::write(
            project.join("one_safe.jsonl"),
            [
                r#"{"type":"session","id":"safe","version":1}"#,
                r#"{"type":"message","message":{"role":"user","content":"完整会话"}}"#,
            ]
            .join("\n"),
        )
        .unwrap();
        fs::write(
            project.join("other_corrupt.jsonl"),
            [
                r#"{"type":"session","id":"other","version":1}"#,
                "{truncated",
            ]
            .join("\n"),
        )
        .unwrap();

        let detail = read_session_in(tmp.path(), "proj", "safe").unwrap();
        assert_eq!(detail.events[0]["message"]["content"], "完整会话");
        let removed = std::cell::Cell::new(false);
        let error = delete_session_with(tmp.path(), "proj", "safe", |_| {
            removed.set(true);
            Ok(())
        })
        .unwrap_err();
        assert!(error.contains("无法完整扫描"), "{error}");
        assert!(!removed.get(), "只读容错不能放宽删除校验");
    }

    #[test]
    fn read_degraded_session_keeps_valid_messages() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("proj");
        fs::create_dir(&project).unwrap();
        fs::write(
            project.join("one_partial.jsonl"),
            [
                r#"{"type":"session","id":"partial","version":1}"#,
                r#"{"type":"message","message":{"role":"user","content":"保留的问题"}}"#,
                r#"{"type":"message","message":{"role":"assistant","content":"保留的回答"}}"#,
                "{truncated",
            ]
            .join("\n"),
        )
        .unwrap();

        let detail = read_session_in(tmp.path(), "proj", "partial").unwrap();
        assert_eq!(detail.events.len(), 2);
        assert_eq!(detail.events[0]["message"]["content"], "保留的问题");
        assert_eq!(detail.events[1]["message"]["content"], "保留的回答");
    }

    #[test]
    fn read_refuses_duplicate_ids_even_when_one_is_degraded() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("proj");
        fs::create_dir(&project).unwrap();
        let content = [
            r#"{"type":"session","id":"dup","version":1}"#,
            r#"{"type":"message","message":{"role":"user","content":"同 ID 会话"}}"#,
        ]
        .join("\n");
        fs::write(project.join("one_dup.jsonl"), &content).unwrap();
        fs::write(
            project.join("two_dup.jsonl"),
            format!("{content}\n{{truncated"),
        )
        .unwrap();

        let error = read_session_in(tmp.path(), "proj", "dup").err().unwrap();
        assert!(
            error.contains("多个"),
            "损坏副本也必须参与歧义检查: {error}"
        );
    }

    #[test]
    fn duplicate_ids_within_project_are_rejected_for_read_and_delete() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("proj");
        fs::create_dir_all(&project).unwrap();
        for (name, body) in [("one_dup.jsonl", "第一份"), ("two_dup.jsonl", "第二份")] {
            fs::write(
                project.join(name),
                [
                    r#"{"type":"session","id":"dup","version":1}"#.to_string(),
                    format!(
                        r#"{{"type":"message","message":{{"role":"user","content":"{body}"}}}}"#
                    ),
                ]
                .join("\n"),
            )
            .unwrap();
        }

        let read_error = read_session_in(tmp.path(), "proj", "dup").err();
        let removed = std::cell::Cell::new(false);
        let delete_error = delete_session_with(tmp.path(), "proj", "dup", |_| {
            removed.set(true);
            Ok(())
        })
        .err();

        assert!(
            read_error
                .as_deref()
                .is_some_and(|error| error.contains("多个")),
            "读取必须拒绝歧义: {read_error:?}"
        );
        assert!(
            delete_error
                .as_deref()
                .is_some_and(|error| error.contains("多个")),
            "删除必须拒绝歧义: {delete_error:?}"
        );
        assert!(!removed.get(), "歧义时不得执行删除");
    }

    #[test]
    fn delete_fails_closed_when_project_scan_is_incomplete() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("proj");
        fs::create_dir_all(&project).unwrap();
        fs::write(
            project.join("one_safe.jsonl"),
            [
                r#"{"type":"session","id":"safe","version":1}"#,
                r#"{"type":"message","message":{"role":"user","content":"hello"}}"#,
            ]
            .join("\n"),
        )
        .unwrap();
        let fifo = project.join("unknown.jsonl");
        let fifo_c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo_c.as_ptr(), 0o600) }, 0);
        let removed = std::cell::Cell::new(false);

        let detail = read_session_in(tmp.path(), "proj", "safe").unwrap();
        assert_eq!(detail.events[0]["message"]["content"], "hello");
        let err = delete_session_with(tmp.path(), "proj", "safe", |_| {
            removed.set(true);
            Ok(())
        })
        .unwrap_err();
        assert!(err.contains("无法完整扫描"), "{err}");
        assert!(!removed.get(), "扫描不完整时不得删除任何文件");
    }

    #[test]
    fn delete_refuses_partially_corrupt_session() {
        let tmp = tempfile::tempdir().unwrap();
        let proj = tmp.path().join("proj");
        fs::create_dir_all(&proj).unwrap();
        // header 身份正确，但后续含损坏行：宽松解析会判 Degraded 并匹配，
        // 严格验证器必须逐行扫描后拒绝，不能只凭 header 放行
        let file = proj.join("1700000000_corrupt1.jsonl");
        fs::write(
            &file,
            [
                r#"{"type":"session","id":"corrupt1","timestamp":"2026-01-01T00:00:00Z","version":1}"#,
                r#"{"type":"message","message":{"role":"user","content":"hello"}}"#,
                "这一行不是合法 JSON",
            ]
            .join("\n"),
        )
        .unwrap();

        let err = delete_session_in(tmp.path(), "proj", "corrupt1").unwrap_err();
        assert!(err.contains("无法解析"), "{err}");
        assert!(file.exists(), "部分损坏的会话不得删除");
    }

    #[test]
    fn parses_v2_tree_and_follows_leaf_branch() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        use std::io::Write;
        let lines = [
            r#"{"type":"session","version":2,"id":"pi-001","timestamp":"2026-09-03T10:00:00Z","cwd":"/tmp/pi-proj"}"#,
            // 旧分支（被 /fork 废弃）
            r#"{"type":"message","id":"e1","parentId":"","message":{"role":"user","content":"旧问题"}}"#,
            r#"{"type":"message","id":"e2","parentId":"e1","message":{"role":"assistant","content":[{"type":"text","text":"旧回答"}]}}"#,
            // 活跃分支
            r#"{"type":"message","id":"e3","parentId":"e1","message":{"role":"user","content":"新问题"}}"#,
            r#"{"type":"session_info","id":"e4","parentId":"e3","name":"我的命名"}"#,
            r#"{"type":"message","id":"e5","parentId":"e4","message":{"role":"assistant","content":[{"type":"text","text":"新回答"},{"type":"thinking","thinking":"思考"}]}}"#,
        ];
        for line in lines {
            writeln!(f, "{line}").unwrap();
        }

        let p = session_of(f.path());
        assert_eq!(p.id, "pi-001");
        assert_eq!(p.cwd.as_deref(), Some("/tmp/pi-proj"));
        assert_eq!(p.name.as_deref(), Some("我的命名"));
        // 活跃分支：e1(旧问题) → e3(新问题) → e5(新回答)，旧回答不在叶链上
        assert_eq!(p.real_user_count, 2);
        assert_eq!(p.assistant_text_count, 1);
        assert_eq!(p.events.len(), 3);
        assert_eq!(p.events[2]["message"]["content"], "新回答");
        assert_eq!(p.first_user_text.as_deref(), Some("旧问题"));
    }

    #[test]
    fn parses_v1_linear() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        use std::io::Write;
        let lines = [
            r#"{"type":"session","version":1,"id":"pi-old","timestamp":"2026-09-01T08:00:00Z","cwd":"/tmp/old"}"#,
            r#"{"type":"message","message":{"role":"user","content":"线性消息"}}"#,
            r#"{"type":"message","message":{"role":"assistant","content":"线性回复"}}"#,
        ];
        for line in lines {
            writeln!(f, "{line}").unwrap();
        }
        let p = session_of(f.path());
        assert_eq!(p.events.len(), 2);
        assert_eq!(p.real_user_count, 1);
        assert!(p.name.is_none());
        assert_eq!(p.first_user_text.as_deref(), Some("线性消息"));
    }
}
