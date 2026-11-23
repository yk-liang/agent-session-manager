//! Qoder 和 Claude Code 共用的 JSONL 会话解析。消息由 uuid 和 parentUuid 串联。

mod context;
mod subagents;
mod tools;
#[cfg(test)]
mod context_tests;
#[cfg(test)]
mod diagnostic_tests;

use serde_json::Value;
#[cfg(test)]
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use super::{SessionDetail, SessionMeta};

#[derive(Clone, Copy)]
pub struct FamilyOptions {
    /// Qoder 的运行配置可补足缺失或标为 auto 的助手模型。
    pub use_runtime_model: bool,
    /// 过滤 entrypoint 非 cli 的会话（QoderWork/云端）
    pub filter_non_cli_entrypoint: bool,
}

pub fn is_safe_name(name: &str) -> bool {
    !name.is_empty() && !name.contains('\0') && !name.contains('/') && !name.contains('\\') && name != "." && name != ".."
}

pub fn is_session_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

/// 项目目录名是路径转义（'/' → '-'），反转义仅作展示兜底；
/// 权威路径来自 JSONL 内的 cwd 字段。
pub fn unescape_dir_name(name: &str) -> String {
    name.replace('-', "/")
}

/// 斜杠命令的用户输入在 `<command-args>` 内。注入的上下文不计入用户消息。
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

/// 完整的元数据块从标题中排除，仍可在会话上下文中查看。
pub(super) fn strip_noise_tags(text: &str) -> String {
    context::visible_text(text)
}

pub fn parse_meta(
    path: &Path,
    project_dir: &str,
    opts: FamilyOptions,
) -> super::ParseOutcome<SessionMeta> {
    parse_meta_with(path, project_dir, opts, "catalog_metadata", |_| {})
}

fn parse_meta_with(
    path: &Path,
    project_dir: &str,
    opts: FamilyOptions,
    operation: &str,
    mut observe: impl FnMut(Value),
) -> super::ParseOutcome<SessionMeta> {
    let file = match super::scanning::open_jsonl_diagnosed(path, operation) {
        Ok(file) => file,
        Err(error) => return super::ParseOutcome::Failed(error),
    };
    let size_bytes = match file.metadata() {
        Ok(meta) => meta.len(),
        Err(error) => return super::ParseOutcome::Failed(super::diagnostics::io(
            operation, "file_metadata", path, &error,
        )),
    };
    let Some(session_id) = path.file_stem().map(|s| s.to_string_lossy().to_string()) else {
        return super::ParseOutcome::Failed(format!("会话文件名无效：{:?}", path));
    };

    let mut meta = SessionMeta {
        session_id,
        project_dir: project_dir.to_string(),
        cwd: None,
        title: None,
        model: None,
        version: None,
        client: None,
        git_branch: None,
        entrypoint: None,
        codex: None,
        subagent: None,
        first_ts: None,
        last_ts: None,
        user_count: 0,
        assistant_count: 0,
        real_user_count: 0,
        assistant_text_count: 0,
        size_bytes: Some(size_bytes),
        has_transcript: true,
        archived: false,
        stats_pending: false,
        source_files: vec![path.to_path_buf()],
    };

    let mut ai_title: Option<String> = None;
    let mut custom_title: Option<String> = None;
    let mut last_prompt: Option<String> = None;
    let mut first_user_text: Option<String> = None;
    let mut configured_model: Option<String> = None;
    let mut last_assistant_model: Option<String> = None;

    let result = super::scanning::read_jsonl_checked(BufReader::new(file), path, operation, |v| {
        let mut v = v;
        context::promote_queued_prompt(&mut v);
        match v["type"].as_str().unwrap_or_default() {
            "ai-title" => ai_title = nonempty(&v["aiTitle"]).map(str::to_owned),
            "custom-title" => custom_title = nonempty(&v["customTitle"]).map(str::to_owned),
            "last-prompt" => last_prompt = nonempty(&v["lastPrompt"]).map(|s| s.chars().take(80).collect()),
            "runtime-config" if opts.use_runtime_model => {
                if let Some(model) = context::valid_model(&v["model"]) {
                    configured_model = Some(model.to_owned());
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
                let assistant = v["type"] == "assistant";
                if let Some(version) = nonempty(&v["version"])
                    .filter(|_| assistant || meta.version.is_none())
                {
                    meta.version = Some(version.to_owned());
                }
                if meta.git_branch.is_none() {
                    meta.git_branch = v
                        .get("gitBranch")
                        .and_then(Value::as_str)
                        .map(|s| s.to_string());
                }
                if let Some(entrypoint) = nonempty(&v["entrypoint"]) {
                    if meta.entrypoint.is_none() {
                        meta.entrypoint = Some(entrypoint.to_owned());
                    }
                    if assistant {
                        meta.client = Some(entrypoint.to_owned());
                    }
                }
                if v["type"] == "user" {
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
                            } else if msg["content"].as_array().is_some_and(|parts| parts.iter().any(super::context::is_attachment)) {
                                meta.real_user_count += 1;
                            }
                        }
                    }
                } else {
                    meta.assistant_count += 1;
                    if let Some(model) = context::valid_model(&v["message"]["model"]) {
                        last_assistant_model = Some(model.to_owned());
                    }
                    if v["isMeta"] != true
                        && v["isCompactSummary"] != true
                        && v["isApiErrorMessage"] != true
                        && super::context::has_text(&v["message"]["content"])
                    {
                        meta.assistant_text_count += 1;
                    }
                }
            }
            _ => {}
        }
        observe(v);
    });

    if meta.client.is_none() {
        meta.client.clone_from(&meta.entrypoint);
    }
    meta.model = match last_assistant_model {
        Some(model) if model != "auto" => Some(model),
        model => configured_model.or(model),
    };
    meta.title = custom_title
        .or(ai_title)
        .or(first_user_text)
        .or(last_prompt);
    match result {
        Ok(None) => super::ParseOutcome::Ok(meta),
        Ok(Some(warning)) => super::ParseOutcome::Degraded(meta, warning),
        Err(error) => super::ParseOutcome::Failed(error),
    }
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
    true
}

fn nonempty(value: &Value) -> Option<&str> {
    value.as_str().map(str::trim).filter(|s| !s.is_empty())
}

fn summary_cache_kind(dir_name: &str, opts: FamilyOptions) -> String {
    format!("family-transcript-v6:{}:{}:{dir_name}", opts.use_runtime_model, opts.filter_non_cli_entrypoint)
}

/// 一次扫描生成项目分组和会话列表。读取失败时保留已读会话和诊断。
pub fn catalog(
    base: &Path,
    opts: FamilyOptions,
    include_empty: bool,
    cancelled: &dyn Fn() -> bool,
) -> Result<super::Catalog, String> {
    let mut warnings = Vec::new();
    let mut catalog = super::assemble_catalog(catalog_items(base, opts, include_empty, &mut warnings, cancelled)?);
    catalog.warnings = warnings;
    Ok(catalog)
}

pub(super) fn catalog_items(
    base: &Path,
    opts: FamilyOptions,
    include_empty: bool,
    warnings: &mut Vec<String>,
    cancelled: &dyn Fn() -> bool,
) -> Result<Vec<(SessionMeta, String)>, String> {
    let mut items = Vec::new();
    for entry in super::read_dir_warned(base, warnings) {
        super::check_catalog_cancelled(cancelled)?;
        if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        let Ok(dir_name) = entry.file_name().into_string() else {
            continue;
        };
        if dir_name.starts_with('.') || !is_safe_name(&dir_name) {
            continue;
        }
        let sessions = sessions_in_dir(base, &dir_name, opts, include_empty, warnings, cancelled)?;
        // 编码目录无法还原隐藏目录和字面连字符。项目名应由有正文会话的
        // 权威 cwd 决定，不能因显示空会话或文件枚举顺序而被猜测路径覆盖。
        let display = project_display_path(&dir_name, &sessions);
        for meta in sessions {
            items.push((meta, display.clone()));
        }
    }
    Ok(items)
}

fn project_display_path(dir_name: &str, sessions: &[SessionMeta]) -> String {
    sessions
        .iter()
        .filter_map(|meta| {
            meta.cwd
                .as_deref()
                .filter(|cwd| !cwd.trim().is_empty())
                .map(|cwd| (meta.subagent.is_some(), meta.real_user_count == 0, cwd))
        })
        // 优先采用有正文的会话。条件相同时使用与枚举顺序无关的排序。
        .min()
        .map(|(_, _, cwd)| cwd.to_string())
        .unwrap_or_else(|| unescape_dir_name(dir_name))
}

fn cached_meta(kind: &str, path: &Path, project_dir: &str, opts: FamilyOptions) -> Result<(SessionMeta, Vec<String>), String> {
    super::catalog_cache::summary(kind, &[path.to_path_buf()], || match parse_meta(path, project_dir, opts) {
        super::ParseOutcome::Ok(meta) => Ok((meta, Vec::new())),
        super::ParseOutcome::Degraded(meta, warning) => Ok((meta, vec![warning])),
        super::ParseOutcome::Failed(error) => Err(error),
    })
}

fn sessions_in_dir(
    base: &Path,
    dir_name: &str,
    opts: FamilyOptions,
    include_empty: bool,
    warnings: &mut Vec<String>,
    cancelled: &dyn Fn() -> bool,
) -> Result<Vec<SessionMeta>, String> {
    let dir = base.join(dir_name);
    let entries: Vec<_> = super::read_dir_warned(&dir, warnings);
    let mut sessions = Vec::new();
    let mut seen_ids = std::collections::HashSet::new();

    for entry in &entries {
        super::check_catalog_cancelled(cancelled)?;
        let name = entry.file_name().to_string_lossy().to_string();
        let path = entry.path();
        if name.strip_suffix(".jsonl").is_some_and(is_session_id)
            && !entry.file_type().is_ok_and(|kind| kind.is_symlink() || kind.is_dir())
        {
            let kind = summary_cache_kind(dir_name, opts);
            let (parsed, ws) = match cached_meta(&kind, &path, dir_name, opts) {
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
    for entry in &entries {
        super::check_catalog_cancelled(cancelled)?;
        let name = entry.file_name();
        let Some(root_id) = name.to_str().filter(|name| is_session_id(name)) else { continue };
        if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        for child in subagents::children(&dir, root_id, warnings) {
            super::check_catalog_cancelled(cancelled)?;
            let kind = summary_cache_kind(dir_name, opts);
            match cached_meta(&kind, &child.log, dir_name, opts) {
                Ok((mut meta, ws)) => {
                    warnings.extend(ws);
                    child.apply(&mut meta, dir_name);
                    if is_relevant(&meta, opts) && (include_empty || !meta.is_empty()) {
                        sessions.push(meta);
                    }
                }
                Err(error) => warnings.push(error),
            }
        }
    }
    // 空壳会话：只有状态目录、没有 transcript
    if include_empty {
        for entry in &entries {
            let name = entry.file_name().to_string_lossy().to_string();
            if entry.file_type().is_ok_and(|kind| kind.is_dir())
                && is_session_id(&name) && !seen_ids.contains(&name) {
                sessions.push(SessionMeta {
                    session_id: name,
                    project_dir: dir_name.to_string(),
                    cwd: None,
                    title: None,
                    model: None,
                    version: None,
                    client: None,
                    git_branch: None,
                    entrypoint: None,
                    codex: None,
                    subagent: None,
                    first_ts: None,
                    last_ts: None,
                    user_count: 0,
                    assistant_count: 0,
                    real_user_count: 0,
                    assistant_text_count: 0,
                    size_bytes: None,
                    has_transcript: false,
                    archived: false,
                    stats_pending: false,
                    source_files: Vec::new(),
                });
            }
        }
    }
    Ok(sessions)
}

pub fn read_session(
    base: &Path,
    project_dir: &str,
    session_id: &str,
    opts: FamilyOptions,
) -> Result<SessionDetail, String> {
    if !is_safe_name(project_dir) {
        return Err("项目目录名无效".into());
    }
    let project = base.join(project_dir);
    if let Some((root_id, id)) = subagents::child_identity(session_id) {
        let project = super::verify_within_root(base, &project, super::TargetKind::Dir)?;
        let mut warnings = Vec::new();
        let children = subagents::children(&project, root_id, &mut warnings);
        let child = children.iter().find(|child| child.id == id).ok_or("未找到该子会话")?;
        let log = super::verify_within_root(&project, &child.log, super::TargetKind::File)?;
        let mut detail = read_transcript(&log, project_dir, opts)?;
        child.apply(&mut detail.meta, project_dir);
        detail.warnings.extend(warnings);
        return Ok(detail);
    }
    if !is_session_id(session_id) {
        return Err("会话 ID 无效".into());
    }
    let path = project.join(format!("{session_id}.jsonl"));
    let missing = match std::fs::metadata(&path) {
        Ok(_) => None,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some(error),
        Err(error) => return Err(super::diagnostics::io(
            "session_detail", "path_metadata", &path, &error,
        )),
    };
    if let Some(missing_error) = missing {
        let state_dir = base.join(project_dir).join(session_id);
        let state_exists = match std::fs::metadata(&state_dir) {
            Ok(meta) => meta.is_dir(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => return Err(super::diagnostics::io(
                "session_detail", "state_directory_metadata", &state_dir, &error,
            )),
        };
        if state_exists {
            return Ok(SessionDetail {
                warnings: Vec::new(),
                meta: SessionMeta {
                    session_id: session_id.to_string(),
                    project_dir: project_dir.to_string(),
                    cwd: None,
                    title: None,
                    model: None,
                    version: None,
                    client: None,
                    git_branch: None,
                    entrypoint: None,
                    codex: None,
                    subagent: None,
                    first_ts: None,
                    last_ts: None,
                    user_count: 0,
                    assistant_count: 0,
                    real_user_count: 0,
                    assistant_text_count: 0,
                    size_bytes: None,
                    has_transcript: false,
                    archived: false,
                    stats_pending: false,
                    source_files: Vec::new(),
                },
                events: Vec::new(),
            });
        }
        return Err(super::diagnostics::io(
            "session_detail", "path_metadata", &path, &missing_error,
        ));
    }
    read_transcript(&path, project_dir, opts)
}

pub(super) fn search_events(
    base: &Path,
    project_dir: &str,
    meta: &SessionMeta,
    opts: FamilyOptions,
) -> Result<Vec<Value>, String> {
    let Some((root, id)) = subagents::child_identity(&meta.session_id) else {
        return read_session(base, project_dir, &meta.session_id, opts).map(|detail| detail.events);
    };
    let project = super::verify_within_root(base, &base.join(project_dir), super::TargetKind::Dir)?;
    let log = super::verify_within_root(&project, &subagents::child_log(&project, root, id), super::TargetKind::File)?;
    read_transcript(&log, project_dir, opts).map(|detail| detail.events)
}

fn read_transcript(path: &Path, project_dir: &str, opts: FamilyOptions) -> Result<SessionDetail, String> {
    let mut events = Vec::new();
    let mut injected = std::collections::HashMap::new();
    // 同一份读取同时提取摘要和详情，保留阶段错误，避免二次打开时错误被替换。
    let outcome = parse_meta_with(path, project_dir, opts, "session_detail", |v| {
        if let (true, Some(call), Some(uuid), Some(text)) = (v["isMeta"] == true, v["sourceToolUseID"].as_str(), v["uuid"].as_str(), tools::injected_text(&v)) {
            injected.insert(call.to_owned(), (uuid.to_owned(), text));
        }
        match v.get("type").and_then(Value::as_str) {
            Some("user" | "assistant" | "system" | "developer" | "attachment") => {
                events.extend(context::normalize(v));
            }
            _ => {}
        }
    });
    let (meta, warnings) = match outcome {
        super::ParseOutcome::Ok(meta) => (meta, Vec::new()),
        super::ParseOutcome::Degraded(meta, warning) => (meta, vec![warning]),
        super::ParseOutcome::Failed(error) => return Err(error),
    };
    tools::describe(&mut events, injected);
    Ok(SessionDetail {
        meta,
        events,
        warnings,
    })
}

/// 会话日志和同名状态目录一并移入废纸篓。
pub fn delete_session(base: &Path, project_dir: &str, session_id: &str, preserved: Option<&[PathBuf]>) -> Result<super::SessionDeletion, String> {
    let plan = deletion_plan(base, project_dir, session_id)?;
    super::check_preserved_targets(&plan.targets, preserved)?;
    Ok(super::trash_session_files(project_dir, &plan.targets, plan.sessions))
}

#[cfg(test)]
fn delete_session_with(
    base: &Path,
    project_dir: &str,
    session_id: &str,
    remove: impl FnOnce(&[PathBuf]) -> Result<(), String>,
) -> Result<(), String> {
    remove(&deletion_targets(base, project_dir, session_id)?)
}

/// 与实际删除共用校验和目标集合，供删除前用量保存使用。
pub(crate) fn deletion_targets(
    base: &Path,
    project_dir: &str,
    session_id: &str,
) -> Result<Vec<PathBuf>, String> {
    deletion_plan(base, project_dir, session_id).map(|plan| plan.targets)
}

#[derive(Default)]
struct DeletionPlan {
    targets: Vec<PathBuf>,
    sessions: Vec<(String, PathBuf)>,
}

impl DeletionPlan {
    fn add_session(&mut self, id: String, path: PathBuf) {
        self.sessions.push((id, path.clone()));
        self.targets.push(path);
    }
}

fn deletion_plan(base: &Path, project_dir: &str, session_id: &str) -> Result<DeletionPlan, String> {
    if !is_safe_name(project_dir) {
        return Err("项目目录名无效".into());
    }
    let dir = base.join(project_dir);
    let project = super::verify_within_root(base, &dir, super::TargetKind::Dir)?;
    if let Some((root_id, id)) = subagents::child_identity(session_id) {
        let mut warnings = Vec::new();
        let children = subagents::children(&project, root_id, &mut warnings);
        if !warnings.is_empty() {
            return Err(warnings.join("；"));
        }
        return subagents::deletion_plan(&project, &children, id);
    }
    if !is_session_id(session_id) {
        return Err("会话 ID 无效".into());
    }
    let jsonl = project.join(format!("{session_id}.jsonl"));
    let state_dir = project.join(session_id);
    let jsonl_present = super::present(&jsonl)?;
    let state_present = super::present(&state_dir)?;
    if !jsonl_present && !state_present {
        return Err("未找到会话文件".into());
    }
    let mut plan = DeletionPlan::default();
    if jsonl_present {
        plan.add_session(session_id.to_owned(), super::verify_within_root(
            &project,
            &jsonl,
            super::TargetKind::File,
        )?);
    }
    if state_present {
        plan.add_session(session_id.to_owned(), super::verify_within_root(
            &project,
            &state_dir,
            super::TargetKind::Dir,
        )?);
        let mut warnings = Vec::new();
        plan.sessions.extend(subagents::child_logs(&project, session_id, &mut warnings).into_iter()
            .map(|(id, path)| (subagents::child_id(session_id, &id), path)));
        if !warnings.is_empty() {
            return Err(warnings.join("；"));
        }
    }
    if jsonl_present {
        // 结构校验通过后再复核内容身份：sessionId 必须与文件名一致
        verify_jsonl_session_id(&jsonl, session_id)?;
    }
    Ok(plan)
}

fn verify_jsonl_session_id(path: &Path, session_id: &str) -> Result<(), String> {
    let file = super::open_jsonl(path).map_err(|e| e.to_string())?;
    for line in BufReader::new(file).lines() {
        let line = line.map_err(|e| format!("读取 {:?} 失败：{e}", path))?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let v: serde_json::Map<String, Value> = serde_json::from_str(line).map_err(|_| {
            format!("{:?} 含无法解析的行，无法完成身份复核，已中止", path)
        })?;
        if v.get("sessionId").is_some_and(|id| !id.is_null() && id.as_str() != Some(session_id)) {
            return Err(format!("文件内 sessionId 与待删会话（{session_id}）不一致，已中止"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const QODER_OPTS: FamilyOptions = FamilyOptions {
        use_runtime_model: true,
        filter_non_cli_entrypoint: true,
    };

    const CLAUDE_OPTS: FamilyOptions = FamilyOptions {
        use_runtime_model: false,
        filter_non_cli_entrypoint: false,
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
            let collapsed = catalog(temp.path(), opts, false, &|| false).unwrap();
            let expanded = catalog(temp.path(), opts, true, &|| false).unwrap();
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
                    catalog(temp.path(), opts, include_empty, &|| false).unwrap().projects[0].display_path,
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
        let sessions = sessions_in_dir(temp.path(), dir_name, CLAUDE_OPTS, true, &mut Vec::new(), &|| false).unwrap();
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
            catalog(temp.path(), QODER_OPTS, false, &|| false).unwrap().sessions[0].real_user_count,
            1
        );
        fs::OpenOptions::new()
            .append(true)
            .open(&file)
            .unwrap()
            .write_all(user.as_bytes())
            .unwrap();
        assert_eq!(
            catalog(temp.path(), QODER_OPTS, false, &|| false).unwrap().sessions[0].real_user_count,
            2
        );
        fs::write(&file, "").unwrap();
        assert!(catalog(temp.path(), QODER_OPTS, false, &|| false)
            .unwrap()
            .sessions
            .is_empty());
        assert_eq!(
            catalog(temp.path(), QODER_OPTS, true, &|| false)
                .unwrap()
                .sessions
                .len(),
            1
        );
        fs::write(project.join("bbbb.jsonl"), user).unwrap();
        assert_eq!(
            catalog(temp.path(), QODER_OPTS, false, &|| false).unwrap().sessions[0].session_id,
            "bbbb"
        );
        fs::remove_file(project.join("bbbb.jsonl")).unwrap();
        assert!(catalog(temp.path(), QODER_OPTS, false, &|| false)
            .unwrap()
            .sessions
            .is_empty());
    }

    #[test]
    fn nested_family_transcripts_keep_identity_and_deletion_scope() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        let dir = project.join("aaaa/subagents");
        let worker = dir.join("agent-worker.jsonl");
        let reviewer = dir.join("agent-reviewer.jsonl");
        fs::create_dir_all(&dir).unwrap();
        fs::write(project.join("aaaa.jsonl"), r#"{"type":"user","sessionId":"aaaa","message":{"content":"Main task"}}"#).unwrap();
        fs::write(&worker, r#"{"type":"assistant","sessionId":"aaaa","agentId":"worker","message":{"content":[{"type":"text","text":"Spawn review"},{"type":"tool_use","name":"Agent","id":"call_nested"}]}}"#).unwrap();
        fs::write(&reviewer, r#"{"type":"assistant","sessionId":"aaaa","agentId":"reviewer","message":{"content":"Review"}}"#).unwrap();
        fs::write(dir.join("agent-reviewer.meta.json"), r#"{"spawnDepth":2,"toolUseId":"call_nested"}"#).unwrap();
        for opts in [CLAUDE_OPTS, QODER_OPTS] {
            let catalog = catalog(temp.path(), opts, false, &|| false).unwrap();
            assert_eq!(catalog.projects[0].session_count, 3);
            let detail = read_session(temp.path(), "project", "subagent:aaaa:reviewer", opts).unwrap();
            assert_eq!(detail.meta.subagent.unwrap().parent_id.as_deref(), Some("worker"));
            assert_eq!(detail.events.len(), 1);
        }
        let targets = deletion_targets(temp.path(), "project", "subagent:aaaa:worker").unwrap();
        assert_eq!(targets.len(), 3);
        assert!(targets.contains(&worker.canonicalize().unwrap()));
        assert!(targets.contains(&reviewer.canonicalize().unwrap()));
        assert!(!targets.contains(&project.join("aaaa.jsonl").canonicalize().unwrap()));
        fs::remove_file(project.join("aaaa.jsonl")).unwrap();
        let relation = read_session(temp.path(), "project", "subagent:aaaa:worker", CLAUDE_OPTS).unwrap().meta.subagent.unwrap();
        assert!(relation.orphan);
        assert_eq!(relation.parent_id.as_deref(), Some("aaaa"));
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
            r#"{"type":"user","uuid":"u1","timestamp":"2026-09-03T06:00:00Z","message":{"role":"user","content":"帮我写个排序算法"},"isMeta":false,"cwd":"/tmp/proj","sessionId":"abc","entrypoint":"cli","version":"1.1.40","gitBranch":"main"}"#,
            r#"{"type":"assistant","uuid":"a1","timestamp":"2026-09-03T06:01:00Z","message":{"role":"assistant","model":"mode-xyz","content":[{"type":"text","text":"好的"}]},"cwd":"/tmp/proj","sessionId":"abc","entrypoint":"cli","version":"1.1.41"}"#,
            r#"{"type":"user","uuid":"u2","timestamp":"2026-09-03T06:02:00Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"ok"}]},"isMeta":false,"cwd":"/tmp/proj","sessionId":"abc"}"#,
            r#"{"type":"ai-title","sessionId":"abc","aiTitle":"排序算法实现"}"#,
        ]);
        let meta = meta_of(f.path(), "-tmp-proj", QODER_OPTS);
        assert_eq!(meta.title.as_deref(), Some("排序算法实现"));
        assert_eq!(meta.cwd.as_deref(), Some("/tmp/proj"));
        assert_eq!(meta.model.as_deref(), Some("mode-xyz"));
        assert_eq!(meta.version.as_deref(), Some("1.1.41"));
        assert_eq!(meta.client.as_deref(), Some("cli"));
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
    fn unnamed_title_prefers_first_user_text_over_last_prompt() {
        let f = write_fixture(&[
            r#"{"type":"user","message":{"role":"user","content":"第一条用户消息"},"isMeta":false}"#,
            r#"{"type":"last-prompt","lastPrompt":"最后的提问"}"#,
        ]);
        let meta = meta_of(f.path(), "p", QODER_OPTS);
        assert_eq!(meta.title.as_deref(), Some("第一条用户消息"));

        let f2 = write_fixture(&[
            r#"{"type":"user","message":{"role":"user","content":"<local-command-caveat>忽略</local-command-caveat>"},"isMeta":true}"#,
            r#"{"type":"user","message":{"role":"user","content":"真正的问题"},"isMeta":false}"#,
        ]);
        let meta2 = meta_of(f2.path(), "p", QODER_OPTS);
        assert_eq!(meta2.title.as_deref(), Some("真正的问题"));
    }

    #[test]
    fn qoder_metadata_uses_latest_runtime_selection_and_a_stable_unnamed_title() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("aaaa.jsonl");
        fs::write(
            &path,
            concat!(
                "{\"type\":\"runtime-config\",\"model\":\"mode-old\"}\n",
                "{\"type\":\"user\",\"message\":{\"content\":\"Initial task\"}}\n",
                "{\"type\":\"runtime-config\",\"model\":\"mode-new\"}\n",
                "{\"type\":\"assistant\",\"message\":{\"model\":\"auto\",\"content\":\"Reply\"}}\n",
                "{\"type\":\"last-prompt\",\"lastPrompt\":\"Continue\"}\n"
            ),
        )
        .unwrap();
        let meta = meta_of(&path, "project", QODER_OPTS);
        assert_eq!(meta.model.as_deref(), Some("mode-new"));
        assert_eq!(meta.title.as_deref(), Some("Initial task"));
    }

    #[test]
    fn both_adapters_use_the_latest_valid_assistant_model() {
        let file = write_fixture(&[
            r#"{"type":"assistant","message":{"model":"mode-old","content":"first"}}"#,
            r#"{"type":"assistant","message":{"model":"mode-new","content":"second"}}"#,
            r#"{"type":"assistant","message":{"model":"<synthetic>","content":"summary"}}"#,
            r#"{"type":"runtime-config","model":" "}"#,
        ]);
        assert_eq!(
            meta_of(file.path(), "p", QODER_OPTS).model.as_deref(),
            Some("mode-new")
        );
        assert_eq!(
            meta_of(file.path(), "p", CLAUDE_OPTS).model.as_deref(),
            Some("mode-new")
        );
    }

    #[test]
    fn claude_uses_latest_title_event() {
        let f = write_fixture(&[
            r#"{"type":"ai-title","aiTitle":"初始标题"}"#,
            r#"{"type":"user","message":{"role":"user","content":"Claude 的第一条消息"},"isMeta":false}"#,
            r#"{"type":"ai-title","aiTitle":"最新标题"}"#,
        ]);
        let meta = meta_of(f.path(), "p", CLAUDE_OPTS);
        assert_eq!(meta.title.as_deref(), Some("最新标题"));
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
            "标题应提取 command-args 内的用户文本：{:?}",
            meta.title
        );
    }

    #[test]
    fn strip_noise_tags_removes_noise_keeps_command_args() {
        let input = "<command-message>kit</command-message><command-name>/kit</command-name><command-args>用户问题</command-args>";
        let cleaned = strip_noise_tags(input);
        // command-name 和 command-args 内文保留，command-message 删除
        assert!(cleaned.contains("/kit"), "斜杠命令名应保留：{cleaned}");
        assert!(cleaned.contains("用户问题"), "用户输入应保留：{cleaned}");
        assert!(!cleaned.contains("command-"), "标签本身应被剥离：{cleaned}");
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
    fn filters_non_cli_entrypoints() {
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
        assert!(is_relevant(&meta3, QODER_OPTS));

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

        let err = delete_session(tmp.path(), "-tmp-proj", "aaaa-1111", None).unwrap_err();
        assert!(err.contains("不一致"), "{err}");
        assert!(jsonl.exists(), "中止后文件必须完好");
    }

    #[test]
    fn delete_allows_empty_and_control_only_transcripts() {
        let tmp = tempfile::tempdir().unwrap();
        let proj = tmp.path().join("-tmp-proj");
        fs::create_dir_all(&proj).unwrap();
        for (id, contents) in [("aaaa", ""), ("bbbb", r#"{"type":"queue-operation"}"#)] {
            let jsonl = proj.join(format!("{id}.jsonl"));
            fs::write(&jsonl, contents).unwrap();
            assert!(read_session(tmp.path(), "-tmp-proj", id, CLAUDE_OPTS).unwrap().events.is_empty());
            delete_session_with(tmp.path(), "-tmp-proj", id, |targets| {
                assert_eq!(targets, &[jsonl.canonicalize().unwrap()]);
                fs::remove_file(&targets[0]).map_err(|error| error.to_string())
            }).unwrap();
            assert!(!jsonl.exists());
        }
    }

    #[test]
    fn delete_refuses_corrupt_file_without_session_id() {
        let tmp = tempfile::tempdir().unwrap();
        let proj = tmp.path().join("-tmp-proj");
        fs::create_dir_all(&proj).unwrap();
        let jsonl = proj.join("aaaa-1111.jsonl");
        fs::write(&jsonl, "不是合法 JSON").unwrap();

        let err = delete_session(tmp.path(), "-tmp-proj", "aaaa-1111", None).unwrap_err();
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

        let err = delete_session(tmp.path(), "-tmp-proj", "aaaa-1111", None).unwrap_err();
        assert!(err.contains("符号链接"), "{err}");
        assert!(state_dir.exists(), "不得发生部分删除：状态目录必须完好");
        assert!(jsonl.symlink_metadata().is_ok(), "悬空链接本身也不得被动");
    }
}
