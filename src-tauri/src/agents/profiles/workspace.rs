//! 配置工作区：磁盘状态、格式校验、变更预览与可恢复的操作前备份。
//! 所有写入仍复用 profiles 的事务；预览携带双方版本，确认时重新复核。

use super::*;
use sha2::{Digest, Sha256};
use std::collections::HashSet;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Validation {
    pub valid: bool,
    pub error: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceFile {
    #[serde(flatten)]
    pub file: CurrentFile,
    /// 基于存在性和原始字节；不存在与空文件必须区分。
    pub revision: String,
    pub validation: Validation,
}

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ConfigOverview {
    pub model: Option<String>,
    pub provider: Option<String>,
    pub endpoint: Option<String>,
    pub mcp_servers: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Workspace {
    pub agent: String,
    pub revision: String,
    pub files: Vec<WorkspaceFile>,
    pub profiles: Vec<ProfileSummary>,
    pub matching_profiles: Vec<String>,
    pub overview: ConfigOverview,
    pub backups: Vec<BackupSummary>,
    pub warnings: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct BackupFile {
    key: String,
    /// 备份恢复绑定原路径，避免更换 CODEX_HOME 后误恢复到另一套配置。
    path: String,
    content: Option<String>,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct Backup {
    id: String,
    agent: String,
    created_at: u64,
    label: String,
    files: Vec<BackupFile>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupSummary {
    pub id: String,
    pub created_at: u64,
    pub label: String,
    pub files: Vec<String>,
    pub legacy: bool,
}

#[derive(Deserialize, Serialize, Clone, Copy)]
#[serde(rename_all = "camelCase")]
pub enum ChangeKind {
    Profile,
    Backup,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileChange {
    pub key: String,
    pub label: String,
    pub path: String,
    pub action: &'static str,
    pub before: Option<String>,
    pub after: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangePreview {
    pub kind: ChangeKind,
    pub target: String,
    pub title: String,
    pub current_revision: String,
    pub target_revision: String,
    pub files: Vec<FileChange>,
}

type Slots = Vec<(PathBuf, Option<Vec<u8>>)>;

fn hash_parts(parts: impl IntoIterator<Item = impl AsRef<[u8]>>) -> String {
    let mut hash = Sha256::new();
    for part in parts {
        let bytes = part.as_ref();
        hash.update((bytes.len() as u64).to_le_bytes());
        hash.update(bytes);
    }
    format!("{:x}", hash.finalize())
}

fn file_revision(content: Option<&[u8]>) -> String {
    hash_parts([
        if content.is_some() {
            b"present".as_slice()
        } else {
            b"absent".as_slice()
        },
        content.unwrap_or_default(),
    ])
}

fn slots_revision(slots: &[(PathBuf, Option<Vec<u8>>)]) -> String {
    hash_parts(slots.iter().flat_map(|(path, bytes)| {
        [
            path.to_string_lossy().into_owned(),
            file_revision(bytes.as_deref()),
        ]
    }))
}

fn read_slots(spec: &[ConfigFile]) -> Result<Slots, String> {
    spec.iter()
        .map(|file| Ok((file.path.clone(), read_config_raw(&file.path)?)))
        .collect()
}

pub(super) fn check_revision(spec: &[ConfigFile], expected: &str) -> Result<(), String> {
    if slots_revision(&read_slots(spec)?) != expected {
        return Err("配置文件已被外部修改，请刷新并比较后重试；本次未覆盖磁盘内容".into());
    }
    Ok(())
}

pub(super) fn check_snapshot_revision(
    spec: &[ConfigFile],
    snapshot: &[ConfigSnapshot],
    expected: &str,
) -> Result<(), String> {
    let captured: Slots = spec
        .iter()
        .map(|file| {
            let content = snapshot
                .iter()
                .find(|saved| saved.key == file.key)
                .map(|saved| saved.content.as_bytes().to_vec());
            (file.path.clone(), content)
        })
        .collect();
    if slots_revision(&captured) != expected {
        return Err("采集期间配置发生变化，未保存方案；请刷新后重试".into());
    }
    check_revision(spec, expected)
}

fn position(content: &str, offset: usize) -> (usize, usize) {
    let mut end = offset.min(content.len());
    while !content.is_char_boundary(end) {
        end -= 1;
    }
    let prefix = &content[..end];
    (
        prefix.bytes().filter(|b| *b == b'\n').count() + 1,
        prefix
            .rsplit('\n')
            .next()
            .unwrap_or_default()
            .chars()
            .count()
            + 1,
    )
}

/// 只解释语法，不重写原文；JSONC 仅允许注释和尾逗号，拒绝 JSON5 扩展。
pub(super) fn validate_document(key: &str, content: &str) -> Result<Value, String> {
    // Qoder 新版把默认模型存为带 key/uid 的 JSON，旧版也可能只写模型名。
    if key == ".models/default" {
        let text = content.trim();
        if text.starts_with(['{', '[', '"']) {
            let value: Value = serde_json::from_str(content).map_err(|e| {
                format!(
                    "默认模型 JSON 语法错误，第 {} 行第 {} 列：{e}",
                    e.line(),
                    e.column()
                )
            })?;
            if value.is_object() || value.is_string() {
                return Ok(value);
            }
            return Err("默认模型配置须为 JSON 对象或模型名（第 1 行第 1 列）".into());
        }
        return Ok(Value::String(text.to_string()));
    }
    let value = if key.ends_with(".jsonc") {
        let options = jsonc_parser::ParseOptions {
            allow_comments: true,
            allow_trailing_commas: true,
            allow_loose_object_property_names: false,
            allow_missing_commas: false,
            allow_single_quoted_strings: false,
            allow_hexadecimal_numbers: false,
            allow_unary_plus_numbers: false,
            allow_bare_decimal_point_numbers: false,
            allow_non_finite_numbers: false,
            allow_extended_string_escapes: false,
        };
        jsonc_parser::parse_to_serde_value::<Value>(content, &options).map_err(|e| {
            format!(
                "JSONC 语法错误，第 {} 行第 {} 列：{}",
                e.line_display(),
                e.column_display(),
                e.kind()
            )
        })?
    } else if key.ends_with(".json") {
        serde_json::from_str::<Value>(content)
            .map_err(|e| format!("JSON 语法错误，第 {} 行第 {} 列：{e}", e.line(), e.column()))?
    } else if key.ends_with(".toml") {
        let parsed = toml::from_str::<toml::Value>(content).map_err(|e| {
            let (line, column) = position(content, e.span().map(|s| s.start).unwrap_or(0));
            format!("TOML 语法错误，第 {line} 行第 {column} 列：{}", e.message())
        })?;
        serde_json::to_value(parsed).map_err(|e| format!("无法解析 TOML 配置：{e}"))?
    } else {
        return Ok(Value::String(content.trim().to_string()));
    };
    if !value.is_object() {
        return Err("配置根节点必须为对象（第 1 行第 1 列）".into());
    }
    Ok(value)
}

pub fn validate_content(agent: &str, key: &str, content: &str) -> Result<Validation, String> {
    let spec = super::super::find_source(agent)?.config_spec();
    if !spec.iter().any(|file| file.key == key) {
        return Err("不支持该配置文件".into());
    }
    let error = validate_document(key, content).err();
    Ok(Validation {
        valid: error.is_none(),
        error,
    })
}

fn endpoint_summary(raw: &str) -> String {
    // 摘要去掉 URL 中的用户信息和查询参数；完整原文仍仅在文件编辑区查看。
    let without_query = raw.split(['?', '#']).next().unwrap_or_default();
    match without_query.split_once("://") {
        Some((scheme, rest)) => format!("{scheme}://{}", rest.rsplit('@').next().unwrap_or(rest)),
        None => without_query.to_string(),
    }
}

fn overview(files: &[WorkspaceFile]) -> ConfigOverview {
    let mut result = ConfigOverview::default();
    let mut servers = HashSet::new();
    for file in files {
        let Some(content) = &file.file.content else {
            continue;
        };
        let Ok(value) = validate_document(&file.file.key, content) else {
            continue;
        };
        if file.file.key == ".models/default" && !content.trim().is_empty() {
            if let Some(model) = value
                .get("key")
                .and_then(Value::as_str)
                .or_else(|| value.as_str())
                .filter(|model| !model.trim().is_empty())
            {
                result.model = Some(model.to_string());
            }
        }
        let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_string);
        let env = value.get("env");
        result.model = result.model.or_else(|| text("model")).or_else(|| {
            env.and_then(|v| v.get("ANTHROPIC_MODEL"))
                .and_then(Value::as_str)
                .map(str::to_string)
        });
        result.provider = result.provider.or_else(|| text("model_provider"));
        let endpoint = result
            .provider
            .as_ref()
            .and_then(|provider| value.get("model_providers").and_then(|p| p.get(provider)))
            .and_then(|p| p.get("base_url"))
            .and_then(Value::as_str)
            .or_else(|| value.get("openai_base_url").and_then(Value::as_str))
            .or_else(|| {
                env.and_then(|v| v.get("ANTHROPIC_BASE_URL"))
                    .and_then(Value::as_str)
            });
        if result.endpoint.is_none() {
            result.endpoint = endpoint.map(endpoint_summary);
        }
        for key in ["mcp_servers", "mcpServers", "mcp"] {
            if let Some(map) = value.get(key).and_then(Value::as_object) {
                servers.extend(map.keys().cloned());
            }
        }
    }
    if result.provider.is_none() {
        result.provider = result
            .model
            .as_ref()
            .and_then(|m| m.split_once('/').map(|(p, _)| p.to_string()));
    }
    result.mcp_servers = servers.into_iter().collect();
    result.mcp_servers.sort();
    result
}

fn backup_dir() -> Result<PathBuf, String> {
    let path = ensure_store_dir()?.join("config-history");
    let mut builder = fs::DirBuilder::new();
    builder.mode(0o700);
    match builder.create(&path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(format!("创建配置备份目录失败：{e}")),
    }
    let meta = path
        .symlink_metadata()
        .map_err(|e| format!("无法读取备份目录：{e}"))?;
    if !meta.is_dir() || meta.file_type().is_symlink() {
        return Err("备份目录必须是真实目录".into());
    }
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?;
    Ok(path)
}

fn validate_backup(backup: &Backup, agent: &str, spec: &[ConfigFile]) -> Result<(), String> {
    if backup.agent != agent || backup.files.is_empty() {
        return Err("备份不属于当前 Agent 或没有文件".into());
    }
    let mut seen = HashSet::new();
    for file in &backup.files {
        let entry = spec
            .iter()
            .find(|c| c.key == file.key)
            .ok_or("备份包含未知配置文件")?;
        if !seen.insert(&file.key) || file.path != entry.path.to_string_lossy() {
            return Err("备份的配置路径已变化或文件重复，已拒绝恢复".into());
        }
    }
    Ok(())
}

fn backup_summary(backup: &Backup) -> BackupSummary {
    BackupSummary {
        id: backup.id.clone(),
        created_at: backup.created_at,
        label: backup.label.clone(),
        files: backup.files.iter().map(|file| file.key.clone()).collect(),
        legacy: backup.id.starts_with("legacy-"),
    }
}

/// 兼容旧版散落在配置文件旁的 .bak；只枚举当前适配器声明的目录和文件名。
fn legacy_backups(agent: &str, spec: &[ConfigFile], warnings: &mut Vec<String>) -> Vec<Backup> {
    let mut result = Vec::new();
    for file in spec {
        let (Some(parent), Some(name)) = (file.path.parent(), file.path.file_name()) else {
            continue;
        };
        let prefix = format!("{}.bak-", name.to_string_lossy());
        let entries = match fs::read_dir(parent) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                warnings.push(format!("无法读取 {} 的旧版备份：{e}", file.key));
                continue;
            }
        };
        for entry in entries {
            let Ok(entry) = entry else {
                warnings.push(format!("读取 {} 的备份目录项失败", file.key));
                continue;
            };
            let filename = entry.file_name().to_string_lossy().into_owned();
            let Some(suffix) = filename.strip_prefix(&prefix) else {
                continue;
            };
            let Some(timestamp) = suffix.split('-').next().and_then(|s| s.parse::<u64>().ok())
            else {
                continue;
            };
            match read_store_file(&entry.path()) {
                Ok(Some(content)) => result.push(Backup {
                    id: format!(
                        "legacy-{}",
                        hash_parts([file.key.as_str(), filename.as_str()])
                    ),
                    agent: agent.to_string(),
                    created_at: timestamp,
                    label: format!("旧版单文件备份 · {}", file.key),
                    files: vec![BackupFile {
                        key: file.key.clone(),
                        path: file.path.to_string_lossy().into_owned(),
                        content: Some(content),
                    }],
                }),
                Ok(None) => {}
                Err(e) => warnings.push(e),
            }
        }
    }
    result
}

fn load_backups(
    dir: &Path,
    agent: &str,
    spec: &[ConfigFile],
    warnings: &mut Vec<String>,
) -> Vec<Backup> {
    let mut result = Vec::new();
    for entry in super::super::read_dir_warned(dir, warnings) {
        let filename = entry.file_name().to_string_lossy().into_owned();
        if !filename.starts_with("history-") || !filename.ends_with(".json") {
            continue;
        }
        let loaded = (|| {
            let raw = read_store_file(&entry.path())?.ok_or("备份文件已不存在")?;
            let backup: Backup = serde_json::from_str(&raw).map_err(|_| "配置备份记录损坏")?;
            if backup.id != filename {
                return Err("配置备份身份不匹配".into());
            }
            Ok::<_, String>(backup)
        })();
        match loaded {
            Ok(backup) if backup.agent == agent => match validate_backup(&backup, agent, spec) {
                Ok(()) => result.push(backup),
                Err(e) => warnings.push(e),
            },
            Ok(_) => {}
            Err(e) => warnings.push(format!("无法读取配置备份 {filename}：{e}")),
        }
    }
    // 新事务同时保留 .bak 兼容文件，列表中由整组备份代表，避免重复呈现。
    let standalone: Vec<_> = legacy_backups(agent, spec, warnings)
        .into_iter()
        .filter(|legacy| {
            !result.iter().any(|group| {
                group.created_at.abs_diff(legacy.created_at) < 10_000
                    && legacy.files.iter().all(|old| {
                        group.files.iter().any(|file| {
                            file.key == old.key
                                && file.path == old.path
                                && file.content == old.content
                        })
                    })
            })
        })
        .collect();
    result.extend(standalone);
    result.sort_by(|a, b| {
        b.created_at
            .cmp(&a.created_at)
            .then_with(|| b.id.cmp(&a.id))
    });
    result
}

fn workspace_in(
    agent: &str,
    spec: &[ConfigFile],
    profiles: &[Profile],
    history: &Path,
) -> Result<Workspace, String> {
    transaction::recover_in(history.parent().ok_or("缺少配置存储目录")?, spec)?;
    let slots = read_slots(spec)?;
    let revision = slots_revision(&slots);
    let mut files = Vec::new();
    for (entry, (_, bytes)) in spec.iter().zip(&slots) {
        let content = bytes
            .as_ref()
            .map(|raw| bytes_to_text(&entry.path, raw.clone()))
            .transpose()?;
        let error = content
            .as_ref()
            .and_then(|text| validate_document(&entry.key, text).err());
        files.push(WorkspaceFile {
            file: CurrentFile {
                key: entry.key.clone(),
                label: entry.label.clone(),
                path: entry.path.to_string_lossy().into_owned(),
                present: content.is_some(),
                content,
            },
            revision: file_revision(bytes.as_deref()),
            validation: Validation {
                valid: error.is_none(),
                error,
            },
        });
    }
    let mut matching_profiles = Vec::new();
    let mut summaries = Vec::new();
    for profile in profiles.iter().filter(|p| p.agent == agent) {
        if profile_slots(profile, spec).is_ok_and(|target| target == slots) {
            matching_profiles.push(profile.name.clone());
        }
        summaries.push(summarize(profile, spec));
    }
    let mut warnings = Vec::new();
    let backups = load_backups(history, agent, spec, &mut warnings)
        .iter()
        .take(100)
        .map(backup_summary)
        .collect();
    let overview = overview(&files);
    Ok(Workspace {
        agent: agent.to_string(),
        revision,
        files,
        profiles: summaries,
        matching_profiles,
        overview,
        backups,
        warnings,
    })
}

pub fn get_workspace(agent: &str) -> Result<Workspace, String> {
    let _guard = store_lock()?;
    let spec = super::super::find_source(agent)?.config_spec();
    workspace_in(agent, &spec, &load_store()?.profiles, &backup_dir()?)
}

fn target_slots(
    agent: &str,
    spec: &[ConfigFile],
    profiles: &[Profile],
    history: &Path,
    kind: ChangeKind,
    target: &str,
) -> Result<(String, Slots), String> {
    let (title, slots) = match kind {
        ChangeKind::Profile => {
            let profile = profiles
                .iter()
                .find(|p| p.agent == agent && p.name == target)
                .ok_or("未找到配置方案")?;
            (
                format!("应用配置「{}」", profile.name),
                profile_slots(profile, spec)?,
            )
        }
        ChangeKind::Backup => {
            let mut warnings = Vec::new();
            let backup = load_backups(history, agent, spec, &mut warnings)
                .into_iter()
                .find(|b| b.id == target)
                .ok_or("未找到可恢复的备份，请刷新后重试")?;
            validate_backup(&backup, agent, spec)?;
            let slots = backup
                .files
                .iter()
                .map(|f| {
                    (
                        PathBuf::from(&f.path),
                        f.content.as_ref().map(|c| c.as_bytes().to_vec()),
                    )
                })
                .collect();
            (format!("恢复备份 · {}", backup.label), slots)
        }
    };
    for (path, bytes) in &slots {
        if let Some(bytes) = bytes {
            let entry = spec
                .iter()
                .find(|c| &c.path == path)
                .ok_or("配置目标不在受管理范围内")?;
            validate_document(&entry.key, &bytes_to_text(path, bytes.clone())?)?;
        }
    }
    Ok((title, slots))
}

fn preview_in(
    agent: &str,
    spec: &[ConfigFile],
    profiles: &[Profile],
    history: &Path,
    kind: ChangeKind,
    target: &str,
) -> Result<ChangePreview, String> {
    transaction::recover_in(history.parent().ok_or("缺少配置存储目录")?, spec)?;
    let current = read_slots(spec)?;
    let (title, slots) = target_slots(agent, spec, profiles, history, kind, target)?;
    let mut files = Vec::new();
    for (path, after) in &slots {
        let entry = spec
            .iter()
            .find(|c| c.path == *path)
            .ok_or("配置文件位置已变化")?;
        let before = &current
            .iter()
            .find(|(p, _)| p == path)
            .ok_or("配置文件位置已变化")?
            .1;
        let action = match (before.as_ref(), after.as_ref()) {
            _ if before == after => "unchanged",
            (None, Some(_)) => "create",
            (Some(_), None) => "remove",
            _ => "replace",
        };
        files.push(FileChange {
            key: entry.key.clone(),
            label: entry.label.clone(),
            path: path.to_string_lossy().into_owned(),
            action,
            before: before
                .as_ref()
                .map(|b| bytes_to_text(path, b.clone()))
                .transpose()?,
            after: after
                .as_ref()
                .map(|b| bytes_to_text(path, b.clone()))
                .transpose()?,
        });
    }
    Ok(ChangePreview {
        kind,
        target: target.to_string(),
        title,
        current_revision: slots_revision(&current),
        target_revision: slots_revision(&slots),
        files,
    })
}

pub fn preview_change(
    agent: &str,
    kind: ChangeKind,
    target: &str,
) -> Result<ChangePreview, String> {
    let _guard = store_lock()?;
    let spec = super::super::find_source(agent)?.config_spec();
    preview_in(
        agent,
        &spec,
        &load_store()?.profiles,
        &backup_dir()?,
        kind,
        target,
    )
}

fn record_backup(
    agent: &str,
    spec: &[ConfigFile],
    slots: &[(PathBuf, Option<Vec<u8>>)],
    history: &Path,
    label: &str,
) -> Result<(), String> {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let created_at = now_millis();
    let mut files = Vec::new();
    for (path, _) in slots {
        let entry = spec
            .iter()
            .find(|c| &c.path == path)
            .ok_or("未知配置文件")?;
        let content = read_config_raw(path)?
            .map(|bytes| bytes_to_text(path, bytes))
            .transpose()?;
        files.push(BackupFile {
            key: entry.key.clone(),
            path: path.to_string_lossy().into_owned(),
            content,
        });
    }
    // 保留操作前的整组内容；失败尝试的备份也保留，便于回滚不完整时人工恢复。
    let id = format!(
        "history-{created_at}-{}-{}.json",
        std::process::id(),
        SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    let backup = Backup {
        id: id.clone(),
        agent: agent.to_string(),
        created_at,
        label: label.to_string(),
        files,
    };
    atomic_write(
        &history.join(id),
        &serde_json::to_vec(&backup).map_err(|e| e.to_string())?,
    )
}

fn apply_change_in(
    agent: &str,
    spec: &[ConfigFile],
    profiles: &[Profile],
    history: &Path,
    kind: ChangeKind,
    target: &str,
    expected_current: &str,
    expected_target: &str,
) -> Result<(), String> {
    transaction::recover_in(history.parent().ok_or("缺少配置存储目录")?, spec)?;
    check_revision(spec, expected_current)?;
    let (title, slots) = target_slots(agent, spec, profiles, history, kind, target)?;
    if slots_revision(&slots) != expected_target {
        return Err("目标配置已变化，请重新查看差异后再确认".into());
    }
    let current = read_slots(spec)?;
    if slots
        .iter()
        .all(|(p, c)| current.iter().any(|(path, bytes)| p == path && c == bytes))
    {
        return Ok(());
    }
    record_backup(agent, spec, &slots, history, &title)?;
    // 备份期间 CLI 也可能修改文件：写入前再复核一次，冲突时保留备份但不覆盖。
    check_revision(spec, expected_current)?;
    if slots_revision(&current) != expected_current {
        return Err("配置文件已被外部修改，请刷新并比较后重试".into());
    }
    apply_snapshot_in(
        history.parent().ok_or("缺少配置存储目录")?,
        spec,
        &slots,
        Some(&current),
    )
}

pub fn apply_change(
    agent: &str,
    kind: ChangeKind,
    target: &str,
    expected_current: &str,
    expected_target: &str,
) -> Result<(), String> {
    let _guard = store_lock()?;
    let spec = super::super::find_source(agent)?.config_spec();
    apply_change_in(
        agent,
        &spec,
        &load_store()?.profiles,
        &backup_dir()?,
        kind,
        target,
        expected_current,
        expected_target,
    )
}

fn update_in(
    agent: &str,
    spec: &[ConfigFile],
    history: &Path,
    key: &str,
    content: &str,
    expected: &str,
) -> Result<(), String> {
    transaction::recover_in(history.parent().ok_or("缺少配置存储目录")?, spec)?;
    let entry = spec
        .iter()
        .find(|file| file.key == key)
        .ok_or("不支持该配置文件")?;
    validate_document(key, content)?;
    let before = read_config_raw(&entry.path)?;
    if file_revision(before.as_deref()) != expected {
        return Err("此文件已被外部修改，请保留草稿，刷新并比较后重试；本次未覆盖磁盘内容".into());
    }
    if before.as_deref() == Some(content.as_bytes()) {
        return Ok(());
    }
    let slots = vec![(entry.path.clone(), Some(content.as_bytes().to_vec()))];
    record_backup(
        agent,
        spec,
        &slots,
        history,
        &format!("编辑 {}", entry.label),
    )?;
    if file_revision(read_config_raw(&entry.path)?.as_deref()) != expected {
        return Err("备份期间文件已被外部修改，本次未覆盖磁盘内容".into());
    }
    apply_snapshot_in(
        history.parent().ok_or("缺少配置存储目录")?,
        spec,
        &slots,
        Some(&[(entry.path.clone(), before)]),
    )
}

pub fn update_file(agent: &str, key: &str, content: &str, expected: &str) -> Result<(), String> {
    let _guard = store_lock()?;
    let spec = super::super::find_source(agent)?.config_spec();
    update_in(agent, &spec, &backup_dir()?, key, content, expected)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(root: &Path) -> Vec<ConfigFile> {
        vec![
            ConfigFile {
                key: "settings.json".into(),
                label: "settings.json".into(),
                path: root.join("settings.json"),
            },
            ConfigFile {
                key: "mcp.json".into(),
                label: "mcp.json".into(),
                path: root.join("mcp.json"),
            },
        ]
    }

    #[test]
    fn validates_formats_without_accepting_json5_extensions() {
        for (key, content) in [
            ("a.json", "{\"a\":1}"),
            ("a.jsonc", "// comment\n{\"a\":1,}"),
            ("config.toml", "# comment\nmodel = \"demo\"\n"),
        ] {
            assert!(validate_document(key, content).is_ok(), "{key}");
        }
        for (key, content) in [
            ("a.json", "{broken}"),
            ("a.jsonc", "{a:1}"),
            ("a.jsonc", "{\"a\":1 \"b\":2}"),
            ("a.jsonc", "{'a':1}"),
            ("config.toml", "model = ["),
            ("a.json", "[]"),
        ] {
            let error = validate_document(key, content).unwrap_err();
            assert!(error.contains("行") && error.contains("列"), "{error}");
        }
        assert_ne!(file_revision(None), file_revision(Some(b"")));
    }

    #[test]
    fn update_detects_external_edits_and_restores_before_creation() {
        let tmp = tempfile::tempdir().unwrap();
        let history = tmp.path().join("history");
        fs::create_dir(&history).unwrap();
        let spec = spec(tmp.path());
        let empty_revision = file_revision(None);
        update_in(
            "test",
            &spec,
            &history,
            "settings.json",
            "{\"model\":\"new\"}",
            &empty_revision,
        )
        .unwrap();
        let before_external = file_revision(Some(b"{\"model\":\"new\"}"));
        fs::write(&spec[0].path, "{\"external\":true}").unwrap();
        let err = update_in(
            "test",
            &spec,
            &history,
            "settings.json",
            "{}",
            &before_external,
        )
        .unwrap_err();
        assert!(err.contains("外部修改"));
        assert_eq!(
            fs::read_to_string(&spec[0].path).unwrap(),
            "{\"external\":true}"
        );
        let mut warnings = Vec::new();
        let backups = load_backups(&history, "test", &spec, &mut warnings);
        assert_eq!(backups.len(), 1);
        assert_eq!(backups[0].files[0].content, None);
        let preview = preview_in(
            "test",
            &spec,
            &[],
            &history,
            ChangeKind::Backup,
            &backups[0].id,
        )
        .unwrap();
        assert_eq!(preview.files[0].action, "remove");
        // 删除原先不存在的配置文件走注入的事务，不操作真实回收站。
        let (_, slots) = target_slots(
            "test",
            &spec,
            &[],
            &history,
            ChangeKind::Backup,
            &backups[0].id,
        )
        .unwrap();
        apply_snapshot_with(&slots, write_config_file, |path| {
            fs::remove_file(path).map_err(|e| e.to_string())
        })
        .unwrap();
        assert!(!spec[0].path.exists());
    }

    #[test]
    fn preview_apply_and_group_restore_preserve_original_bytes() {
        let tmp = tempfile::tempdir().unwrap();
        let history = tmp.path().join("history");
        fs::create_dir(&history).unwrap();
        let spec = spec(tmp.path());
        fs::write(&spec[0].path, "{ \"model\": \"old\" }\n").unwrap();
        fs::write(&spec[1].path, "{\"mcpServers\":{}}\n").unwrap();
        let profiles = vec![Profile {
            name: "work".into(),
            agent: "test".into(),
            created_at: 1,
            files: vec![
                ConfigSnapshot {
                    key: "settings.json".into(),
                    content: "{\"model\":\"new\"}".into(),
                },
                ConfigSnapshot {
                    key: "mcp.json".into(),
                    content: "{}".into(),
                },
            ],
            captured_keys: Some(spec.iter().map(|f| f.key.clone()).collect()),
        }];
        let preview = preview_in(
            "test",
            &spec,
            &profiles,
            &history,
            ChangeKind::Profile,
            "work",
        )
        .unwrap();
        assert_eq!(
            preview
                .files
                .iter()
                .filter(|f| f.action == "replace")
                .count(),
            2
        );
        apply_change_in(
            "test",
            &spec,
            &profiles,
            &history,
            ChangeKind::Profile,
            "work",
            &preview.current_revision,
            &preview.target_revision,
        )
        .unwrap();
        let workspace = workspace_in("test", &spec, &profiles, &history).unwrap();
        assert_eq!(workspace.matching_profiles, ["work"]);
        let backup = workspace.backups.iter().find(|b| !b.legacy).unwrap();
        let restore = preview_in(
            "test",
            &spec,
            &profiles,
            &history,
            ChangeKind::Backup,
            &backup.id,
        )
        .unwrap();
        apply_change_in(
            "test",
            &spec,
            &profiles,
            &history,
            ChangeKind::Backup,
            &backup.id,
            &restore.current_revision,
            &restore.target_revision,
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(&spec[0].path).unwrap(),
            "{ \"model\": \"old\" }\n"
        );
        assert_eq!(
            fs::read_to_string(&spec[1].path).unwrap(),
            "{\"mcpServers\":{}}\n"
        );
        let err = apply_change_in(
            "test",
            &spec,
            &profiles,
            &history,
            ChangeKind::Profile,
            "work",
            &preview.current_revision,
            "changed-target",
        )
        .unwrap_err();
        assert!(err.contains("目标配置已变化"));
    }

    #[test]
    fn workspace_access_recovers_before_reading_and_returns_conflicts() {
        for conflict in [false, true] {
            let tmp = tempfile::tempdir().unwrap();
            let history = tmp.path().join("history");
            fs::create_dir(&history).unwrap();
            let spec = spec(tmp.path());
            fs::write(&spec[0].path, "{\"model\":\"old\"}").unwrap();
            let profile = Profile {
                name: "new".into(),
                agent: "test".into(),
                created_at: 1,
                files: vec![ConfigSnapshot {
                    key: "settings.json".into(),
                    content: "{\"model\":\"new\"}".into(),
                }],
                captured_keys: Some(spec.iter().map(|f| f.key.clone()).collect()),
            };
            let preview = preview_in(
                "test",
                &spec,
                &[profile.clone()],
                &history,
                ChangeKind::Profile,
                "new",
            )
            .unwrap();
            let stopped = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                transaction::with_hook(
                    |event| {
                        if event == transaction::Event::BeforeCommit {
                            panic!("interrupted workspace apply");
                        }
                        Ok(())
                    },
                    || {
                        apply_change_in(
                            "test",
                            &spec,
                            &[profile],
                            &history,
                            ChangeKind::Profile,
                            "new",
                            &preview.current_revision,
                            &preview.target_revision,
                        )
                        .unwrap()
                    },
                )
            }));
            assert!(stopped.is_err());
            if conflict {
                fs::write(&spec[0].path, "{\"model\":\"external\"}").unwrap();
            }
            let result = workspace_in("test", &spec, &[], &history);
            if conflict {
                assert!(result.err().unwrap().contains("冲突"));
                assert_eq!(
                    fs::read_to_string(&spec[0].path).unwrap(),
                    "{\"model\":\"external\"}"
                );
            } else {
                assert_eq!(result.unwrap().overview.model.as_deref(), Some("old"));
                assert_eq!(
                    workspace_in("test", &spec, &[], &history).unwrap().revision,
                    preview.current_revision
                );
            }
        }
    }

    #[test]
    fn invalid_profile_and_stale_preview_never_write() {
        let tmp = tempfile::tempdir().unwrap();
        let history = tmp.path().join("history");
        fs::create_dir(&history).unwrap();
        let spec = spec(tmp.path());
        fs::write(&spec[0].path, "{}").unwrap();
        let mut profile = Profile {
            name: "work".into(),
            agent: "test".into(),
            created_at: 1,
            files: vec![ConfigSnapshot {
                key: "settings.json".into(),
                content: "{broken}".into(),
            }],
            captured_keys: Some(spec.iter().map(|f| f.key.clone()).collect()),
        };
        assert!(preview_in(
            "test",
            &spec,
            &[profile.clone()],
            &history,
            ChangeKind::Profile,
            "work"
        )
        .err()
        .unwrap()
        .contains("JSON"));
        profile.files[0].content = "{\"a\":1}".into();
        let preview = preview_in(
            "test",
            &spec,
            &[profile.clone()],
            &history,
            ChangeKind::Profile,
            "work",
        )
        .unwrap();
        fs::write(&spec[0].path, "{\"external\":true}").unwrap();
        assert!(apply_change_in(
            "test",
            &spec,
            &[profile],
            &history,
            ChangeKind::Profile,
            "work",
            &preview.current_revision,
            &preview.target_revision
        )
        .unwrap_err()
        .contains("外部修改"));
        assert_eq!(
            fs::read_to_string(&spec[0].path).unwrap(),
            "{\"external\":true}"
        );
        assert_eq!(fs::read_dir(&history).unwrap().count(), 0);
    }

    #[test]
    fn backups_reject_changed_paths_and_links_and_support_legacy() {
        let tmp = tempfile::tempdir().unwrap();
        let history = tmp.path().join("history");
        fs::create_dir(&history).unwrap();
        let spec = spec(tmp.path());
        let legacy = tmp.path().join("settings.json.bak-1234-1");
        fs::write(&legacy, "{}").unwrap();
        std::os::unix::fs::symlink(&legacy, tmp.path().join("mcp.json.bak-1235-1")).unwrap();
        let mut warnings = Vec::new();
        let backups = load_backups(&history, "test", &spec, &mut warnings);
        assert_eq!(backups.len(), 1);
        assert!(!warnings.is_empty());
        let mut invalid = backups[0].clone();
        invalid.files[0].path = "/tmp/another-home/settings.json".into();
        assert!(validate_backup(&invalid, "test", &spec).is_err());
        assert!(target_slots(
            "test",
            &spec,
            &[],
            &history,
            ChangeKind::Backup,
            "../../outside"
        )
        .is_err());
        let info = fs::metadata(&legacy).unwrap();
        assert!(info.is_file());
    }

    #[test]
    fn snapshot_capture_must_match_both_displayed_version_and_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let spec = spec(tmp.path());
        fs::write(&spec[0].path, "{}").unwrap();
        let expected = slots_revision(&read_slots(&spec).unwrap());
        let mut snapshot = vec![ConfigSnapshot {
            key: "settings.json".into(),
            content: "{}".into(),
        }];
        check_snapshot_revision(&spec, &snapshot, &expected).unwrap();
        snapshot[0].content = "{\"mixed\":true}".into();
        assert!(check_snapshot_revision(&spec, &snapshot, &expected)
            .unwrap_err()
            .contains("采集期间"));
        snapshot[0].content = "{}".into();
        fs::write(&spec[0].path, "{\"external\":true}").unwrap();
        assert!(check_snapshot_revision(&spec, &snapshot, &expected)
            .unwrap_err()
            .contains("外部修改"));
    }

    #[test]
    fn editing_toml_and_jsonc_preserves_comments_and_rejects_invalid_writes() {
        let tmp = tempfile::tempdir().unwrap();
        let history = tmp.path().join("history");
        fs::create_dir(&history).unwrap();
        let spec: Vec<_> = ["config.toml", "opencode.jsonc"]
            .into_iter()
            .map(|key| ConfigFile {
                key: key.into(),
                label: key.into(),
                path: tmp.path().join(key),
            })
            .collect();
        let toml = "# retained\nmodel=\"demo\"  # inline\nmodel_provider=\"company\"\n[model_providers.company]\nbase_url=\"https://user:fake@api.example.invalid/v1?token=fake\"\n[mcp_servers.docs]\ncommand=\"docs\"\n";
        let jsonc = "{\n  // retained\n  \"mcp\": {\"files\": {}},\n}\n";
        for (file, content) in spec.iter().zip([toml, jsonc]) {
            update_in(
                "test",
                &spec,
                &history,
                &file.key,
                content,
                &file_revision(None),
            )
            .unwrap();
            assert_eq!(fs::read_to_string(&file.path).unwrap(), content);
            let invalid = if file.key.ends_with(".toml") {
                "model = ["
            } else {
                "{\"missing\":1 \"comma\":2}"
            };
            assert!(update_in(
                "test",
                &spec,
                &history,
                &file.key,
                invalid,
                &file_revision(Some(content.as_bytes()))
            )
            .is_err());
            assert_eq!(fs::read_to_string(&file.path).unwrap(), content);
        }
        let workspace = workspace_in("test", &spec, &[], &history).unwrap();
        assert_eq!(workspace.overview.model.as_deref(), Some("demo"));
        assert_eq!(
            workspace.overview.endpoint.as_deref(),
            Some("https://api.example.invalid/v1")
        );
        assert_eq!(workspace.overview.mcp_servers, ["docs", "files"]);
        for backup in fs::read_dir(&history).unwrap() {
            assert_eq!(
                backup.unwrap().metadata().unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn qoder_default_model_reads_key_instead_of_internal_metadata() {
        let tmp = tempfile::tempdir().unwrap();
        let history = tmp.path().join("history");
        fs::create_dir(&history).unwrap();
        let spec = vec![ConfigFile {
            key: ".models/default".into(),
            label: "默认模型".into(),
            path: tmp.path().join("default"),
        }];
        fs::write(
            &spec[0].path,
            r#"{"key":"auto","uid":"internal-id","scene":"assistant","updatedAt":1}"#,
        )
        .unwrap();
        assert_eq!(
            workspace_in("qoder", &spec, &[], &history)
                .unwrap()
                .overview
                .model
                .as_deref(),
            Some("auto")
        );
        fs::write(&spec[0].path, "legacy-model\n").unwrap();
        assert_eq!(
            workspace_in("qoder", &spec, &[], &history)
                .unwrap()
                .overview
                .model
                .as_deref(),
            Some("legacy-model")
        );
        assert!(validate_document(".models/default", r#"{"key":"#)
            .unwrap_err()
            .contains("JSON"));
        assert!(validate_document(".models/default", "[]").is_err());
    }
}
