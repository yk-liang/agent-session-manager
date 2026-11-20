use crate::usage::{cache::FileCache, normalize, source::jsonl, storage, types::Sample};
use rusqlite::{params, Connection};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};
#[cfg(test)]
use std::collections::{BTreeMap, BTreeSet};

pub(in crate::usage) fn source_keys(
    db: &Connection,
    agent: &str,
    path: &Path,
    file: &FileCache,
) -> Result<(), String> {
    let source: i64 = db
        .query_row(
            "SELECT id FROM usage_ledger_sources WHERE agent=?1 AND path=?2",
            params![agent, path.to_string_lossy()],
            |r| r.get(0),
        )
        .map_err(|_| "无法定位请求来源")?;
    let mut insert = db
        .prepare_cached("INSERT OR IGNORE INTO usage_source_keys VALUES (?1,?2)")
        .map_err(|_| "无法更新请求来源索引")?;
    // 保留旧身份连接：日志改写或父历史迟到后，先前生成的差额仍需一起重新归并。
    for key in normalize::source_keys(agent, path, file) {
        insert
            .execute(params![source, key])
            .map_err(|_| "无法更新请求来源索引")?;
    }
    Ok(())
}

pub(in crate::usage) fn observation_ids(samples: &[Sample]) -> Vec<String> {
    let mut counts = HashMap::<String, usize>::new();
    samples
        .iter()
        .map(|sample| {
            let identity = if let Some(id) = &sample.response_id {
                format!("response:{id}")
            } else if let Some(ordinal) = sample.ordinal {
                format!(
                    "ordinal:{}:{ordinal}:{}:{}:{}",
                    sample.session,
                    sample.timestamp,
                    sample.tokens.input_tokens,
                    sample.tokens.output_tokens,
                )
            } else {
                let encoded = serde_json::to_vec(&(
                    sample.session.as_str(),
                    sample.timestamp,
                    &sample.tokens,
                    &sample.last_usage,
                    sample.cumulative,
                ))
                .unwrap_or_default();
                format!("{:x}", Sha256::digest(encoded))
            };
            let n = counts.entry(identity.clone()).or_default();
            *n += 1;
            format!("{identity}:{n}")
        })
        .collect()
}

pub(super) fn previous_codex_source(
    tx: &Connection,
    path: &Path,
    incoming: &FileCache,
) -> Result<Option<FileCache>, String> {
    let Some(origin) = &incoming.origin_path else {
        return Ok(None);
    };
    let mut stmt = tx
        .prepare_cached(
            "SELECT s.id,s.data FROM usage_source_keys k
         JOIN usage_ledger_sources s ON s.id=k.source
         WHERE s.agent='codex' AND k.key=?1",
        )
        .map_err(|_| "无法查找 Codex 历史来源")?;
    let rows = stmt
        .query_map([format!("file:{}", origin.display())], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                storage::decode::<FileCache>(row.get_ref(1)?)?,
            ))
        })
        .map_err(|_| "无法读取 Codex 历史来源")?;
    for row in rows {
        let (id, old) = row.map_err(|_| "无法读取 Codex 历史来源")?;
        if old.parser_version < incoming.parser_version
            && jsonl::covers_observations(incoming, &old)
        {
            tx.execute(
                "UPDATE usage_ledger_sources SET path=?1 WHERE id=?2",
                params![path.to_string_lossy(), id],
            )
            .map_err(|_| "无法更新 Codex 来源身份")?;
            return Ok(Some(old));
        }
    }
    Ok(None)
}

pub(super) fn replace_covered_archives(
    tx: &Connection,
    path: &Path,
    incoming: &FileCache,
) -> Result<(), String> {
    let rewrite = || -> rusqlite::Result<()> {
        let target: i64 = tx.query_row(
            "SELECT id FROM usage_ledger_sources WHERE agent='codex' AND path=?1",
            [path.to_string_lossy()],
            |row| row.get(0),
        )?;
        let other_sources = tx.prepare_cached(
            "SELECT s.id,s.data FROM usage_source_keys k JOIN usage_ledger_sources s ON s.id=k.source
            WHERE s.agent='codex' AND k.key=?1 AND s.id<>?2",
        )?.query_map(params![format!("physical:{}", normalize::physical_id(path)), target], |row| {
            Ok((row.get::<_, i64>(0)?, storage::decode::<FileCache>(row.get_ref(1)?)?))
        })?.collect::<Result<Vec<_>, _>>()?;
        for (id, old) in other_sources {
            if old.parser_version > incoming.parser_version
                || !jsonl::covers_observations(incoming, &old)
            {
                continue;
            }
            tx.execute(
                "INSERT OR IGNORE INTO usage_source_keys SELECT ?1,key FROM usage_source_keys WHERE source=?2",
                params![target, id],
            )?;
            tx.execute(
                "UPDATE usage_request_events SET source=?1 WHERE agent='codex' AND source=?2",
                params![target, id],
            )?;
            tx.execute("DELETE FROM usage_source_keys WHERE source=?1", [id])?;
            tx.execute("DELETE FROM usage_ledger_sources WHERE id=?1", [id])?;
        }
        Ok(())
    };
    rewrite().map_err(|e| format!("无法归并 Codex 归档来源：{e}"))
}

fn merge_reparsed(agent: &str, old: &FileCache, incoming: &FileCache) -> FileCache {
    if jsonl::covers_observations(incoming, old) {
        return incoming.clone();
    }
    replace_observations(agent, old, incoming)
}

// SQLite 来源也可能已删掉部分会话。重新读取整个文件仍须保留未再次观察到的历史。
pub(super) fn replace_observations(agent: &str, old: &FileCache, incoming: &FileCache) -> FileCache {
    let mut merged = incoming.clone();
    if merged.state.session.is_empty() && !old.state.session.is_empty() {
        merged.state = old.state.clone();
    }
    merged.samples = replace_matching(agent, &old.samples, &incoming.samples, agent == "codex");
    merged.failures = replace_matching(agent, &old.failures, &incoming.failures, false);
    if merged.samples.len() > incoming.samples.len() || merged.failures.len() > incoming.failures.len() {
        merged.offset = old.offset.max(incoming.offset);
        clear_source_coverage(&mut merged);
    }
    merged
}

fn replace_matching(agent: &str, old: &[Sample], new: &[Sample], observations: bool) -> Vec<Sample> {
    let keys: HashSet<_> = new
        .iter()
        .filter(|sample| !sample.key.is_empty())
        .map(|sample| normalize::canonical_key(agent, &sample.key))
        .collect();
    let missing_key = |sample: &Sample| !keys.contains(&normalize::canonical_key(agent, &sample.key));
    if !observations {
        return old.iter().filter(|sample| missing_key(sample)).chain(new).cloned().collect();
    }

    // Codex 的累计计数依赖观测顺序。保留已截断的尾部时仍需先处理现存前缀。
    let positions: HashMap<_, _> = observation_ids(new)
        .into_iter()
        .enumerate()
        .map(|(index, id)| (id, index))
        .collect();
    let mut merged = Vec::with_capacity(old.len().max(new.len()));
    let mut next = 0;
    for (id, sample) in observation_ids(old).into_iter().zip(old) {
        if let Some(&index) = positions.get(&id) {
            if index >= next {
                merged.extend_from_slice(&new[next..=index]);
                next = index + 1;
            }
        } else if missing_key(sample) {
            merged.push(sample.clone());
        }
    }
    merged.extend_from_slice(&new[next..]);
    merged
}

pub(in crate::usage) fn merge(agent: &str, old: &FileCache, incoming: &FileCache) -> FileCache {
    if incoming.parser_version > old.parser_version {
        return merge_reparsed(agent, old, incoming);
    }
    if incoming.parser_version < old.parser_version {
        return merge_reparsed(agent, incoming, old);
    }
    merge_observations(agent, old, incoming)
}

pub(in crate::usage) fn merge_restored(agent: &str, old: &FileCache, incoming: &FileCache) -> FileCache {
    // 缓存记录来自不同 scope 的独立快照。相同文件戳也可能各自保留不同历史。
    let mut merged = if incoming.parser_version < old.parser_version {
        merge_observations(agent, incoming, old)
    } else {
        merge_observations(agent, old, incoming)
    };
    clear_source_coverage(&mut merged);
    merged
}

pub(in crate::usage) fn clear_source_coverage(file: &mut FileCache) {
    // 独立历史不能由当前文件指纹证明已完整覆盖。
    file.stamps.clear();
    file.head_len = 0;
}

fn includes_detached_history(file: &FileCache) -> bool {
    file.stamps.is_empty() && file.head_len == 0 && (!file.samples.is_empty() || !file.failures.is_empty())
}

fn merge_observations(agent: &str, old: &FileCache, incoming: &FileCache) -> FileCache {
    let mut merged = incoming.clone();
    if merged.state.session.is_empty() && !old.state.session.is_empty() {
        merged.state = old.state.clone();
    }
    if includes_detached_history(old) {
        clear_source_coverage(&mut merged);
    }
    merged.offset = old.offset.max(incoming.offset);
    let failures = old.failures.iter().chain(&incoming.failures).cloned();
    merged.failures = normalize::unique(agent, failures);
    if agent != "codex" {
        merged.samples =
            normalize::unique(agent, old.samples.iter().chain(&incoming.samples).cloned());
        return merged;
    }
    let mut samples = old.samples.clone();
    let mut positions: HashMap<_, _> = observation_ids(&samples)
        .into_iter()
        .enumerate()
        .map(|(i, k)| (k, i))
        .collect();
    for (key, sample) in observation_ids(&incoming.samples)
        .into_iter()
        .zip(&incoming.samples)
    {
        if let Some(&index) = positions.get(&key) {
            let mut details = samples[index].details.clone();
            details.refine(&sample.details);
            let mut request = samples[index].request.clone();
            request.merge_codex(&sample.request);
            if normalize::prefer(sample, &samples[index]) {
                samples[index] = sample.clone();
            }
            samples[index].details = details;
            samples[index].request = request;
        } else {
            positions.insert(key, samples.len());
            samples.push(sample.clone());
        }
    }
    merged.samples = samples;
    merged
}

#[cfg(test)]
pub(in crate::usage) fn absorb(
    agent: &str,
    history: &mut BTreeMap<PathBuf, FileCache>,
    incoming: &BTreeMap<PathBuf, FileCache>,
    changed: &mut BTreeSet<PathBuf>,
) {
    for (path, file) in incoming {
        let key = source_path(agent, path, file);
        if history.get(&key).is_some_and(|old| {
            old.stamps == file.stamps && old.parser_version == file.parser_version
        }) {
            continue;
        }
        let mut file = file.clone();
        file.origin_path = Some(path.clone());
        let combined = history
            .get(&key)
            .map(|old| merge(agent, old, &file))
            .unwrap_or_else(|| file.clone());
        history.insert(key.clone(), combined);
        changed.insert(key);
    }
}

pub(in crate::usage) fn same_source(agent: &str, left: &FileCache, right: &FileCache) -> bool {
    left.state.session == right.state.session
        && (agent != "codex" || normalize::segment_id(left) == normalize::segment_id(right))
}

pub(in crate::usage) fn source_path(agent: &str, path: &Path, file: &FileCache) -> PathBuf {
    // 同一路径可被不同会话或分页复用。保留原文件名以识别 Codex 物理线程 ID。
    let segment;
    let identity = if agent == "codex" {
        segment = normalize::segment_id(file);
        &segment
    } else {
        &file.state.session
    };
    let identity = format!("{:x}", Sha256::digest(identity.as_bytes()));
    path.join(format!("ledger-{identity}")).join(path.file_name().unwrap_or_default())
}
