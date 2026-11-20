//! 累计计数和逐响应明细同时存在时只保留一次；参考 ccusage 的 replay/delta 思路。
use super::{cache::FileCache, types::*};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::{Path, PathBuf},
};

mod codex;

pub(super) use codex::segment_id;

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
pub(super) fn unique(agent: &str, samples: impl IntoIterator<Item = Sample>) -> Vec<Sample> {
    let mut by_key = HashMap::<String, Sample>::new();
    for mut s in samples {
        if let Some(old) = by_key.get_mut(&s.key) {
            if agent == "claude" && old.claude_fallback != s.claude_fallback {
                if s.claude_fallback {
                    s.request.merge_claude_start(&old.request);
                    *old = s;
                } else {
                    old.request.merge_claude_start(&s.request);
                }
                continue;
            }
            let copy_fingerprint = s
                .copy_fingerprint
                .clone()
                .or_else(|| old.copy_fingerprint.clone());
            let mut details = old.details.clone();
            details.refine(&s.details);
            let mut identities = old.request_identities.clone();
            identities.merge(&s.request_identities);
            let mut request = old.request.clone();
            if old.replayed && !s.replayed {
                request = s.request.clone();
            } else if !s.replayed || old.replayed {
                if s.response_id.is_some() {
                    request.merge_codex(&s.request);
                } else {
                    request.merge(&s.request);
                }
            }
            let preferred = if matches!(agent, "claude" | "qoder") {
                let rank = |sample: &Sample| {
                    (
                        !sample.replayed,
                        sample.complete,
                        sample.tokens.output_tokens,
                        !sample.partial,
                        std::cmp::Reverse(sample.timestamp),
                    )
                };
                rank(&s) > rank(old)
            } else {
                prefer(&s, old)
            };
            if preferred {
                std::mem::swap(old, &mut s);
            }
            if matches!(agent, "claude" | "qoder") && s.model == old.model {
                old.tokens.refine_prompt(&s.tokens, agent == "claude");
            }
            old.copy_fingerprint = copy_fingerprint;
            old.details = details;
            old.request = request;
            old.request_identities = identities;
        } else {
            by_key.insert(s.key.clone(), s);
        }
    }
    by_key.into_values().collect()
}
pub(super) fn physical_id(path: &Path) -> String {
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
                && (id.starts_with("resp_") || super::claude::is_global_message_id(id))
            {
                return format!("message:{id}");
            }
        }
    }
    key.to_owned()
}

fn resolved_parent(origin: &Path, parent: &str) -> PathBuf {
    use std::path::Component;
    let path = origin.parent().unwrap_or(Path::new("")).join(parent);
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir if result.file_name().is_some_and(|name| name != "..") => {
                result.pop();
            }
            _ => result.push(component),
        }
    }
    result
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
        for parent in file
            .state
            .parent_session
            .iter()
            .chain(&file.state.forked_from)
        {
            keys.extend([
                format!("session:{parent}"),
                format!("physical:{parent}"),
                format!("file:{parent}"),
                format!("file:{}", resolved_parent(origin, parent).display()),
            ]);
        }
    }
    for sample in file.samples.iter().chain(&file.failures) {
        // Pi 的短 entry ID 只在父会话链内唯一，不能用它连接无关会话。
        if !sample.key.is_empty() && (agent != "pi" || sample.key.starts_with("response:")) {
            keys.insert(format!("request:{}", canonical_key(agent, &sample.key)));
        }
        if agent == "codex" && sample.cumulative {
            keys.insert(format!("counter:{}", codex::counter_identity(sample)));
        }
        if let Some(fingerprint) = &sample.copy_fingerprint {
            keys.insert(format!("copy:{fingerprint}"));
        }
        keys.extend(
            sample
                .request_identities
                .keys
                .iter()
                .map(|key| format!("request:{key}")),
        );
    }
    keys
}
pub(super) fn parent_path<'a>(
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
    let id = file
        .state
        .forked_from
        .as_deref()
        .or(file.state.parent_session.as_deref())?;
    let exact = PathBuf::from(id);
    let relative = resolved_parent(file.origin_path.as_deref().unwrap_or(path), id);
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
pub(super) fn records(agent: &str, files: &BTreeMap<PathBuf, FileCache>) -> Vec<Sample> {
    match agent {
        "codex" => codex::records(files),
        "opencode" => super::opencode::records(files),
        "antigravity" => super::antigravity::records(files),
        "pi" => super::pi::records(files),
        _ => unique(
            agent,
            files
                .values()
                .flat_map(|file| file.samples.iter().cloned())
                .map(|mut sample| {
                    sample.key = canonical_key(agent, &sample.key);
                    sample
                }),
        ),
    }
}
