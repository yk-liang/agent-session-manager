use super::{parent_path, physical_id, prefer, unique};
use crate::usage::{
    cache::FileCache,
    codex::{COUNTER_BASELINE, FORK_METADATA_VERSION},
    ledger,
    types::{Sample, Tokens},
};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::{Path, PathBuf},
};

fn codex_parent(file: &FileCache) -> Option<&str> {
    file.state.forked_from.as_deref().or_else(|| {
        (file.parser_version < FORK_METADATA_VERSION)
            .then_some(file.state.parent_session.as_deref())
            .flatten()
    })
}

fn baseline(
    path: &Path,
    file: &FileCache,
    files: &BTreeMap<PathBuf, FileCache>,
    seen: &mut HashSet<PathBuf>,
) -> Option<Tokens> {
    if file.state.history_base.is_none() && codex_parent(file).is_none() {
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
        fork_history(path, file, files)
            .into_iter()
            .filter_map(|s| value(s).map(|v| ((s.timestamp, s.ordinal, s.offset), v)))
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
            cache_write_1h_tokens: current
                .cache_write_1h_tokens
                .saturating_sub(previous.cache_write_1h_tokens),
        },
        reset,
    )
}

pub(in crate::usage) fn segment_id(file: &FileCache) -> String {
    let parent = file
        .state
        .forked_from
        .as_ref()
        .or(file.state.parent_session.as_ref());
    serde_json::to_string(&(
        &file.state.session,
        &file.state.history_base,
        parent,
        file.state.parent_before,
        parent.and(file.state.created_at),
    ))
    .unwrap_or_default()
}

fn merged_observations(sources: &[(&PathBuf, &FileCache)]) -> Option<Vec<Sample>> {
    let mut positions = HashMap::<String, usize>::new();
    let mut samples = Vec::<Sample>::new();
    let mut parser_versions = Vec::new();
    let mut edges = Vec::<HashSet<usize>>::new();
    let mut incoming = Vec::<usize>::new();
    for (_, file) in sources {
        let mut previous: Option<usize> = None;
        for (id, sample) in ledger::sources::observation_ids(&file.samples)
            .into_iter()
            .zip(&file.samples)
        {
            let index = if let Some(&i) = positions.get(&id) {
                if file.parser_version > parser_versions[i] {
                    samples[i] = sample.clone();
                    parser_versions[i] = file.parser_version;
                } else if file.parser_version == parser_versions[i] {
                    let mut details = samples[i].details.clone();
                    details.refine(&sample.details);
                    let mut request = samples[i].request.clone();
                    request.merge_codex(&sample.request);
                    if prefer(sample, &samples[i]) {
                        samples[i] = sample.clone();
                    }
                    samples[i].details = details;
                    samples[i].request = request;
                }
                i
            } else {
                let i = samples.len();
                positions.insert(id, i);
                samples.push(sample.clone());
                parser_versions.push(file.parser_version);
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

pub(super) fn records(files: &BTreeMap<PathBuf, FileCache>) -> Vec<Sample> {
    let mut out = Vec::new();
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
        let history = fork_history(path, file, &segments);
        let replayed_prefix = replayed_prefix(file, &history);
        let inherited: HashSet<_> = history
            .iter()
            .filter(|s| s.cumulative)
            .map(|s| signature(s.timestamp, &s.tokens))
            .collect();
        let mut own_started = false;
        // 同一响应可能在一个物理文件中重放；先按 response_id 合并，防止抵扣两次。
        let details = unique(
            "codex",
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
                out.push(s);
                continue;
            }
            let measured = measured_by_snapshot
                .get(&snapshot_index)
                .cloned()
                .unwrap_or_default();
            snapshot_index += 1;
            if sample.request.scope.as_deref() == Some(COUNTER_BASELINE) {
                previous = Some(sample.tokens.clone());
                continue;
            }
            let inherited_at = file
                .state
                .created_at
                .is_some_and(|t| sample.timestamp > 0 && sample.timestamp < t);
            if replayed_prefix.contains(&sample.offset)
                || !own_started
                    && (inherited.contains(&signature(sample.timestamp, &sample.tokens))
                        || !history.is_empty() && inherited_at)
            {
                previous = Some(sample.tokens.clone());
                continue;
            }
            own_started = true;
            let mut uncertain = sample.partial;
            let mut increment = if previous.as_ref() == Some(&sample.tokens) {
                Tokens::default()
            } else if let Some(last) = &sample.last_usage {
                // 单次请求计量与父线程基线和累计计数重置无关。
                last.clone()
            } else if let Some(start) = &previous {
                if sample.tokens.input_tokens < start.input_tokens
                    || sample.tokens.output_tokens < start.output_tokens
                    || sample.tokens.total() < start.total()
                {
                    previous = Some(sample.tokens.clone());
                    continue;
                }
                let (increment, incomplete) = delta(&sample.tokens, start);
                uncertain |= incomplete;
                increment
            } else {
                previous = Some(sample.tokens.clone());
                continue;
            };
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
            }
            if increment.total() > 0 || (snapshot_index == 1 && upper == 0) {
                let mut record = sample.clone();
                // 原始累计观测的身份不随计算出的差额变化。
                record.key = counter_key;
                record.tokens = increment;
                // 没有逐响应 ID 的剩余累计差额可能跨多次调用，不提供虚假的 TPS。
                if measured.total() > 0 || sample.last_usage.is_none() {
                    record.request.scope = Some("usage_delta".into());
                    record.request.clear_timing();
                }
                record.cumulative = false;
                record.partial = uncertain;
                out.push(record);
            }
            previous = Some(sample.tokens.clone());
        }
    }
    unique("codex", out)
}

fn fork_history<'a>(
    path: &Path,
    file: &FileCache,
    files: &'a BTreeMap<PathBuf, FileCache>,
) -> Vec<&'a Sample> {
    let Some(parent) = codex_parent(file).filter(|_| file.state.parent_before != Some(0)) else {
        return Vec::new();
    };
    let mut history: Vec<_> = files
        .iter()
        .filter(|(p, f)| {
            p.as_path() != path && (f.state.session == parent || physical_id(p) == parent)
        })
        .flat_map(|(_, f)| &f.samples)
        .filter(|s| {
            file.state.created_at.is_none_or(|end| s.timestamp <= end)
                && file
                    .state
                    .parent_before
                    .is_none_or(|end| s.ordinal.is_none_or(|n| n < end))
        })
        .collect();
    history.sort_by_key(|s| (s.timestamp, s.ordinal, s.offset));
    history
}

fn counter_steps<'a>(
    samples: impl IntoIterator<Item = &'a Sample>,
) -> impl Iterator<Item = &'a Sample> {
    let mut previous = None;
    samples.into_iter().filter(move |s| {
        let changed = s.cumulative
            && s.request.scope.as_deref() != Some(COUNTER_BASELINE)
            && previous != Some(&s.tokens);
        if s.cumulative {
            previous = Some(&s.tokens);
        }
        changed
    })
}

fn replayed_prefix(child: &FileCache, history: &[&Sample]) -> HashSet<u64> {
    let own: Vec<_> = counter_steps(&child.samples).collect();
    let inherited: Vec<_> = counter_steps(history.iter().copied()).collect();
    let matches = |a: &Sample, b: &Sample| {
        (a.model == b.model || a.model.is_empty() || b.model.is_empty())
            && a.tokens == b.tokens
            && a.last_usage
                .as_ref()
                .zip(b.last_usage.as_ref())
                .is_none_or(|(a, b)| a == b)
    };
    let matched = inherited
        .iter()
        .enumerate()
        .filter_map(|(start, _)| {
            let len = own
                .iter()
                .zip(&inherited[start..])
                .take_while(|(a, b)| matches(a, b))
                .count();
            let first = own.first()?;
            let near_creation = child.state.created_at.is_some_and(|created| {
                (0..=1000).contains(&first.timestamp.saturating_sub(created))
            });
            (len >= 2
                || len == 1
                    && near_creation
                    && first.last_usage.is_some()
                    && first.last_usage == inherited[start].last_usage)
                .then_some(len)
        })
        .max()
        .unwrap_or(0);
    if matched == 0 {
        return HashSet::new();
    }
    let end = own[matched - 1].offset;
    child
        .samples
        .iter()
        .take_while(|s| s.offset <= end)
        .filter(|s| s.cumulative)
        .map(|s| s.offset)
        .collect()
}

pub(super) fn counter_identity(sample: &Sample) -> String {
    serde_json::to_string(&(
        &sample.session,
        sample.timestamp,
        sample.ordinal,
        &sample.tokens,
        &sample.last_usage,
    ))
    .unwrap_or_default()
}

fn counter_key(file: &FileCache, identity: &str, occurrence: u64) -> String {
    let segment = if file.state.parent_session.is_some()
        || file.state.forked_from.is_some()
        || file.state.history_base.is_some()
    {
        segment_id(file)
    } else {
        String::new()
    };
    format!("counter:{segment}:{identity}:{occurrence}")
}

fn covered_requests(files: &BTreeMap<PathBuf, FileCache>) -> HashSet<String> {
    let mut result = HashSet::new();
    for file in files.values() {
        let mut candidates = HashMap::<(u64, u64), Vec<(&Sample, String)>>::new();
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
                        .entry((last.input_tokens, last.output_tokens))
                        .or_default()
                        .push((s, key));
                }
            }
            previous = Some(&s.tokens);
        }
        let mut used = HashSet::new();
        for detail in unique(
            "codex",
            file.samples
                .iter()
                .filter(|s| s.response_id.is_some())
                .cloned(),
        ) {
            let Some(entries) =
                candidates.get(&(detail.tokens.input_tokens, detail.tokens.output_tokens))
            else {
                continue;
            };
            if let Some((s, key)) = entries
                .iter()
                .filter(|(s, _)| {
                    let last = s.last_usage.as_ref().unwrap();
                    !used.contains(&s.offset)
                        && (!s.details.cache_read_recorded
                            || !detail.details.cache_read_recorded
                            || last.cache_read_tokens == detail.tokens.cache_read_tokens)
                        && (!s.details.cache_write_recorded
                            || !detail.details.cache_write_recorded
                            || last.cache_write_tokens == detail.tokens.cache_write_tokens)
                        && (!s.details.reasoning_recorded
                            || !detail.details.reasoning_recorded
                            || last.reasoning_tokens == detail.tokens.reasoning_tokens)
                })
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
