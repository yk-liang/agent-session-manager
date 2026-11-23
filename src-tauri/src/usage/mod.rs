//! 从本地日志采集、去重并汇总 Token 用量。
mod antigravity;
mod cache;
mod claude;
mod codex;
#[cfg(test)]
mod cold_start_tests;
pub mod collector;
mod deletion;
#[cfg(test)]
mod event_tests;
pub mod events;
pub(crate) mod ingest;
mod ledger;
mod normalize;
mod opencode;
mod parse;
mod pi;
pub mod pricing;
mod request_parse;
pub mod service;
mod source;
mod storage;
#[cfg(test)]
mod storage_bench;
#[cfg(test)]
mod storage_tests;
pub mod summary;
#[cfg(test)]
mod summary_tests;
#[cfg(test)]
mod tests;
mod types;

pub(crate) use cache::Store;
pub(crate) use codex::failures::database as codex_failure_database;
#[cfg(test)]
use chrono::{Local, TimeZone, Utc};
use sha2::{Digest, Sha256};
use source::Source;
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

pub const AGENTS: [&str; 6] = ["qoder", "claude", "codex", "opencode", "pi", "antigravity"];

#[cfg(test)]
struct ReportSnapshot {
    scope: String,
    revision: cache::StoreRevision,
    timezone: String,
    sources: BTreeMap<PathBuf, Vec<source::Stamp>>,
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
    sources: &BTreeMap<PathBuf, Vec<source::Stamp>>,
    cancel: &dyn Fn() -> bool,
) -> Result<bool, String> {
    let input = Source::for_agent(agent);
    let paths = discover(agent, roots, cancel)?;
    if paths.len() != sources.len() {
        return Ok(false);
    }
    for path in paths {
        if cancel() {
            return Err("统计已取消".into());
        }
        let stamps = input.stamps(&path);
        match stamps {
            Ok(stamps) if sources.get(&path) == Some(&stamps) => {}
            _ => return Ok(false),
        }
    }
    Ok(true)
}

pub fn roots(agent: &str) -> Result<Vec<PathBuf>, String> {
    if !AGENTS.contains(&agent) {
        return Err("不支持的用量来源".into());
    }
    if agent == "antigravity" {
        return Ok(crate::agents::antigravity::conversation_dirs());
    }
    let root = crate::agents::find_source(agent)?
        .data_dir()
        .ok_or("缺少数据目录")?;
    Ok(match agent {
        "claude" => crate::agents::claude::project_roots(),
        "qoder" => vec![root.join("projects")],
        "codex" => vec![
            root.join("sessions"),
            root.join("archived_sessions"),
            codex::failures::database(&root),
        ],
        "pi" => vec![root.join("sessions")],
        _ => vec![root],
    })
}
fn scope(agent: &str, roots: &[PathBuf]) -> String {
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&(agent, roots)).unwrap_or_default())
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
        let path = (!cfg!(test)).then(cache::path).flatten();
        let ledger_path = (!cfg!(test)).then(|| ledger::path().ok()).flatten();
        Mutex::new(Store::open_with_ledger(
            path.as_deref(),
            ledger_path.as_deref(),
        ))
    })
}

/// 删除动作必须晚于账本的持久化提交；失败时保留会话，不能静默丢掉最后一轮用量。
pub fn preserve_before_delete(
    agent: &str,
    targets: Option<&[PathBuf]>,
    delete: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    let roots = roots(agent)?;
    let _writer = ledger::SyncLock::acquire(&ledger::path()?, &|| false)?;
    let mut store = store(agent).lock().unwrap_or_else(|e| e.into_inner());
    match targets {
        Some(targets) => deletion::preserve_at(agent, &roots, &mut store, targets, delete),
        None => preserve_before_delete_at(agent, &roots, &mut store, delete),
    }
}

pub(crate) fn preserve_codex_deletion(
    targets: &[PathBuf],
    validate: impl FnOnce() -> Result<(), String>,
    delete: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    let roots = roots("codex")?;
    let _writer = ledger::SyncLock::acquire(&ledger::path()?, &|| false)?;
    let mut store = store("codex").lock().unwrap_or_else(|e| e.into_inner());
    deletion::preserve_validated_at("codex", &roots, &mut store, targets, validate, delete)
}

fn preserve_before_delete_at(
    agent: &str,
    roots: &[PathBuf],
    store: &mut Store,
    delete: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    ingest::sync_at(agent, roots, store, &|| false)
        .map_err(|e| format!("删除前未能保存累计用量，会话已保留：{e}"))?;
    delete()
}

fn source_error(path: &std::path::Path, cause: impl std::fmt::Display) -> String {
    format!("无法读取用量来源 {}：{cause}", path.display())
}

fn discover(
    agent: &str,
    roots: &[PathBuf],
    cancel: &dyn Fn() -> bool,
) -> Result<BTreeSet<PathBuf>, String> {
    let input = Source::for_agent(agent);
    let mut found = BTreeSet::new();
    let mut pending: Vec<_> = roots.iter().map(|dir| (dir, dir.clone())).collect();
    let mut visited = HashSet::new();
    while let Some((root, dir)) = pending.pop() {
        if cancel() {
            return Err("统计已取消".into());
        }
        match fs::symlink_metadata(&dir) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && roots.contains(&dir) => continue,
            Err(e) => return Err(source_error(&dir, e)),
        }
        if dir == *root && input.is_file_root(&dir) {
            if dir.is_file() {
                found.insert(dir);
                continue;
            }
            return Err(source_error(&dir, "Codex 历史数据库必须是普通文件"));
        }
        let canonical = dir
            .canonicalize()
            .map_err(|e| source_error(&dir, e))?;
        if !visited.insert(canonical) {
            continue;
        }
        let entries = fs::read_dir(&dir).map_err(|e| source_error(&dir, e))?;
        for entry in entries {
            let entry = entry.map_err(|e| source_error(&dir, e))?;
            let path = entry.path();
            let kind = entry
                .file_type()
                .map_err(|e| source_error(&path, e))?;
            if kind.is_dir() && input.descends(&path) {
                // Pi 扩展的 debug 导出会复制主记录，不是额外模型调用。
                if path.file_name().is_none_or(|n| n != "subagent-artifacts") {
                    pending.push((root, path));
                }
            } else {
                if input.discovers(root, &path, kind) {
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
    if cancel() {
        return Err("统计已取消".into());
    }
    if refresh {
        // 直接生成测试报表时，解析缓存不可用仍可从现存日志统计。
        let _ = store.recover_released_cache(agent, roots, cancel);
    }
    let started = Instant::now();
    let input = Source::for_agent(agent);
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
                    && unchanged_sources(agent, roots, &snapshot.sources, cancel)?
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
    let updated = store.ledger()?.updated(agent);
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
    let mut bytes = 0;
    let mut changed = BTreeSet::new();
    let mut obsolete = Vec::new();
    let mut readable = true;
    if refresh {
        let paths = discover(agent, roots, cancel)?;
        obsolete = files.keys().filter(|p| !paths.contains(*p)).cloned().collect();
        for p in &obsolete {
            files.remove(p);
        }
        for path in paths {
            if cancel() {
                return Err("统计已取消".into());
            }
            let old = files.get(&path);
            let loaded = input.read(&path, old, force, cancel);
            match loaded {
                Ok((file, read)) => {
                    bytes += read;
                    if read > 0
                        || old.is_none()
                        || old.is_some_and(|old| {
                            old.stamps != file.stamps
                                || old.parser_version != file.parser_version
                        })
                    {
                        changed.insert(path.clone());
                    }
                    files.insert(path, file);
                }
                Err(error) if cancel() => return Err(error),
                Err(_) => readable = false,
            }
        }
    }
    if cancel() {
        return Err("统计已取消".into());
    }
    readable &= files.iter().all(|(path, file)| file.parser_version >= input.parser_version(path));
    let incoming = changed.iter().map(|path| (path.clone(), files[path].clone())).collect();
    ledger::sources::absorb(agent, &mut history, &incoming, &mut recorded);
    let records = normalize::records(agent, &history);
    let measured = history.values().filter(|f| !f.samples.is_empty()).count();
    let mut partial = !readable || history.values().any(|file| file.pending);
    let mut grouped = BTreeMap::<(String, String), (Tokens, bool, RecordedFields)>::new();
    for record in &records {
        let date = Local
            .timestamp_millis_opt(record.timestamp)
            .single()
            .filter(|_| record.timestamp > 0)
            .map(|t| t.format("%Y-%m-%d").to_string())
            .unwrap_or_default();
        if date.is_empty() {
            partial = true;
        }
        let model = if record.model.is_empty() {
            "unknown"
        } else {
            &record.model
        };
        let row = grouped.entry((date, model.to_owned())).or_default();
        row.0.add(&record.tokens);
        partial |= record.partial;
        row.1 |= record.partial;
        row.2.merge(&record.details);
    }
    let rows = grouped
        .into_iter()
        .map(|((date, model), (tokens, partial, details))| UsageRow {
            date,
            model,
            total_tokens: tokens.total(),
            tokens,
            details,
            partial,
        })
        .collect();
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
