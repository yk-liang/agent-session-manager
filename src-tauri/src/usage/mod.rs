//! 本地 token 统计。解析/去重规则参考 ccusage，运行时不依赖其程序或定价服务。
mod cache;
#[cfg(test)]
mod cold_start_tests;
pub mod collector;
#[cfg(test)]
mod event_tests;
pub mod events;
mod ingest;
mod ledger;
mod normalize;
mod opencode;
mod parse;
mod request_parse;
pub mod service;
pub mod summary;
#[cfg(test)]
mod summary_tests;
#[cfg(test)]
mod tests;
mod types;

pub(crate) use cache::Store;
#[cfg(test)]
use chrono::{Local, TimeZone, Utc};
use sha2::{Digest, Sha256};
#[cfg(test)]
use std::{collections::BTreeMap, time::Instant};
use std::{
    collections::{BTreeSet, HashSet},
    fs,
    path::PathBuf,
    sync::{Mutex, OnceLock},
};
#[cfg(test)]
use types::{RecordedFields, Tokens, UsageReport, UsageRow};

pub const AGENTS: [&str; 5] = ["qoder", "claude", "codex", "opencode", "pi"];

#[cfg(test)]
struct ReportSnapshot {
    scope: String,
    revision: cache::StoreRevision,
    timezone: String,
    sources: BTreeMap<PathBuf, Vec<cache::Stamp>>,
    readable: bool,
    report: UsageReport,
}

#[cfg(test)]
fn timezone_key() -> String {
    let tz = std::env::var_os("TZ");
    let tzdir = std::env::var_os("TZDIR");
    let mut digest = Sha256::new();
    digest.update(format!("{tz:?}:{tzdir:?}:{}", Local::now().offset()));
    let mut paths = vec![PathBuf::from("/etc/localtime")];
    if let Some(tz) = tz.as_ref().and_then(|v| v.to_str()) {
        let tz = tz.trim_start_matches(':');
        paths.push(PathBuf::from(tz));
        for dir in ["/usr/share/zoneinfo", "/share/zoneinfo", "/etc/zoneinfo"] {
            paths.push(PathBuf::from(dir).join(tz));
        }
        if let Some(dir) = tzdir {
            paths.push(PathBuf::from(dir).join(tz));
        }
    }
    for path in paths {
        if let Ok(data) = fs::read(&path) {
            digest.update(data);
        }
    }
    format!("{:x}", digest.finalize())
}

#[cfg(test)]
fn unchanged_sources(
    agent: &str,
    roots: &[PathBuf],
    sources: &BTreeMap<PathBuf, Vec<cache::Stamp>>,
    cancel: &dyn Fn() -> bool,
    strict: bool,
) -> Result<bool, CollectionError> {
    let mut warnings = Vec::new();
    let paths = discover(agent, roots, &mut warnings, cancel, strict)?;
    if !warnings.is_empty() || paths.len() != sources.len() {
        return Ok(false);
    }
    for path in paths {
        if cancel() {
            return Err("统计已取消".into());
        }
        let stamps = if agent == "opencode" {
            opencode::stamps(&path)
        } else {
            cache::stamp(&path).map(|s| vec![s])
        };
        match stamps {
            Ok(stamps) if sources.get(&path) == Some(&stamps) => {}
            Err(e) if strict => return Err(CollectionError::source(&path, e)),
            _ => return Ok(false),
        }
    }
    Ok(true)
}

pub fn roots(agent: &str) -> Result<Vec<PathBuf>, String> {
    if !AGENTS.contains(&agent) {
        return Err("不支持的用量来源".into());
    }
    let root = crate::agents::find_source(agent)?
        .data_dir()
        .ok_or("缺少数据目录")?;
    Ok(match agent {
        "qoder" | "claude" => vec![root.join("projects")],
        "codex" => vec![root.join("sessions"), root.join("archived_sessions")],
        "pi" => vec![root.join("sessions")],
        _ => vec![root],
    })
}
fn scope(agent: &str, roots: &[PathBuf]) -> String {
    // 解析语义升级时变更版本；环境变量切换数据根后不复用另一份数据的索引。
    format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(&(
                2,
                if agent == "claude" {
                    3
                } else if agent == "codex" {
                    4
                } else {
                    1
                },
                agent,
                roots
            ))
            .unwrap_or_default()
        )
    )
}
fn store(agent: &str) -> &'static Mutex<Store> {
    static STORES: [OnceLock<Mutex<Store>>; AGENTS.len()] =
        [const { OnceLock::new() }; AGENTS.len()];
    let index = AGENTS
        .iter()
        .position(|id| *id == agent)
        .expect("Validated usage agent");
    STORES[index].get_or_init(|| {
        let path = (!cfg!(test))
            .then(|| {
                dirs::cache_dir()
                    .map(|p| p.join("com.liangyukun.agent-session-manager/token-usage-v1.sqlite3"))
            })
            .flatten();
        let ledger_path = (!cfg!(test))
            .then(|| {
                dirs::data_dir().map(|p| p.join("Agent Session Manager/usage-ledger-v1.sqlite3"))
            })
            .flatten();
        Mutex::new(Store::open_with_ledger(
            path.as_deref(),
            ledger_path.as_deref(),
        ))
    })
}

/// 删除动作必须晚于账本的持久化提交；失败时保留会话，不能静默丢掉最后一轮用量。
pub fn preserve_before_delete(
    agent: &str,
    delete: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    let roots = roots(agent)?;
    let _writer = ledger::SyncLock::acquire(&ledger::path()?, &|| false)?;
    let mut store = store(agent).lock().unwrap_or_else(|e| e.into_inner());
    preserve_before_delete_at(agent, &roots, &mut store, delete)
}

fn preserve_before_delete_at(
    agent: &str,
    roots: &[PathBuf],
    store: &mut Store,
    delete: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    summary::sync_at(agent, roots, store, &|| false)
        .map_err(|e| format!("删除前未能保存累计用量，会话已保留：{e}"))?;
    delete()
}

#[derive(Debug)]
enum CollectionError {
    Source { path: PathBuf, cause: String },
    Other(String),
}
impl CollectionError {
    fn source(path: &std::path::Path, cause: impl ToString) -> Self {
        Self::Source {
            path: path.into(),
            cause: cause.to_string(),
        }
    }
}
impl From<String> for CollectionError {
    fn from(error: String) -> Self {
        Self::Other(error)
    }
}
impl From<&str> for CollectionError {
    fn from(error: &str) -> Self {
        Self::Other(error.into())
    }
}
impl std::fmt::Display for CollectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Source { path, cause } => {
                write!(f, "当前来源无法读取（{}）：{cause}", path.display())
            }
            Self::Other(error) => f.write_str(error),
        }
    }
}

fn discover(
    agent: &str,
    roots: &[PathBuf],
    warnings: &mut Vec<String>,
    cancel: &dyn Fn() -> bool,
    strict: bool,
) -> Result<BTreeSet<PathBuf>, CollectionError> {
    let mut found = BTreeSet::new();
    let mut pending = roots.to_vec();
    let mut visited = HashSet::new();
    while let Some(dir) = pending.pop() {
        if cancel() {
            return Err("统计已取消".into());
        }
        match fs::symlink_metadata(&dir) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && roots.contains(&dir) => continue,
            Err(e) => return Err(CollectionError::source(&dir, e)),
        }
        let canonical = dir
            .canonicalize()
            .map_err(|e| CollectionError::source(&dir, e))?;
        if !visited.insert(canonical) {
            continue;
        }
        let entries = match fs::read_dir(&dir) {
            Ok(v) => v,
            Err(e) if strict => return Err(CollectionError::source(&dir, e)),
            Err(_) => {
                warnings.push("部分用量目录无法读取".into());
                continue;
            }
        };
        for entry in entries {
            let entry = match entry {
                Ok(v) => v,
                Err(e) if strict => return Err(CollectionError::source(&dir, e)),
                Err(_) => {
                    warnings.push("部分用量文件无法列出".into());
                    continue;
                }
            };
            let path = entry.path();
            let kind = entry
                .file_type()
                .map_err(|e| CollectionError::source(&path, e))?;
            if kind.is_dir() && agent != "opencode" {
                // Pi 扩展的 debug 导出会复制主记录，不是额外模型调用。
                if path.file_name().is_none_or(|n| n != "subagent-artifacts") {
                    pending.push(path);
                }
            } else {
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                let eligible = if agent == "opencode" {
                    name.starts_with("opencode") && name.ends_with(".db")
                } else {
                    name.ends_with(".jsonl") || agent == "codex" && name.ends_with(".jsonl.zst")
                };
                if eligible {
                    found.insert(path);
                }
            }
        }
    }
    Ok(found)
}

#[cfg(test)]
pub(crate) fn read_at(
    agent: &str,
    roots: &[PathBuf],
    store: &mut Store,
    refresh: bool,
    force: bool,
    cancel: &dyn Fn() -> bool,
) -> Result<Option<UsageReport>, String> {
    collect_at(agent, roots, store, refresh, force, cancel, false).map_err(|e| e.to_string())
}

#[cfg(test)]
fn collect_at(
    agent: &str,
    roots: &[PathBuf],
    store: &mut Store,
    refresh: bool,
    force: bool,
    cancel: &dyn Fn() -> bool,
    strict: bool,
) -> Result<Option<UsageReport>, CollectionError> {
    if cancel() {
        return Err("统计已取消".into());
    }
    let started = Instant::now();
    let key = scope(agent, roots);
    let revision = store.revision()?;
    let timezone = timezone_key();
    let timezone_ready = store.timezone_ready(&timezone, started);
    if !force && timezone_ready {
        if let Some(snapshot) = store
            .snapshot
            .as_ref()
            .filter(|s| s.scope == key && s.revision == revision && s.timezone == timezone)
        {
            if !refresh
                || snapshot.readable
                    && unchanged_sources(agent, roots, &snapshot.sources, cancel, strict)?
            {
                let mut report = snapshot.report.clone();
                if cancel() {
                    return Err("统计已取消".into());
                }
                if store.revision()? == revision {
                    report.bytes_read = 0;
                    report.scan_ms = started.elapsed().as_secs_f64() * 1000.;
                    return Ok(Some(report));
                }
            }
        }
    }
    let updated = store
        .ledger()?
        .updated(agent)
        .or_else(|| store.updated(&key));
    if !refresh && updated.is_none() {
        return Ok(None);
    }
    #[cfg(test)]
    {
        store.report_builds += 1;
    }
    let mut files = store.load(&key);
    let mut history = store.ledger()?.load(agent)?;
    let mut recorded = BTreeSet::new();
    // 在清理缺失源之前迁移 0.1.23 缓存，升级时已经删掉的日志仍能保住已读计量。
    ledger::absorb(agent, &mut history, &files, &mut recorded);
    let mut warnings = Vec::new();
    let mut bytes = 0;
    let mut changed = BTreeSet::new();
    let mut obsolete = Vec::new();
    if refresh {
        let paths = discover(agent, roots, &mut warnings, cancel, strict)?;
        if warnings.is_empty() {
            obsolete = files
                .keys()
                .filter(|p| !paths.contains(*p))
                .cloned()
                .collect();
        }
        for p in &obsolete {
            files.remove(p);
        }
        for path in paths {
            if cancel() {
                return Err("统计已取消".into());
            }
            let old = files.get(&path);
            let loaded = if agent == "opencode" {
                opencode::read(&path, old, force, cancel)
            } else {
                cache::read_file(&path, agent, old, force, cancel)
            };
            match loaded {
                Ok((file, read)) => {
                    bytes += read;
                    if read > 0
                        || old.is_none()
                        || old.is_some_and(|old| {
                            old.stamps != file.stamps
                                || old.parser_version != file.parser_version
                                || old.problem.is_some()
                        })
                    {
                        changed.insert(path.clone());
                    }
                    files.insert(path, file);
                }
                Err(error) if cancel() => return Err(error.into()),
                Err(error) if strict => return Err(CollectionError::source(&path, error)),
                Err(_) => {
                    let mut file = old.cloned().unwrap_or_default();
                    file.problem = Some("部分用量文件无法读取，当前结果可能不完整".into());
                    files.insert(path.clone(), file);
                    changed.insert(path);
                }
            }
        }
    }
    if cancel() {
        return Err("统计已取消".into());
    }
    let readable = warnings.is_empty()
        && files.values().all(|f| {
            f.problem.is_none() && f.parser_version >= if agent == "opencode" { 4 } else { 3 }
        });
    ledger::absorb(agent, &mut history, &files, &mut recorded);
    let (records, mut partial, extra) = normalize::records(agent, &history);
    warnings.extend(extra);
    let known_ids: HashSet<_> = records.iter().map(|s| s.key.as_str()).collect();
    let bad: u64 = history.values().map(|f| f.state.bad_lines).sum();
    let missing = history.values().any(|f| {
        f.state.missing_usage > 0
            || f.state
                .unmeasured_ids
                .iter()
                .any(|id| !known_ids.contains(id.as_str()))
    });
    let measured = history.values().filter(|f| !f.samples.is_empty()).count();
    warnings.extend(history.values().filter_map(|f| f.problem.clone()));
    if bad > 0 {
        warnings.push(format!("有 {bad} 条用量记录损坏或过大，已保留可读取数据"));
    }
    if missing {
        warnings.push("部分助手记录未保存 usage，不能从对话文字还原消耗".into());
    }
    if history.values().any(|f| f.pending) {
        warnings.push("部分日志含未完成尾行，仅计入已读取的完整计量".into());
    }
    if agent == "qoder" && records.iter().any(|r| r.partial) {
        warnings
            .push("Qoder 缓存与输入的包含关系尚未确认，总量仅计已记录输入和输出，可能偏低".into());
    }
    partial |= bad > 0 || missing || !warnings.is_empty();
    let models: Vec<_> = records
        .iter()
        .map(|s| s.model.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let names = if cfg!(test) {
        Default::default()
    } else {
        match agent {
            "opencode" => crate::agents::opencode::usage_model_names(&models),
            "qoder" => crate::agents::qoder::usage_model_names(&models),
            _ => Default::default(),
        }
    };
    let mut grouped = BTreeMap::<(String, String), (Tokens, bool, RecordedFields)>::new();
    let mut undated = false;
    for record in &records {
        let date = Local
            .timestamp_millis_opt(record.timestamp)
            .single()
            .filter(|_| record.timestamp > 0)
            .map(|t| t.format("%Y-%m-%d").to_string())
            .unwrap_or_default();
        if date.is_empty() {
            partial = true;
            undated = true;
        }
        let model = if record.model.is_empty() {
            "unknown"
        } else {
            &record.model
        };
        let row = grouped.entry((date, model.to_owned())).or_default();
        row.0.add(&record.tokens);
        row.1 |= record.partial;
        row.2.merge(&record.details);
    }
    let rows = grouped
        .into_iter()
        .map(|((date, model), (tokens, partial, details))| UsageRow {
            model_name: names.get(&model).cloned().unwrap_or_else(|| {
                if model == "unknown" {
                    "未知模型".into()
                } else {
                    model.clone()
                }
            }),
            date,
            model,
            total_tokens: tokens.total(),
            tokens,
            details,
            partial,
        })
        .collect();
    if undated {
        warnings.push("部分记录没有有效时间，只计入全部范围，不加入每日趋势".into());
    }
    warnings.sort();
    warnings.dedup();
    let now = if refresh {
        Utc::now().timestamp_millis()
    } else {
        updated.unwrap_or(0)
    };
    if cancel() {
        return Err("统计已取消".into());
    }
    if refresh || !recorded.is_empty() {
        store.ledger()?.commit(agent, &history, &recorded, now)?;
    }
    if refresh {
        store.commit(&key, &files, &changed, &obsolete, now);
    }
    let report = UsageReport {
        agent: agent.into(),
        updated_at: now,
        rows,
        has_usage: !records.is_empty(),
        partial,
        file_count: history.len(),
        measured_file_count: measured,
        warnings,
        scan_ms: started.elapsed().as_secs_f64() * 1000.,
        bytes_read: bytes,
    };
    let after = store.revision()?;
    store.snapshot = (timezone_ready && revision.same_external(&after)).then(|| ReportSnapshot {
        scope: key,
        revision: after,
        timezone,
        sources: files.into_iter().map(|(p, f)| (p, f.stamps)).collect(),
        readable,
        report: report.clone(),
    });
    Ok(Some(report))
}
