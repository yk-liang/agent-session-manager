//! 累计计数和逐响应明细同时存在时只保留一次；参考 ccusage 的 replay/delta 思路。
use super::{cache::FileCache, types::*};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    path::{Path, PathBuf},
};

pub(super) fn prefer(next: &Sample, old: &Sample) -> bool {
    let known = |s: &Sample| !s.model.is_empty() && s.model != "unknown";
    (
        !next.replayed,
        next.tokens.total(),
        known(next),
        next.complete,
        next.tokens.output_tokens,
        next.timestamp,
        !next.partial,
    ) > (
        !old.replayed,
        old.tokens.total(),
        known(old),
        old.complete,
        old.tokens.output_tokens,
        old.timestamp,
        !old.partial,
    )
}
pub(super) fn unique(samples: impl IntoIterator<Item = Sample>) -> Vec<Sample> {
    let mut by_key = HashMap::<String, Sample>::new();
    for mut s in samples {
        s.migrate_availability();
        if let Some(old) = by_key.get_mut(&s.key) {
            if s.copy_fingerprint.is_some() {
                old.copy_fingerprint = s.copy_fingerprint.clone();
                old.replayed = s.replayed;
            }
            let mut details = old.details.clone();
            details.refine(&s.details);
            let mut request = old.request.clone();
            if old.replayed && !s.replayed {
                request = s.request.clone();
            } else if !s.replayed || old.replayed {
                request.merge(&s.request);
            }
            if prefer(&s, old) {
                *old = s;
            }
            old.details = details;
            old.request = request;
        } else {
            by_key.insert(s.key.clone(), s);
        }
    }
    by_key.into_values().collect()
}
fn physical_id(path: &Path) -> String {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let name = name.trim_end_matches(".zst").trim_end_matches(".jsonl");
    name.get(name.len().saturating_sub(36)..)
        .unwrap_or(name)
        .to_owned()
}

fn canonical_key(agent: &str, key: &str) -> String {
    if matches!(agent, "claude" | "qoder") {
        if let Some((prefix, id)) = key.rsplit_once(':') {
            if (prefix.starts_with("session:") || prefix.starts_with("request:"))
                && crate::agents::claude_family::is_session_id(id)
            {
                return format!("message:{id}");
            }
        }
    }
    key.to_owned()
}

/// 只保存归并会用到的身份索引；更新一个会话时无需解码其他会话的历史观测。
pub(super) fn source_keys(agent: &str, path: &Path, file: &FileCache) -> BTreeSet<String> {
    let origin = file.origin_path.as_deref().unwrap_or(path);
    let mut keys = BTreeSet::from([format!("file:{}", origin.display())]);
    if !file.state.session.is_empty() {
        keys.insert(format!("session:{}", file.state.session));
    }
    if matches!(agent, "codex" | "pi") {
        keys.insert(format!("physical:{}", physical_id(path)));
        if let Some(base) = &file.state.history_base {
            keys.insert(format!("physical:{}", base.thread_id));
        }
        if let Some(parent) = &file.state.parent_session {
            keys.extend([
                format!("session:{parent}"),
                format!("physical:{parent}"),
                format!("file:{parent}"),
                format!(
                    "file:{}",
                    origin
                        .parent()
                        .unwrap_or(Path::new(""))
                        .join(parent)
                        .display()
                ),
            ]);
        }
    }
    for sample in file.samples.iter().chain(&file.failures) {
        // Pi 的短 entry ID 只在父会话链内唯一，不能用它连接无关会话。
        if !sample.key.is_empty() && (agent != "pi" || sample.key.starts_with("response:")) {
            keys.insert(format!("request:{}", canonical_key(agent, &sample.key)));
        }
        if agent == "codex" && sample.cumulative {
            keys.insert(format!("counter:{}", counter_identity(sample)));
        }
        if let Some(fingerprint) = &sample.copy_fingerprint {
            keys.insert(format!("copy:{fingerprint}"));
        }
    }
    if matches!(agent, "claude" | "qoder") {
        for key in &file.state.unmeasured_ids {
            keys.insert(format!("request:{}", canonical_key(agent, key)));
        }
    }
    keys
}
fn parent_path<'a>(
    path: &Path,
    file: &FileCache,
    files: &'a BTreeMap<PathBuf, FileCache>,
) -> Option<&'a PathBuf> {
    if let Some(base) = &file.state.history_base {
        if let Some((p, _)) = files
            .iter()
            .filter(|(p, _)| physical_id(p) == base.thread_id && p.as_path() != path)
            .max_by_key(|(_, source)| (source.offset, source.samples.len()))
        {
            return Some(p);
        }
    }
    let id = file.state.parent_session.as_deref()?;
    let exact = PathBuf::from(id);
    let relative = file
        .origin_path
        .as_deref()
        .unwrap_or(path)
        .parent()
        .unwrap_or(Path::new(""))
        .join(id);
    files
        .iter()
        .filter(|(p, f)| {
            p.as_path() != path
                && (f.origin_path.as_deref().unwrap_or(p) == exact
                    || f.origin_path.as_deref().unwrap_or(p) == relative)
        })
        .max_by_key(|(_, source)| (source.offset, source.samples.len()))
        .map(|(p, _)| p)
        .or_else(|| {
            files
                .iter()
                .filter(|(p, f)| {
                    p.as_path() != path && (physical_id(p) == id || f.state.session == id)
                })
                .max_by_key(|(_, source)| (source.offset, source.samples.len()))
                .map(|(p, _)| p)
        })
}
fn baseline(
    path: &Path,
    file: &FileCache,
    files: &BTreeMap<PathBuf, FileCache>,
    seen: &mut HashSet<PathBuf>,
) -> Option<Tokens> {
    if file.state.history_base.is_none() && file.state.parent_session.is_none() {
        return Some(Tokens::default());
    }
    if !seen.insert(path.to_owned()) {
        return None;
    }
    let parent = parent_path(path, file, files)?;
    let source = files.get(parent)?;
    let value = |s: &Sample| {
        if s.cumulative {
            Some(s.tokens.clone())
        } else {
            s.thread_total.clone()
        }
    };
    if let Some(base) = &file.state.history_base {
        if base.end_byte_offset.is_none() && base.end_ordinal_exclusive.is_none() {
            return None;
        }
        if base.end_byte_offset.is_some_and(|end| end > source.offset) {
            return None;
        }
        source
            .samples
            .iter()
            .rev()
            .filter(|s| {
                base.end_byte_offset.is_none_or(|end| s.offset < end)
                    && base.end_ordinal_exclusive.is_none_or(|end|
                    // 新页可能引用旧版无 ordinal 的父日志，此时字节边界仍然可靠。
                    s.ordinal.map(|n|n<end).unwrap_or(base.end_byte_offset.is_some()))
            })
            .find_map(value)
            .or_else(|| baseline(parent, source, files, seen))
    } else {
        // 没有 history_base 的旧 fork，从父线程在 fork 时刻的计量建立起点。
        source
            .samples
            .iter()
            .filter(|s| {
                file.state
                    .created_at
                    .is_none_or(|end| s.timestamp > 0 && s.timestamp <= end)
                    && file
                        .state
                        .parent_before
                        .is_none_or(|end| s.ordinal.is_none_or(|n| n < end))
            })
            .filter_map(|s| value(s).map(|v| ((s.timestamp, s.offset), v)))
            .max_by_key(|(key, _)| *key)
            .map(|(_, v)| v)
            .or_else(|| source.samples.is_empty().then(Tokens::default))
    }
}
fn delta(current: &Tokens, previous: &Tokens) -> (Tokens, bool) {
    let reset = current.input_tokens < previous.input_tokens
        || current.output_tokens < previous.output_tokens
        || current.cache_read_tokens < previous.cache_read_tokens
        || current.cache_write_tokens < previous.cache_write_tokens;
    (
        Tokens {
            input_tokens: current.input_tokens.saturating_sub(previous.input_tokens),
            output_tokens: current.output_tokens.saturating_sub(previous.output_tokens),
            cache_read_tokens: current
                .cache_read_tokens
                .saturating_sub(previous.cache_read_tokens),
            cache_write_tokens: current
                .cache_write_tokens
                .saturating_sub(previous.cache_write_tokens),
            reasoning_tokens: current
                .reasoning_tokens
                .saturating_sub(previous.reasoning_tokens),
            unclassified_tokens: current
                .unclassified_tokens
                .saturating_sub(previous.unclassified_tokens),
        },
        reset,
    )
}

pub(super) fn records(
    agent: &str,
    files: &BTreeMap<PathBuf, FileCache>,
) -> (Vec<Sample>, bool, Vec<String>) {
    if agent == "codex" {
        return codex(files);
    }
    if agent == "opencode" {
        return super::opencode::records(files);
    }
    let mut partial = false;
    let mut out = Vec::new();
    let mut warnings = Vec::new();
    for (path, file) in files {
        let mut root = path.clone();
        if agent == "pi" {
            let mut visited = HashSet::new();
            while let Some(parent) = parent_path(&root, &files[&root], files) {
                if !visited.insert(root.clone()) {
                    partial = true;
                    warnings.push("Pi 分叉历史存在循环，保留可确认用量".into());
                    break;
                }
                root = parent.clone();
            }
        }
        for sample in &file.samples {
            let mut sample = sample.clone();
            sample.key = canonical_key(agent, &sample.key);
            if agent == "pi" && !sample.key.starts_with("response:") {
                // Pi 的 session ID 在移动/恢复文件时不变；短 entry ID 仅在同一父会话链内唯一。
                let parent = &files[&root];
                let identity = if parent.state.session.is_empty() {
                    parent
                        .origin_path
                        .as_deref()
                        .unwrap_or(&root)
                        .to_string_lossy()
                        .into_owned()
                } else {
                    parent.state.session.clone()
                };
                sample.key = format!("{}:{}", identity, sample.key);
            }
            partial |= sample.partial;
            out.push(sample);
        }
    }
    (unique(out), partial, warnings)
}

pub(super) fn segment_id(file: &FileCache) -> String {
    serde_json::to_string(&(
        &file.state.session,
        &file.state.history_base,
        &file.state.parent_session,
        file.state.parent_before,
        file.state
            .parent_session
            .as_ref()
            .and(file.state.created_at),
    ))
    .unwrap_or_default()
}

fn merged_observations(sources: &[(&PathBuf, &FileCache)]) -> Option<Vec<Sample>> {
    let mut positions = HashMap::<String, usize>::new();
    let mut samples = Vec::<Sample>::new();
    let mut edges = Vec::<HashSet<usize>>::new();
    let mut incoming = Vec::<usize>::new();
    for (_, file) in sources {
        let mut previous: Option<usize> = None;
        for (id, sample) in super::ledger::observation_ids(&file.samples)
            .into_iter()
            .zip(&file.samples)
        {
            let index = if let Some(&i) = positions.get(&id) {
                let mut details = samples[i].details.clone();
                details.refine(&sample.details);
                let mut request = samples[i].request.clone();
                request.merge(&sample.request);
                if prefer(sample, &samples[i]) {
                    samples[i] = sample.clone();
                }
                samples[i].details = details;
                samples[i].request = request;
                i
            } else {
                let i = samples.len();
                positions.insert(id, i);
                samples.push(sample.clone());
                edges.push(HashSet::new());
                incoming.push(0);
                i
            };
            if let Some(before) = previous {
                if edges[before].insert(index) {
                    incoming[index] += 1;
                }
            }
            previous = Some(index);
        }
    }
    // 保留每份原始观测的顺序（包括 reset），只给互不重叠的片段按时间排序。
    let order = |i: usize| (samples[i].timestamp, samples[i].ordinal, i);
    let mut ready: std::collections::BTreeSet<_> = incoming
        .iter()
        .enumerate()
        .filter(|(_, n)| **n == 0)
        .map(|(i, _)| order(i))
        .collect();
    let mut result = Vec::new();
    while let Some((_, _, i)) = ready.pop_first() {
        let mut sample = samples[i].clone();
        sample.offset = result.len() as u64;
        result.push(sample);
        for &next in &edges[i] {
            incoming[next] -= 1;
            if incoming[next] == 0 {
                ready.insert(order(next));
            }
        }
    }
    (result.len() == samples.len()).then_some(result)
}

fn codex_segments(files: &BTreeMap<PathBuf, FileCache>) -> BTreeMap<PathBuf, FileCache> {
    let mut groups = BTreeMap::<String, Vec<_>>::new();
    for (path, file) in files {
        groups
            .entry(segment_id(file))
            .or_default()
            .push((path, file));
    }
    let mut result = BTreeMap::new();
    for mut sources in groups.into_values() {
        sources.sort_by_key(|(_, f)| std::cmp::Reverse(f.samples.len()));
        let (path, file) = sources[0];
        if sources.len() > 1 {
            if let Some(samples) = merged_observations(&sources) {
                let mut merged = file.clone();
                merged.samples = samples;
                result.insert(path.clone(), merged);
                continue;
            }
        }
        result.extend(sources.into_iter().map(|(p, f)| (p.clone(), f.clone())));
    }
    result
}

fn codex(files: &BTreeMap<PathBuf, FileCache>) -> (Vec<Sample>, bool, Vec<String>) {
    let mut out = Vec::new();
    let mut partial = false;
    let mut warnings = Vec::new();
    let segments = codex_segments(files);
    let covered = covered_requests(&segments);
    // 新版 rollout 逐响应记录有稳定 response_id。它们比累计通知更直接，并可跨 fork/page 去重。
    for file in segments.values() {
        out.extend(
            file.samples
                .iter()
                .filter(|s| s.response_id.is_some())
                .cloned(),
        );
    }
    for (path, file) in &segments {
        let mut previous = baseline(path, file, files, &mut HashSet::new());
        let parent = parent_path(path, file, files).and_then(|p| files.get(p));
        let parent_recorded = parent.is_some_and(|p| !p.samples.is_empty());
        // 分页通过 history_base 建立精确基线，不套用 fork 的时间回放启发式。
        let fork = file.state.parent_session.is_some() && file.state.history_base.is_none();
        let replayed_prefix = parent
            .filter(|_| fork)
            .map(|p| replayed_prefix(file, p))
            .unwrap_or_default();
        let rewritten_burst = if fork && parent_recorded {
            rewritten_burst(file)
        } else {
            HashSet::new()
        };
        let inherited: HashSet<String> = parent
            .into_iter()
            .flat_map(|f| f.samples.iter().filter(|s| s.cumulative))
            .map(|s| signature(s.timestamp, &s.tokens))
            .collect();
        let mut own_started = false;
        // 同一响应可能在一个物理文件中重放；先按 response_id 合并，防止抵扣两次。
        let details = unique(
            file.samples
                .iter()
                .filter(|s| s.response_id.is_some())
                .cloned(),
        );
        let snapshots: Vec<_> = file.samples.iter().filter(|s| s.cumulative).collect();
        let measured_by_snapshot = assign_details(&snapshots, &details);
        let mut snapshot_index = 0;
        let mut occurrences = HashMap::<String, u64>::new();
        for sample in file
            .samples
            .iter()
            .filter(|s| s.cumulative || s.response_id.is_none())
        {
            if !sample.cumulative {
                let mut s = sample.clone();
                s.key = format!("legacy:{}:{}:{}", s.session, s.timestamp, s.offset);
                s.partial = true;
                partial = true;
                out.push(s);
                continue;
            }
            let measured = measured_by_snapshot
                .get(&snapshot_index)
                .cloned()
                .unwrap_or_default();
            snapshot_index += 1;
            let replay = parent_recorded
                && !own_started
                && file.state.parent_session.is_some()
                && (inherited.contains(&signature(sample.timestamp, &sample.tokens))
                    || file
                        .state
                        .created_at
                        .is_some_and(|t| sample.timestamp > 0 && sample.timestamp < t));
            // subagent_history_start_ordinal 在部分旧日志中会推进到所有已记录调用之后，
            // 不能用它排除子线程用量；只排除有父计量佐证的重放时间/请求前缀。
            if replay
                || replayed_prefix.contains(&sample.offset)
                || rewritten_burst.contains(&sample.offset)
            {
                previous = Some(sample.tokens.clone());
                continue;
            }
            own_started = true;
            let start = previous.clone();
            if start.is_none() {
                partial = true;
                warnings.push("Codex 历史基线缺失，已保留逐响应记录及可确认增量".into());
            }
            let counter_reset = start.as_ref().is_some_and(|previous| {
                sample.tokens.input_tokens < previous.input_tokens
                    || sample.tokens.output_tokens < previous.output_tokens
                    || sample.tokens.total() < previous.total()
            });
            if counter_reset && sample.last_usage.is_none() {
                partial = true;
                warnings.push("Codex 累计计数曾重置，已保留逐响应记录及可确认增量".into());
                previous = Some(sample.tokens.clone());
                continue;
            }
            let (mut increment, reset) = if start.as_ref() == Some(&sample.tokens) {
                (Tokens::default(), false)
            } else if let Some(last) = &sample.last_usage {
                // last 是单次请求计量；累计值可能跨压缩/分页继承旧范围，不能用其差额替代已知的 last。
                (last.clone(), counter_reset)
            } else if let Some(start) = &start {
                delta(&sample.tokens, start)
            } else {
                (sample.last_usage.clone().unwrap_or_default(), true)
            };
            let mut uncertain = sample.partial || reset;
            if reset {
                // 不把负增量静默清零，也不把重置后的累计值当成一次新调用。
                partial = true;
            }
            let upper = sample.tokens.total();
            if increment.total() == 0 && upper > 0 {
                previous = Some(sample.tokens.clone());
                continue;
            }
            let identity = counter_identity(sample);
            let occurrence = occurrences.entry(identity.clone()).or_default();
            *occurrence += 1;
            let counter_key = counter_key(file, &identity, *occurrence);
            if covered.contains(&counter_key) {
                previous = Some(sample.tokens.clone());
                continue;
            }
            // 累计变化里已由逐响应明细覆盖的部分不能再加。旧格式无明细时保留全部增量。
            if measured.total() > 0 {
                let (remaining, conflict) = delta(&increment, &measured);
                increment = remaining;
                uncertain |= conflict;
                partial |= conflict;
            }
            if increment.total() > 0 || (snapshot_index == 1 && upper == 0) {
                let mut record = sample.clone();
                // 原始累计观测的身份不随计算出的差额变化。
                record.key = counter_key;
                record.tokens = increment;
                // 没有逐响应 ID 的剩余累计差额可能跨多次调用，不提供虚假的 TPS。
                if measured.total() > 0 || sample.last_usage.is_none() {
                    record.request.scope = Some("usage_delta".into());
                }
                record.cumulative = false;
                record.partial = uncertain;
                out.push(record);
            }
            previous = Some(sample.tokens.clone());
        }
    }
    let result = unique(out);
    partial |= result.iter().any(|s| s.partial);
    (result, partial, warnings)
}

fn replayed_prefix(child: &FileCache, parent: &FileCache) -> HashSet<u64> {
    if child.state.parent_before == Some(0) {
        return HashSet::new();
    }
    fn frames(file: &FileCache) -> Vec<&Sample> {
        let mut previous = None;
        file.samples
            .iter()
            .filter(|s| s.cumulative)
            .filter(|s| {
                let changed = previous != Some(&s.tokens);
                previous = Some(&s.tokens);
                changed
            })
            .collect()
    }
    let own = frames(child);
    let inherited = frames(parent);
    let mut offsets = HashSet::new();
    for (a, b) in own.iter().zip(&inherited) {
        let a_usage = a.last_usage.as_ref().unwrap_or(&a.tokens);
        let b_usage = b.last_usage.as_ref().unwrap_or(&b.tokens);
        let models_match = a.model == b.model || a.model.is_empty() || b.model.is_empty();
        if !models_match || a_usage != b_usage {
            break;
        }
        offsets.insert(a.offset);
    }
    // 旧版 fork 会以新时间重放完整请求序列，单靠 timestamp 无法去重；至少两个连续请求吻合才采用此兼容规则。
    if offsets.len() < 2 {
        offsets.clear();
    }
    offsets
}

fn rewritten_burst(file: &FileCache) -> HashSet<u64> {
    // 参考 ccusage 的 1 秒回放间隔：旧 fork 会将复制事件集中改写为创建时刻。
    // 仅用于父记录仍在、首条紧邻创建时刻的日志；真实调用出现间隔后立即停止。
    let mut snapshots = file.samples.iter().filter(|s| s.cumulative);
    let (Some(first), Some(created)) = (snapshots.next(), file.state.created_at) else {
        return HashSet::new();
    };
    if !(0..=1000).contains(&first.timestamp.saturating_sub(created)) {
        return HashSet::new();
    }
    let mut offsets = HashSet::from([first.offset]);
    let mut previous = first;
    let mut distinct = 1;
    for sample in snapshots {
        if !(0..=1000).contains(&sample.timestamp.saturating_sub(previous.timestamp)) {
            break;
        }
        if sample.tokens != previous.tokens {
            distinct += 1;
        }
        offsets.insert(sample.offset);
        previous = sample;
    }
    // 两次相同的额度通知不能证明存在复制历史。
    if distinct < 2 {
        offsets.clear();
    }
    offsets
}

fn counter_identity(sample: &Sample) -> String {
    serde_json::to_string(&(
        &sample.model,
        sample.timestamp,
        sample.ordinal,
        &sample.tokens,
        &sample.last_usage,
    ))
    .unwrap_or_default()
}

fn counter_key(file: &FileCache, identity: &str, occurrence: u64) -> String {
    let segment = if file.state.parent_session.is_some() || file.state.history_base.is_some() {
        segment_id(file)
    } else {
        String::new()
    };
    format!("counter:{segment}:{identity}:{occurrence}")
}

fn covered_requests(files: &BTreeMap<PathBuf, FileCache>) -> HashSet<String> {
    let mut result = HashSet::new();
    for file in files.values() {
        let mut candidates = HashMap::<String, Vec<(&Sample, String)>>::new();
        let mut previous = None;
        let mut occurrences = HashMap::<String, u64>::new();
        for s in file.samples.iter().filter(|s| s.cumulative) {
            if previous != Some(&s.tokens) {
                if let Some(last) = &s.last_usage {
                    let identity = counter_identity(s);
                    let n = occurrences.entry(identity.clone()).or_default();
                    *n += 1;
                    let key = counter_key(file, &identity, *n);
                    candidates
                        .entry(signature(0, last))
                        .or_default()
                        .push((s, key));
                }
            }
            previous = Some(&s.tokens);
        }
        let mut used = HashSet::new();
        for detail in unique(
            file.samples
                .iter()
                .filter(|s| s.response_id.is_some())
                .cloned(),
        ) {
            let Some(entries) = candidates.get(&signature(0, &detail.tokens)) else {
                continue;
            };
            if let Some((s, key)) = entries
                .iter()
                .filter(|(s, _)| !used.contains(&s.offset))
                .min_by_key(|(s, _)| {
                    (
                        s.timestamp.abs_diff(detail.timestamp),
                        s.offset.abs_diff(detail.offset),
                    )
                })
            {
                used.insert(s.offset);
                result.insert(key.clone());
            }
        }
    }
    result
}
fn assign_details(snapshots: &[&Sample], details: &[Sample]) -> HashMap<usize, Tokens> {
    let mut exact = HashMap::<String, Vec<usize>>::new();
    for (index, sample) in snapshots.iter().enumerate() {
        exact
            .entry(signature(0, &sample.tokens))
            .or_default()
            .push(index);
    }
    let mut assigned = HashMap::<usize, Tokens>::new();
    for detail in details {
        let Some(anchor) = &detail.thread_total else {
            continue;
        };
        // 明细可能先于或晚于累计通知；优先匹配同一累计快照的最近位置，不能跨计数重置周期抵扣。
        let index = exact
            .get(&signature(0, anchor))
            .and_then(|indexes| {
                indexes
                    .iter()
                    .min_by_key(|i| {
                        let i = **i;
                        let advancing = i == 0
                            || snapshots[i].tokens.total() != snapshots[i - 1].tokens.total();
                        (!advancing, snapshots[i].offset.abs_diff(detail.offset))
                    })
                    .copied()
            })
            .or_else(|| {
                snapshots
                    .iter()
                    .enumerate()
                    .find(|(index, sample)| {
                        let previous = if *index > 0 {
                            snapshots[*index - 1].tokens.total()
                        } else {
                            0
                        };
                        sample.last_usage.is_none()
                            && sample.offset >= detail.offset
                            && anchor.total() > previous
                            && anchor.total() <= sample.tokens.total()
                    })
                    .map(|(i, _)| i)
            });
        if let Some(index) = index {
            assigned.entry(index).or_default().add(&detail.tokens);
        }
    }
    assigned
}
fn signature(timestamp: i64, tokens: &Tokens) -> String {
    format!(
        "{timestamp}:{}:{}:{}:{}:{}:{}",
        tokens.input_tokens,
        tokens.output_tokens,
        tokens.cache_read_tokens,
        tokens.cache_write_tokens,
        tokens.reasoning_tokens,
        tokens.unclassified_tokens
    )
}
