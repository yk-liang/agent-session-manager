//! 采集只提交变化来源的计量，不为后台文件事件生成全历史报表。
use super::{cache, ledger, opencode, scope, Store};
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
    let key = scope(agent, roots);
    if store.ledger()?.updated(agent).is_none() {
        // 从只有可重建缓存的旧版本升级时，先接入已采集数据，包括已删除的源文件。
        let cached = store.load(&key);
        if !cached.is_empty() {
            let mut history = BTreeMap::new();
            let mut recorded = BTreeSet::new();
            ledger::absorb(agent, &mut history, &cached, &mut recorded);
            store.ledger()?.commit(
                agent,
                &history,
                &recorded,
                chrono::Utc::now().timestamp_millis(),
            )?;
        }
    }
    let discovered;
    let paths = match paths {
        Some(paths) => paths,
        None => {
            discovered = super::discover(agent, roots, &mut Vec::new(), cancel, true)
                .map_err(|e| e.to_string())?;
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
            Err(_) => return Err("采集来源暂时无法读取".into()),
            Ok(m) if !m.is_file() => continue,
            _ => {}
        }
        result.checked_files += 1;
        let stamps = if agent == "opencode" {
            opencode::stamps(path)
        } else {
            cache::stamp(path).map(|s| vec![s])
        }
        .map_err(|_| "无法读取用量文件元数据")?;
        if store.unchanged(&key, path, &stamps, if agent == "opencode" { 4 } else { 3 }) {
            continue;
        }
        let old = store.load_file(&key, path);
        let (file, bytes) = if agent == "opencode" {
            opencode::read(path, old.as_ref(), false, cancel)
        } else {
            cache::read_file(path, agent, old.as_ref(), false, cancel)
        }
        .map_err(|e| super::CollectionError::source(path, e).to_string())?;
        let changed = old.as_ref().is_none_or(|old| {
            old.stamps != file.stamps
                || old.parser_version != file.parser_version
                || old.problem.is_some()
        });
        if !changed {
            // 首次迁移小索引；以后只比较 stamps，不再加载整份 FileCache。
            store.commit(
                &key,
                &BTreeMap::from([(path.clone(), file)]),
                &BTreeSet::from([path.clone()]),
                &[],
                chrono::Utc::now().timestamp_millis(),
            );
            continue;
        }
        let incoming = BTreeMap::from([(path.clone(), file)]);
        let mut history = BTreeMap::new();
        let mut recorded = BTreeSet::new();
        ledger::absorb(agent, &mut history, &incoming, &mut recorded);
        // 每份来源解析后立即持久化，后面的大文件/失败不能阻止已读计量入账。
        // commit 在事务内合并该来源；无需加载另外数万条历史或保留正文。
        let at = chrono::Utc::now().timestamp_millis();
        store.ledger()?.commit(agent, &history, &recorded, at)?;
        store.commit(&key, &incoming, &BTreeSet::from([path.clone()]), &[], at);
        result.files += 1;
        result.bytes_read += bytes;
    }
    progress(paths.len(), paths.len());
    Ok(result)
}
