//! 采集只提交变化来源的计量，不为后台文件事件生成全历史报表。
use super::{discover, events, ledger, scope, source::Source, source_error, Store};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

#[derive(Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Ingested {
    pub checked_files: usize,
    pub files: usize,
    pub bytes_read: u64,
}

#[cfg(test)]
pub(super) fn collect(
    agent: &str,
    roots: &[PathBuf],
    store: &mut Store,
    paths: Option<&BTreeSet<PathBuf>>,
    cancel: &dyn Fn() -> bool,
) -> Result<Ingested, String> {
    store.recover_released_cache(agent, roots, cancel)?;
    collect_with_progress(agent, roots, store, paths, cancel, &|_, _| {})
}

pub(super) fn collect_with_progress(
    agent: &str,
    roots: &[PathBuf],
    store: &mut Store,
    paths: Option<&BTreeSet<PathBuf>>,
    cancel: &dyn Fn() -> bool,
    progress: &dyn Fn(usize, usize),
) -> Result<Ingested, String> {
    let input = Source::for_agent(agent);
    let key = scope(agent, roots);
    let full_scan = paths.is_none();
    let discovered;
    let paths = match paths {
        Some(paths) => paths,
        None => {
            discovered = discover(agent, roots, cancel)?;
            &discovered
        }
    };
    let mut result = Ingested::default();
    for (done, path) in paths.iter().enumerate() {
        if cancel() {
            return Err("采集已停止".into());
        }
        progress(done, paths.len());
        if !roots.iter().any(|root| path.starts_with(root)) {
            continue;
        }
        match std::fs::metadata(path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(source_error(path, error)),
            Ok(m) if !m.is_file() => continue,
            _ => {}
        }
        result.checked_files += 1;
        let stamps = input
            .stamps(path)
            .map_err(|error| source_error(path, error))?;
        if store.ledger()?.unchanged(agent, path, &stamps, input.parser_version(path))? {
            continue;
        }
        let old = store.load_file(&key, path);
        let (file, bytes) = input
            .read(path, old.as_ref(), false, cancel)
            .map_err(|e| source_error(path, e))?;
        let incoming = BTreeMap::from([(path.clone(), file)]);
        let mut history = BTreeMap::new();
        let mut recorded = BTreeSet::new();
        ledger::sources::absorb(agent, &mut history, &incoming, &mut recorded);
        // 每份来源解析后立即持久化，后面的大文件/失败不能阻止已读计量入账。
        // commit 在事务内合并该来源；无需加载另外数万条历史或保留正文。
        let at = chrono::Utc::now().timestamp_millis();
        store.ledger()?.commit(agent, &history, &recorded, at)?;
        store.commit(&key, &incoming, &BTreeSet::from([path.clone()]), &[], at);
        result.files += 1;
        result.bytes_read += bytes;
    }
    if full_scan && !cancel() {
        store.prune_missing(&key, paths);
    }
    progress(paths.len(), paths.len());
    Ok(result)
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Synced {
    pub updated_at: i64,
    pub checked_files: usize,
    pub changed_files: usize,
    pub bytes_read: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncProgress {
    pub agent: String,
    pub phase: &'static str,
    /// 已检查的日志文件数；发现目录/等待其他同步时尚不知道总数。
    pub checked_files: usize,
    pub total_files: Option<usize>,
}

#[allow(dead_code)] // 供隔离的用量性能示例与测试使用。
pub(crate) fn sync_at(
    agent: &str,
    roots: &[PathBuf],
    store: &mut Store,
    cancel: &dyn Fn() -> bool,
) -> Result<Synced, String> {
    sync_paths(agent, roots, store, None, cancel)
}

pub(super) fn sync_paths(
    agent: &str,
    roots: &[PathBuf],
    store: &mut Store,
    paths: Option<&BTreeSet<PathBuf>>,
    cancel: &dyn Fn() -> bool,
) -> Result<Synced, String> {
    sync_with_store(agent, roots, store, paths, cancel, &|_| {})
}

pub(super) fn sync_with_store(
    agent: &str,
    roots: &[PathBuf],
    store: &mut Store,
    paths: Option<&BTreeSet<PathBuf>>,
    cancel: &dyn Fn() -> bool,
    progress: &dyn Fn(SyncProgress),
) -> Result<Synced, String> {
    let report = |phase, checked_files, total_files| {
        progress(SyncProgress {
            agent: agent.into(),
            phase,
            checked_files,
            total_files,
        })
    };
    report("discovering", 0, None);
    let recovery = store.recover_released_cache(agent, roots, cancel);
    // 即使稍后的文件不可读，也先把本轮已经入账的部分转成持久请求事件。
    let collected = collect_with_progress(agent, roots, store, paths, cancel, &|done, total| {
        report("reading", done, Some(total))
    });
    if !cancel() {
        report("indexing", 0, None);
        events::refresh(&mut store.ledger()?.db, agent, cancel)?;
    }
    recovery?;
    let result = collected?;
    if cancel() {
        return Err("采集已停止".into());
    }
    if paths.is_none() {
        // 必须在目录遍历和请求索引全部成功后标记；部分文件入账不能冒充全量完成。
        store
            .ledger()?
            .db
            .execute(
                "INSERT OR IGNORE INTO usage_sync_state(agent,completed_at) VALUES (?1,?2)",
                rusqlite::params![agent, chrono::Utc::now().timestamp_millis()],
            )
            .map_err(|_| "无法保存用量同步状态")?;
    }
    report("complete", result.checked_files, Some(result.checked_files));
    Ok(Synced {
        updated_at: chrono::Utc::now().timestamp_millis(),
        checked_files: result.checked_files,
        changed_files: result.files,
        bytes_read: result.bytes_read,
    })
}
