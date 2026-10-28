//! Claude 系 CLI（QoderCLI / Claude Code）共享的 JSONL 会话解析。
//! 两者的会话都存储在 `<base>/<路径转义目录>/<session-uuid>.jsonl`，
//! 每行一个事件，消息通过 uuid/parentUuid 构成树形链。

mod context;
#[cfg(test)]
mod context_tests;

use serde_json::Value;
#[cfg(test)]
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use super::{SessionDetail, SessionMeta};

#[derive(Clone, Copy)]
pub struct FamilyOptions {
    /// 是否有 Qoder 特有的标题事件（ai-title / custom-title / last-prompt）
    pub title_events: bool,
    /// 过滤 entrypoint 非 cli 的会话（QoderWork/云端）
    pub filter_non_cli_entrypoint: bool,
    /// 过滤 cwd 为 qoderwork 工作区的会话
    pub filter_qoderwork_cwd: bool,
}

pub fn is_safe_name(name: &str) -> bool {
    !name.is_empty() && !name.contains('/') && !name.contains('\\') && name != "." && name != ".."
}

pub fn is_session_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

/// 项目目录名是路径转义（'/' → '-'），反转义仅作展示兜底；
/// 权威路径来自 JSONL 内的 cwd 字段。
pub fn unescape_dir_name(name: &str) -> String {
    name.replace('-', "/")
}

/// 从用户消息中提取有意义的文本（用于标题回退和 real_user_count 统计）。
/// 斜杠命令消息的真实用户输入在 `<command-args>` 内，其余标签
/// （`<command-message>` / `<command-name>` / `<environment_context>` 等）是
/// CLI 注入的噪音；剥离噪音后再判空，避免把真实问题当作空消息。
pub fn extract_user_text(msg: &Value) -> Option<String> {
    let content = msg.get("content")?;
    let texts: Vec<&str> = match content {
        Value::String(text) => vec![text],
        Value::Array(parts) => parts
            .iter()
            .filter(|p| p["type"] == "text")
            .filter_map(|p| p["text"].as_str())
            .collect(),
        _ => return None,
    };
    let cleaned = texts
        .into_iter()
        .map(strip_noise_tags)
        .collect::<Vec<_>>()
        .join("\n");
    let trimmed = cleaned.trim();
    (!trimmed.is_empty()).then(|| trimmed.chars().take(80).collect())
}

/// Only structurally complete metadata envelopes are removed from title text.
/// The detail adapter preserves them as inspectable context events.
pub(super) fn strip_noise_tags(text: &str) -> String {
    context::visible_text(text)
}

pub fn parse_meta(
    path: &Path,
    project_dir: &str,
    opts: FamilyOptions,
) -> super::ParseOutcome<SessionMeta> {
    let Ok(file) = super::open_jsonl(path) else {
        return super::ParseOutcome::Failed(format!("无法读取会话文件 {:?}", path));
    };
    let size_bytes = file.metadata().map(|m| m.len()).unwrap_or(0);
    let Some(session_id) = path.file_stem().map(|s| s.to_string_lossy().to_string()) else {
        return super::ParseOutcome::Failed(format!("会话文件名非法: {:?}", path));
    };

    let mut meta = SessionMeta {
        session_id,
        project_dir: project_dir.to_string(),
        cwd: None,
        title: None,
        model: None,
        model_name: None,
        version: None,
        git_branch: None,
        entrypoint: None,
        first_ts: None,
        last_ts: None,
        user_count: 0,
        assistant_count: 0,
        real_user_count: 0,
        assistant_text_count: 0,
        size_bytes,
        has_transcript: true,
        stats_pending: false,
        source_files: vec![path.to_path_buf()],
    };

    let mut ai_title: Option<String> = None;
    let mut custom_title: Option<String> = None;
    let mut last_prompt: Option<String> = None;
    let mut first_user_text: Option<String> = None;

    let bad_lines = super::for_each_jsonl_line(BufReader::new(file).lines(), |v| {
        let Some(t) = v.get("type").and_then(Value::as_str) else {
            return;
        };
        match t {
            "ai-title" if opts.title_events => {
                ai_title = v
                    .get("aiTitle")
                    .and_then(Value::as_str)
                    .map(|s| s.to_string());
            }
            "custom-title" if opts.title_events => {
                custom_title = v
                    .get("customTitle")
                    .and_then(Value::as_str)
                    .map(|s| s.to_string());
            }
            "last-prompt" if opts.title_events => {
                last_prompt = v
                    .get("lastPrompt")
                    .and_then(Value::as_str)
                    .map(|s| s.chars().take(80).collect());
            }
            "runtime-config" => {
                if meta.model.is_none() {
                    if let Some(m) = v.get("model").and_then(Value::as_str) {
                        if !m.is_empty() {
                            meta.model = Some(m.to_string());
                        }
                    }
                }
            }
            "user" | "assistant" => {
                if let Some(ts) = v.get("timestamp").and_then(Value::as_str) {
                    if meta.first_ts.is_none() {
                        meta.first_ts = Some(ts.to_string());
                    }
                    meta.last_ts = Some(ts.to_string());
                }
                if meta.cwd.is_none() {
                    meta.cwd = v.get("cwd").and_then(Value::as_str).map(|s| s.to_string());
                }
                if meta.version.is_none() {
                    meta.version = v
                        .get("version")
                        .and_then(Value::as_str)
                        .map(|s| s.to_string());
                }
                if meta.git_branch.is_none() {
                    meta.git_branch = v
                        .get("gitBranch")
                        .and_then(Value::as_str)
                        .map(|s| s.to_string());
                }
                if meta.entrypoint.is_none() {
                    meta.entrypoint = v
                        .get("entrypoint")
                        .and_then(Value::as_str)
                        .map(|s| s.to_string());
                }
                if t == "user" {
                    meta.user_count += 1;
                    let is_meta = v.get("isMeta") == Some(&Value::Bool(true))
                        || v.get("isCompactSummary") == Some(&Value::Bool(true))
                        || v.get("isVisibleInTranscriptOnly") == Some(&Value::Bool(true));
                    if !is_meta {
                        if let Some(msg) = v.get("message") {
                            if let Some(text) = extract_user_text(msg) {
                                meta.real_user_count += 1;
                                if first_user_text.is_none() {
                                    first_user_text = Some(text);
                                }
                            }
                        }
                    }
                } else {
                    meta.assistant_count += 1;
                    let has_text = v["isMeta"] != true
                        && v["isCompactSummary"] != true
                        && v.pointer("/message/content")
                            .is_some_and(super::context::has_text);
                    if has_text {
                        meta.assistant_text_count += 1;
                    }
                    if meta.model.is_none() {
                        meta.model = v
                            .pointer("/message/model")
                            .and_then(Value::as_str)
                            .filter(|m| !m.is_empty())
                            .map(|s| s.to_string());
                    }
                }
            }
            _ => {}
        }
    });

    meta.title = custom_title
        .or(ai_title)
        .or(last_prompt)
        .or(first_user_text);
    if bad_lines > 0 {
        return super::ParseOutcome::Degraded(meta, super::degraded_message(path, bad_lines));
    }
    super::ParseOutcome::Ok(meta)
}

/// 是否为该 family 下应展示的本地 CLI 会话
fn is_relevant(meta: &SessionMeta, opts: FamilyOptions) -> bool {
    if opts.filter_non_cli_entrypoint {
        if let Some(ep) = &meta.entrypoint {
            if ep != "cli" {
                return false;
            }
        }
    }
    if opts.filter_qoderwork_cwd {
        if let Some(cwd) = &meta.cwd {
            if cwd.contains("/qoderwork/workspace") {
                return false;
            }
        }
    }
    true
}

/// 单遍扫描：每个项目目录只读一次，同时产出项目分组与会话列表。
/// 读取失败不再静默跳过，记入 warnings；base 不存在/不可读同样降级为
/// warning（与 read_dir_warned 的统一遍历语义一致），不让整个 catalog 失败。
pub fn catalog(
    base: &Path,
    opts: FamilyOptions,
    include_empty: bool,
) -> Result<super::Catalog, String> {
    let mut warnings = Vec::new();
    let mut items = Vec::new();
    for entry in super::read_dir_warned(base, &mut warnings) {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let dir_name = entry.file_name().to_string_lossy().to_string();
        if dir_name.starts_with('.') {
            continue;
        }
        let sessions = sessions_in_dir(base, &dir_name, opts, include_empty, &mut warnings);
        // 编码目录无法还原隐藏目录和字面连字符。项目名应由有正文会话的
        // 权威 cwd 决定，不能因显示空会话或文件枚举顺序而被猜测路径覆盖。
        let display = project_display_path(&dir_name, &sessions);
        for meta in sessions {
            items.push((meta, display.clone()));
        }
    }
    let mut catalog = super::assemble_catalog(items);
    catalog.warnings = warnings;
    Ok(catalog)
}

fn project_display_path(dir_name: &str, sessions: &[SessionMeta]) -> String {
    sessions
        .iter()
        .filter_map(|meta| {
            meta.cwd
                .as_deref()
                .filter(|cwd| !cwd.trim().is_empty())
                .map(|cwd| (meta.real_user_count == 0, cwd))
        })
        // Prefer populated sessions; break ties deterministically, independently of read_dir.
        .min()
        .map(|(_, cwd)| cwd.to_string())
        .unwrap_or_else(|| unescape_dir_name(dir_name))
}

fn sessions_in_dir(
    base: &Path,
    dir_name: &str,
    opts: FamilyOptions,
    include_empty: bool,
    warnings: &mut Vec<String>,
) -> Vec<SessionMeta> {
    let dir = base.join(dir_name);
    let entries: Vec<_> = super::read_dir_warned(&dir, warnings);
    let mut sessions = Vec::new();
    let mut seen_ids = std::collections::HashSet::new();

    for entry in &entries {
        let name = entry.file_name().to_string_lossy().to_string();
        let path = entry.path();
        if name.ends_with(".jsonl") {
            let kind = format!(
                "family-context-v2:{}:{}:{}:{dir_name}",
                opts.title_events, opts.filter_non_cli_entrypoint, opts.filter_qoderwork_cwd
            );
            let (parsed, ws) = match super::catalog_cache::summary(&kind, &[path.clone()], || {
                match parse_meta(&path, dir_name, opts) {
                    super::ParseOutcome::Ok(meta) => Ok((meta, Vec::new())),
                    super::ParseOutcome::Degraded(meta, warning) => Ok((meta, vec![warning])),
                    super::ParseOutcome::Failed(error) => Err(error),
                }
            }) {
                Ok((meta, ws)) => (Some(meta), ws),
                Err(error) => (None, vec![error]),
            };
            warnings.extend(ws);
            if let Some(meta) = parsed {
                seen_ids.insert(meta.session_id.clone());
                // 不相关会话（QoderWork/云端/空）静默跳过，不算错误
                if is_relevant(&meta, opts) && (include_empty || meta.real_user_count > 0) {
                    sessions.push(meta);
                }
            }
        }
    }
    // 空壳会话：只有状态目录、没有 transcript
    if include_empty {
        for entry in &entries {
            let name = entry.file_name().to_string_lossy().to_string();
            let path = entry.path();
            if path.is_dir() && is_session_id(&name) && !seen_ids.contains(&name) {
                sessions.push(SessionMeta {
                    session_id: name,
                    project_dir: dir_name.to_string(),
                    cwd: None,
                    title: None,
                    model: None,
                    model_name: None,
                    version: None,
                    git_branch: None,
                    entrypoint: None,
                    first_ts: None,
                    last_ts: None,
                    user_count: 0,
                    assistant_count: 0,
                    real_user_count: 0,
                    assistant_text_count: 0,
                    size_bytes: 0,
                    has_transcript: false,
                    stats_pending: false,
                    source_files: Vec::new(),
                });
            }
        }
    }
    sessions
}

pub fn read_session(
    base: &Path,
    project_dir: &str,
    session_id: &str,
    opts: FamilyOptions,
) -> Result<SessionDetail, String> {
    if !is_safe_name(project_dir) {
        return Err("非法项目目录名".into());
    }
    if !is_session_id(session_id) {
        return Err("非法会话 ID".into());
    }
    let path = base.join(project_dir).join(format!("{session_id}.jsonl"));
    if !path.exists() {
        let state_dir = base.join(project_dir).join(session_id);
        if state_dir.exists() {
            return Ok(SessionDetail {
                warnings: Vec::new(),
                meta: SessionMeta {
                    session_id: session_id.to_string(),
                    project_dir: project_dir.to_string(),
                    cwd: None,
                    title: None,
                    model: None,
                    model_name: None,
                    version: None,
                    git_branch: None,
                    entrypoint: None,
                    first_ts: None,
                    last_ts: None,
                    user_count: 0,
                    assistant_count: 0,
                    real_user_count: 0,
                    assistant_text_count: 0,
                    size_bytes: 0,
                    has_transcript: false,
                    stats_pending: false,
                    source_files: Vec::new(),
                },
                events: Vec::new(),
            });
        }
        return Err("会话文件不存在".to_string());
    }
    // 详情沿用可解析事件，Degraded 告警由目录扫描统一展示；
    // 坏行同样被跳过，不影响事件渲染
    let (meta, _) = parse_meta(&path, project_dir, opts).split();
    let Some(meta) = meta else {
        return Err(format!("会话文件无法解析: {:?}", path));
    };

    let file = super::open_jsonl(&path).map_err(|e| e.to_string())?;
    let mut events = Vec::new();
    super::for_each_jsonl_line(BufReader::new(file).lines(), |v| {
        match v.get("type").and_then(Value::as_str) {
            Some("user" | "assistant" | "system" | "developer" | "attachment") => {
                events.extend(context::normalize(v));
            }
            _ => {}
        }
    });
    Ok(SessionDetail {
        meta,
        events,
        warnings: Vec::new(),
    })
}

/// 删除会话：jsonl 与同名状态目录一起移入回收站
pub fn delete_session(base: &Path, project_dir: &str, session_id: &str) -> Result<(), String> {
    delete_session_with(base, project_dir, session_id, |targets| {
        trash::delete_all(targets).map_err(|e| format!("移入回收站失败: {e}"))
    })
}

fn delete_session_with(
    base: &Path,
    project_dir: &str,
    session_id: &str,
    remove: impl FnOnce(&[PathBuf]) -> Result<(), String>,
) -> Result<(), String> {
    if !is_safe_name(project_dir) {
        return Err("非法项目目录名".into());
    }
    if !is_session_id(session_id) {
        return Err("非法会话 ID".into());
    }
    let dir = base.join(project_dir);
    let jsonl = dir.join(format!("{session_id}.jsonl"));
    let state_dir = dir.join(session_id);

    // 存在性探测统一走 safety::present：只有 NotFound 才算不存在，
    // 悬空符号链接也算存在，随后会被 verify_within_root 拒绝，
    // 避免跳过它却删掉另一半（部分删除）
    let jsonl_present = super::present(&jsonl)?;
    let state_present = super::present(&state_dir)?;

    // 全部目标先验证、后删除：任一校验失败都不动任何文件
    let mut targets: Vec<PathBuf> = Vec::new();
    if jsonl_present {
        targets.push(super::verify_within_root(
            base,
            &jsonl,
            super::TargetKind::File,
        )?);
    }
    if state_present {
        targets.push(super::verify_within_root(
            base,
            &state_dir,
            super::TargetKind::Dir,
        )?);
    }
    if targets.is_empty() {
        return Err("未找到会话文件".into());
    }
    if jsonl_present {
        // 结构校验通过后再复核内容身份：sessionId 必须与文件名一致
        verify_jsonl_session_id(&jsonl, session_id)?;
    }
    remove(&targets)
}

/// 复核 jsonl 身份与待删文件名一致：
/// - 任一行 sessionId 与待删 ID 不一致 → 中止；
/// - 旧格式没有任何 sessionId 时走可信兼容回退：文件必须逐行可解析、
///   且确为对话转录（含 user/assistant 事件）才放行；
///   损坏或空文件一律拒绝。
fn verify_jsonl_session_id(path: &Path, session_id: &str) -> Result<(), String> {
    let file = super::open_jsonl(path).map_err(|e| e.to_string())?;
    let mut found = false;
    let mut has_message = false;
    for line in BufReader::new(file).lines() {
        let line = line.map_err(|e| format!("读取 {:?} 失败: {e}", path))?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            return Err(format!(
                "{:?} 含无法解析的行，无法完成身份复核，已中止",
                path
            ));
        };
        if let Some(id) = v.get("sessionId").and_then(Value::as_str) {
            if id != session_id {
                return Err(format!(
                    "文件内 sessionId（{id}）与待删会话（{session_id}）不一致，已中止"
                ));
            }
            found = true;
        }
        if matches!(
            v.get("type").and_then(Value::as_str),
            Some("user") | Some("assistant")
        ) {
            has_message = true;
        }
    }
    if found {
        return Ok(());
    }
    // 旧格式兼容回退：无 sessionId 字段，但文件完整可解析且确为对话转录
    if has_message {
        return Ok(());
    }
    Err(format!(
        "文件 {:?} 既无 sessionId 也无对话内容，无法完成身份复核，已中止",
        path
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const QODER_OPTS: FamilyOptions = FamilyOptions {
        title_events: true,
        filter_non_cli_entrypoint: true,
        filter_qoderwork_cwd: true,
    };

    const CLAUDE_OPTS: FamilyOptions = FamilyOptions {
        title_events: false,
        filter_non_cli_entrypoint: false,
        filter_qoderwork_cwd: false,
    };

    #[test]
    fn empty_sessions_keep_authoritative_project_path_when_toggled() {
        let temp = tempfile::tempdir().unwrap();
        let dir_name = "-Users-demo--codex";
        let project = temp.path().join(dir_name);
        fs::create_dir(&project).unwrap();
        fs::write(project.join("aaaa.jsonl"), "").unwrap();
        fs::write(project.join("bbbb.jsonl"), "{\"type\":\"user\",\"cwd\":\"/Users/demo/.codex\",\"message\":{\"content\":\"Existing conversation\"}}\n").unwrap();
        // Even an empty transcript with a conflicting cwd must not rename a populated project.
        fs::write(
            project.join("cccc.jsonl"),
            "{\"type\":\"user\",\"cwd\":\"/Users/demo//codex\",\"message\":{\"content\":\"\"}}\n",
        )
        .unwrap();
        fs::create_dir(project.join("dddd")).unwrap();
        for opts in [CLAUDE_OPTS, QODER_OPTS] {
            let collapsed = catalog(temp.path(), opts, false).unwrap();
            let expanded = catalog(temp.path(), opts, true).unwrap();
            for (data, count) in [(&collapsed, 1), (&expanded, 4)] {
                assert!(data.warnings.is_empty());
                assert_eq!(data.projects.len(), 1);
                assert_eq!(data.projects[0].display_path, "/Users/demo/.codex");
                assert_eq!(data.projects[0].dir_name, dir_name);
                assert_eq!(data.projects[0].session_count, count);
                assert_eq!(data.sessions.len(), count);
                assert!(data.sessions.iter().all(|s| s.project_dir == dir_name));
            }
            assert!(expanded
                .sessions
                .iter()
                .find(|s| s.session_id == "aaaa")
                .unwrap()
                .cwd
                .is_none());
            let mut reversed = expanded.sessions.clone();
            reversed.reverse();
            assert_eq!(
                project_display_path(dir_name, &reversed),
                "/Users/demo/.codex"
            );
            for include_empty in [false, true, false] {
                assert_eq!(
                    catalog(temp.path(), opts, include_empty).unwrap().projects[0].display_path,
                    "/Users/demo/.codex"
                );
            }
            if !opts.filter_non_cli_entrypoint {
                if let Ok(destination) = std::env::var("ASM_EMPTY_PATH_EXPORT") {
                    use std::io::Write;
                    use std::os::unix::fs::OpenOptionsExt;
                    let mut file = fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .mode(0o600)
                        .open(destination)
                        .unwrap();
                    file.write_all(
                        &serde_json::to_vec(
                            &serde_json::json!({"collapsed":collapsed,"expanded":expanded}),
                        )
                        .unwrap(),
                    )
                    .unwrap();
                }
            }
        }
        let sessions = sessions_in_dir(temp.path(), dir_name, CLAUDE_OPTS, true, &mut Vec::new());
        let mut real = sessions
            .into_iter()
            .find(|s| s.session_id == "bbbb")
            .unwrap();
        real.cwd = Some("/Users/demo/.config/my-project".into());
        assert_eq!(
            project_display_path("-Users-demo--config-my-project", &[real]),
            "/Users/demo/.config/my-project"
        );
    }

    #[test]
    fn cached_catalog_observes_rewrites_new_files_deletions_and_filter_changes() {
        use std::io::Write;
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        fs::create_dir(&project).unwrap();
        let file = project.join("aaaa.jsonl");
        let user =
            "{\"type\":\"user\",\"entrypoint\":\"cli\",\"message\":{\"content\":\"question\"}}\n";
        fs::write(&file, user).unwrap();
        assert_eq!(
            catalog(temp.path(), QODER_OPTS, false).unwrap().sessions[0].real_user_count,
            1
        );
        fs::OpenOptions::new()
            .append(true)
            .open(&file)
            .unwrap()
            .write_all(user.as_bytes())
            .unwrap();
        assert_eq!(
            catalog(temp.path(), QODER_OPTS, false).unwrap().sessions[0].real_user_count,
            2
        );
        fs::write(&file, "").unwrap();
        assert!(catalog(temp.path(), QODER_OPTS, false)
            .unwrap()
            .sessions
            .is_empty());
        assert_eq!(
            catalog(temp.path(), QODER_OPTS, true)
                .unwrap()
                .sessions
                .len(),
            1
        );
        fs::write(project.join("bbbb.jsonl"), user).unwrap();
        assert_eq!(
            catalog(temp.path(), QODER_OPTS, false).unwrap().sessions[0].session_id,
            "bbbb"
        );
        fs::remove_file(project.join("bbbb.jsonl")).unwrap();
        assert!(catalog(temp.path(), QODER_OPTS, false)
            .unwrap()
            .sessions
            .is_empty());
    }

    fn write_fixture(lines: &[&str]) -> tempfile::NamedTempFile {
        use std::io::Write;
        let mut f = tempfile::NamedTempFile::new().unwrap();
        for line in lines {
            writeln!(f, "{line}").unwrap();
        }
        f
    }

    fn meta_of(path: &Path, project_dir: &str, opts: FamilyOptions) -> SessionMeta {
        let (parsed, _) = parse_meta(path, project_dir, opts).split();
        match parsed {
            Some(m) => m,
            None => panic!("解析失败"),
        }
    }

    #[test]
    fn parse_meta_extracts_fields() {
        let f = write_fixture(&[
            r#"{"type":"workspace-directories","sessionId":"abc","directories":["/tmp/proj"]}"#,
            r#"{"type":"user","uuid":"u1","timestamp":"2026-09-03T06:00:00Z","message":{"role":"user","content":"帮我写个排序算法"},"isMeta":false,"cwd":"/tmp/proj","sessionId":"abc","version":"1.1.41","gitBranch":"main"}"#,
            r#"{"type":"assistant","uuid":"a1","timestamp":"2026-09-03T06:01:00Z","message":{"role":"assistant","model":"mode-xyz","content":[{"type":"text","text":"好的"}]},"cwd":"/tmp/proj","sessionId":"abc"}"#,
            r#"{"type":"user","uuid":"u2","timestamp":"2026-09-03T06:02:00Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"ok"}]},"isMeta":false,"cwd":"/tmp/proj","sessionId":"abc"}"#,
            r#"{"type":"ai-title","sessionId":"abc","aiTitle":"排序算法实现"}"#,
        ]);
        let meta = meta_of(f.path(), "-tmp-proj", QODER_OPTS);
        assert_eq!(meta.title.as_deref(), Some("排序算法实现"));
        assert_eq!(meta.cwd.as_deref(), Some("/tmp/proj"));
        assert_eq!(meta.model.as_deref(), Some("mode-xyz"));
        assert_eq!(meta.version.as_deref(), Some("1.1.41"));
        assert_eq!(meta.git_branch.as_deref(), Some("main"));
        assert_eq!(meta.user_count, 2);
        assert_eq!(meta.assistant_count, 1);
        assert_eq!(meta.assistant_text_count, 1);
        assert_eq!(meta.first_ts.as_deref(), Some("2026-09-03T06:00:00Z"));
        assert_eq!(meta.last_ts.as_deref(), Some("2026-09-03T06:02:00Z"));
        assert!(meta.has_transcript);
    }

    #[test]
    fn title_priority_custom_over_ai_over_prompt() {
        let f = write_fixture(&[
            r#"{"type":"ai-title","aiTitle":"AI标题"}"#,
            r#"{"type":"custom-title","customTitle":"自定义标题"}"#,
            r#"{"type":"last-prompt","lastPrompt":"最后的提问"}"#,
        ]);
        let meta = meta_of(f.path(), "p", QODER_OPTS);
        assert_eq!(meta.title.as_deref(), Some("自定义标题"));
    }

    #[test]
    fn title_falls_back_to_last_prompt_then_first_user_text() {
        let f = write_fixture(&[
            r#"{"type":"user","message":{"role":"user","content":"第一条用户消息"},"isMeta":false}"#,
            r#"{"type":"last-prompt","lastPrompt":"最后的提问"}"#,
        ]);
        let meta = meta_of(f.path(), "p", QODER_OPTS);
        assert_eq!(meta.title.as_deref(), Some("最后的提问"));

        let f2 = write_fixture(&[
            r#"{"type":"user","message":{"role":"user","content":"<local-command-caveat>忽略</local-command-caveat>"},"isMeta":true}"#,
            r#"{"type":"user","message":{"role":"user","content":"真正的问题"},"isMeta":false}"#,
        ]);
        let meta2 = meta_of(f2.path(), "p", QODER_OPTS);
        assert_eq!(meta2.title.as_deref(), Some("真正的问题"));
    }

    #[test]
    fn claude_mode_ignores_title_events_and_uses_first_user_text() {
        let f = write_fixture(&[
            r#"{"type":"ai-title","aiTitle":"不该采用的标题"}"#,
            r#"{"type":"user","message":{"role":"user","content":"Claude 的第一条消息"},"isMeta":false}"#,
        ]);
        let meta = meta_of(f.path(), "p", CLAUDE_OPTS);
        assert_eq!(meta.title.as_deref(), Some("Claude 的第一条消息"));
    }

    #[test]
    fn session_id_validation() {
        assert!(is_session_id("2c044707-ec1a-4b99-aeab-cbeea8745226"));
        assert!(!is_session_id("../etc/passwd"));
        assert!(!is_session_id("abc/def"));
        assert!(!is_session_id(""));
        assert!(is_safe_name("-Users-liangyukun-code-my"));
        assert!(!is_safe_name("../x"));
    }

    #[test]
    fn slash_command_message_counts_as_real_user() {
        // 斜杠命令消息：真实用户输入在 <command-args> 内，不应被当作空消息
        let f = write_fixture(&[
            r#"{"type":"user","message":{"role":"user","content":"<command-message>yuque-kit</command-message><command-name>/yuque-kit</command-name><command-args>在语雀和ATA上都给我调研一下voyager2.0</command-args>"},"isMeta":false}"#,
        ]);
        let meta = meta_of(f.path(), "p", CLAUDE_OPTS);
        assert_eq!(meta.real_user_count, 1, "斜杠命令消息应计为真实用户输入");
        assert!(
            meta.title.as_deref().unwrap().contains("调研"),
            "标题应提取 command-args 内的用户文本: {:?}",
            meta.title
        );
    }

    #[test]
    fn strip_noise_tags_removes_noise_keeps_command_args() {
        let input = "<command-message>kit</command-message><command-name>/kit</command-name><command-args>用户问题</command-args>";
        let cleaned = strip_noise_tags(input);
        // command-name 和 command-args 内文保留，command-message 删除
        assert!(cleaned.contains("/kit"), "斜杠命令名应保留: {cleaned}");
        assert!(cleaned.contains("用户问题"), "用户输入应保留: {cleaned}");
        assert!(!cleaned.contains("command-"), "标签本身应被剥离: {cleaned}");
    }

    #[test]
    fn strip_noise_tags_preserves_plain_text() {
        assert_eq!(strip_noise_tags("你好世界"), "你好世界");
    }

    #[test]
    fn pure_noise_message_is_still_empty() {
        // 纯噪音（无 command-args）剥离后应为空
        let input = "<environment_context>cli\n/Users/me/proj</environment_context>";
        assert_eq!(strip_noise_tags(input).trim(), "");
    }

    #[test]
    fn filters_non_cli_entrypoints_and_qoderwork() {
        let f = write_fixture(&[
            r#"{"type":"user","message":{"role":"user","content":"你好"},"isMeta":false,"entrypoint":"cli","cwd":"/Users/me/proj"}"#,
        ]);
        let meta = meta_of(f.path(), "p", QODER_OPTS);
        assert!(is_relevant(&meta, QODER_OPTS));

        let f2 = write_fixture(&[
            r#"{"type":"user","message":{"role":"user","content":"你好"},"isMeta":false,"entrypoint":"qoderwork","cwd":"/root/workspace"}"#,
        ]);
        let meta2 = meta_of(f2.path(), "p", QODER_OPTS);
        assert!(!is_relevant(&meta2, QODER_OPTS));

        let f3 = write_fixture(&[
            r#"{"type":"user","message":{"role":"user","content":"你好"},"isMeta":false,"entrypoint":"cli","cwd":"/Users/me/qoderwork/workspace-abc"}"#,
        ]);
        let meta3 = meta_of(f3.path(), "p", QODER_OPTS);
        assert!(!is_relevant(&meta3, QODER_OPTS));

        // Claude 模式不做来源过滤
        assert!(is_relevant(&meta2, CLAUDE_OPTS));
    }

    #[test]
    fn shell_quote_escapes_single_quotes() {
        assert_eq!(super::super::shell_quote("a b"), "'a b'");
        assert_eq!(super::super::shell_quote("it's"), "'it'\\''s'");
    }

    #[test]
    fn parse_meta_reports_degraded_on_bad_lines() {
        let f = write_fixture(&[
            r#"{"type":"user","uuid":"u1","timestamp":"2026-09-03T06:00:00Z","message":{"role":"user","content":"你好"},"isMeta":false,"sessionId":"abc"}"#,
            "这一行不是合法 JSON",
        ]);
        let (parsed, ws) = parse_meta(f.path(), "p", QODER_OPTS).split();
        assert!(parsed.is_some(), "瑕疵文件仍应产出 meta");
        assert_eq!(ws.len(), 1, "跳过的坏行必须产生告警");
        assert!(ws[0].contains("无法解析"), "{}", ws[0]);
    }

    #[test]
    fn delete_refuses_when_inner_session_id_mismatches() {
        let tmp = tempfile::tempdir().unwrap();
        let proj = tmp.path().join("-tmp-proj");
        fs::create_dir_all(&proj).unwrap();
        let jsonl = proj.join("aaaa-1111.jsonl");
        fs::write(&jsonl, r#"{"type":"user","sessionId":"bbbb-2222"}"#).unwrap();

        let err = delete_session(tmp.path(), "-tmp-proj", "aaaa-1111").unwrap_err();
        assert!(err.contains("不一致"), "{err}");
        assert!(jsonl.exists(), "中止后文件必须完好");
    }

    #[test]
    fn delete_allows_legacy_transcript_without_session_id() {
        let tmp = tempfile::tempdir().unwrap();
        let proj = tmp.path().join("-tmp-proj");
        fs::create_dir_all(&proj).unwrap();
        let jsonl = proj.join("aaaa-1111.jsonl");
        // 旧格式：无 sessionId 字段，但逐行可解析且确为对话转录 → 可信回退放行
        fs::write(
            &jsonl,
            r#"{"type":"user","message":{"content":"旧格式消息"}}"#,
        )
        .unwrap();

        delete_session_with(tmp.path(), "-tmp-proj", "aaaa-1111", |targets| {
            assert_eq!(targets, &[jsonl.canonicalize().unwrap()]);
            fs::remove_file(&targets[0]).map_err(|e| e.to_string())
        })
        .unwrap();
        assert!(!jsonl.exists(), "旧格式转录应可删除");
    }

    #[test]
    fn delete_refuses_corrupt_file_without_session_id() {
        let tmp = tempfile::tempdir().unwrap();
        let proj = tmp.path().join("-tmp-proj");
        fs::create_dir_all(&proj).unwrap();
        let jsonl = proj.join("aaaa-1111.jsonl");
        fs::write(&jsonl, "不是合法 JSON").unwrap();

        let err = delete_session(tmp.path(), "-tmp-proj", "aaaa-1111").unwrap_err();
        assert!(err.contains("无法解析"), "{err}");
        assert!(jsonl.exists(), "损坏文件不得删除");
    }

    #[test]
    fn delete_refuses_dangling_symlink_without_partial_delete() {
        let tmp = tempfile::tempdir().unwrap();
        let proj = tmp.path().join("-tmp-proj");
        fs::create_dir_all(&proj).unwrap();
        // 悬空符号链接冒充 jsonl；同名状态目录真实存在
        let jsonl = proj.join("aaaa-1111.jsonl");
        std::os::unix::fs::symlink(proj.join("不存在的目标"), &jsonl).unwrap();
        let state_dir = proj.join("aaaa-1111");
        fs::create_dir_all(&state_dir).unwrap();

        let err = delete_session(tmp.path(), "-tmp-proj", "aaaa-1111").unwrap_err();
        assert!(err.contains("符号链接"), "{err}");
        assert!(state_dir.exists(), "不得发生部分删除：状态目录必须完好");
        assert!(jsonl.symlink_metadata().is_ok(), "悬空链接本身也不得被动");
    }
}
