use super::{
    cache::FileCache,
    parse::{normalize, recorded_fields, text, timestamp},
    request_parse::{string, RequestError, RequestMeta, RequestOutcome},
    types::Sample,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub(super) struct Context {
    project: Option<String>,
    provider: Option<String>,
    effort: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    links: Vec<EntryLink>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    unlinked: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct EntryLink {
    offset: u64,
    id: String,
    parent: Option<String>,
}

impl Context {
    fn add_link(&mut self, value: &Value, offset: u64) {
        let parent = match value.get("parentId") {
            Some(Value::Null) => Some(None),
            Some(Value::String(id)) if !id.is_empty() => Some(Some(id.clone())),
            _ => None,
        };
        if let Some((id, parent)) = text(value.get("id")).zip(parent) {
            self.links.push(EntryLink { offset, id: id.into(), parent });
        } else {
            self.unlinked = true;
        }
    }

    fn active_offsets(&self) -> Option<HashSet<u64>> {
        let links: HashMap<_, _> = self.links.iter().map(|link| (link.id.as_str(), link)).collect();
        if self.unlinked || links.len() != self.links.len()
            || self.links.iter().any(|link| link.parent.as_ref().is_some_and(|id| !links.contains_key(id.as_str())))
        {
            return None;
        }
        let mut active = HashSet::new();
        let mut next = self.links.last();
        while let Some(link) = next {
            if !active.insert(link.offset) {
                return None;
            }
            next = link.parent.as_deref().map(|id| links[id]);
        }
        Some(active)
    }
}

pub(super) fn read_line(value: &Value, offset: u64, path: &Path, file: &mut FileCache) {
    let Some(kind) = value["type"].as_str() else {
        return;
    };
    let state = &mut file.state;
    if kind != "session" {
        state.pi.add_link(value, offset);
    }
    match kind {
        "session" => {
            if let Some(id) = text(value.get("id")) {
                state.session = id.into();
            }
            state.parent_session = string(&value["parentSession"]);
            state.created_at = timestamp(value.get("timestamp")).or(state.created_at);
            state.pi.project = string(&value["cwd"]).or_else(|| state.pi.project.take());
            return;
        }
        "thinking_level_change" => {
            state.pi.effort = string(&value["thinkingLevel"]);
            return;
        }
        _ => {}
    }
    if kind == "model_change" || (kind == "custom" && value["customType"] == "model-snapshot") {
        let data = &value["data"];
        if let Some(model) = text(data.get("modelId"))
            .or_else(|| text(data.get("model")))
            .or_else(|| text(value.get("modelId")))
            .or_else(|| text(value.get("model")))
        {
            state.model = model.into();
        }
        if let Some(provider) = string(&data["provider"]).or_else(|| string(&value["provider"])) {
            state.pi.provider = Some(provider);
        }
        return;
    }

    let (source, request_kind) = match kind {
        "message"
            if matches!(
                value["message"]["role"].as_str(),
                Some("assistant" | "toolResult")
            ) =>
        {
            (&value["message"], None)
        }
        "usage" => (value, string(&value["kind"])),
        "compaction" | "branch_summary" => (value, Some(kind.into())),
        _ => return,
    };
    let assistant = source["role"] == "assistant";
    let stop = string(&source["stopReason"]).or_else(|| string(&source["stop_reason"]));
    let failed = assistant && matches!(stop.as_deref(), Some("error" | "aborted"));
    let measurement = normalize(&source["usage"], "pi");
    if measurement.is_none() && !failed {
        return;
    }
    let provider = string(&source["provider"])
        .or_else(|| string(&value["provider"]))
        .or_else(|| state.pi.provider.clone());
    let raw_model = text(source.get("responseModel"))
        .or_else(|| text(source.get("modelId")))
        .or_else(|| text(source.get("model")))
        .or_else(|| (!state.model.is_empty()).then_some(state.model.as_str()));
    let model = match (provider.as_deref(), raw_model) {
        (Some(provider), Some(model))
            if model
                .split_once('/')
                .is_none_or(|(prefix, _)| prefix != provider) =>
        {
            format!("{provider}/{model}")
        }
        (_, model) => model.unwrap_or("unknown").into(),
    };
    let request_id = string(&source["responseId"]);
    let key = if let Some(id) = &request_id {
        format!("response:{model}:{id}")
    } else {
        format!(
            "entry:{}",
            string(&value["id"]).unwrap_or_else(|| offset.to_string())
        )
    };
    let source_time = timestamp(source.get("timestamp"));
    let event_time = source_time.or_else(|| timestamp(value.get("timestamp")));
    // Pi 在消息内记录请求开始时间，在外层记录会话写入时间。
    let (started_at, finished_at, latency_ms) = if assistant {
        let latency = source["durationMs"].as_i64().filter(|&ms| ms > 0);
        let end = source_time
            .zip(latency)
            .and_then(|(start, ms)| start.checked_add(ms));
        (source_time, end, latency)
    } else {
        (None, event_time, None)
    };
    let mut sample = Sample::unmeasured(
        key,
        &state.session,
        model,
        event_time.or(state.created_at).unwrap_or_default(),
        offset,
    );
    if let Some((tokens, partial)) = measurement {
        sample.tokens = tokens;
        sample.details = recorded_fields(&source["usage"], "pi");
        sample.partial = partial;
    }
    sample.request = RequestMeta {
        request_id: request_id.or_else(|| string(&value["requestId"])),
        provider,
        project: string(&value["cwd"]).or_else(|| state.pi.project.clone()),
        effort: string(&value["effort"]).or_else(|| state.pi.effort.clone()),
        kind: request_kind,
        status: failed.then_some(RequestOutcome::Failed),
        error: failed
            .then(|| RequestError::new(None, source["errorMessage"].as_str()))
            .flatten(),
        stop,
        source: Some(path.to_string_lossy().into_owned()),
        started_at,
        finished_at,
        latency_ms,
        timing: latency_ms.map(|_| "recorded".into()),
        scope: Some("request".into()),
        ..Default::default()
    };
    file.samples.push(sample);
}

fn replay_prefix<'a>(child: &'a FileCache, parent: &'a FileCache) -> HashMap<&'a str, &'a str> {
    let Some(fork_time) = child.state.created_at else { return HashMap::new() };
    let active = if parent.state.pi.links.is_empty() {
        None
    } else if let Some(offsets) = parent.state.pi.active_offsets() {
        Some(offsets)
    } else {
        return HashMap::new();
    };
    if !child.state.pi.links.is_empty() && child.state.pi.active_offsets().is_none() {
        return HashMap::new();
    }
    let mut originals: Vec<_> = parent.samples.iter()
        .filter(|sample| active.as_ref().is_none_or(|offsets| offsets.contains(&sample.offset)))
        .collect();
    originals.sort_unstable_by_key(|sample| sample.offset);
    let mut candidates: Vec<_> = child.samples.iter().collect();
    candidates.sort_unstable_by_key(|sample| sample.offset);
    candidates.into_iter().zip(originals)
        .take_while(|(child, parent)| {
            parent.timestamp > 0 && parent.timestamp <= fork_time
                && child.timestamp == parent.timestamp && child.model == parent.model
                && child.tokens == parent.tokens && child.request.kind == parent.request.kind
                && child.request.stop == parent.request.stop
        })
        .map(|(child, parent)| (child.key.as_str(), parent.key.as_str()))
        .collect()
}

pub(super) fn records(files: &BTreeMap<PathBuf, FileCache>) -> Vec<Sample> {
    let parents: HashMap<_, _> = files.iter().filter_map(|(path, file)| {
        let parent = super::normalize::parent_path(path, file, files)?;
        Some((path, (parent, replay_prefix(file, &files[parent]))))
    }).collect();
    let mut out = Vec::new();
    for (path, file) in files {
        let mut root = path;
        let mut lineage = Vec::new();
        let mut inherited = HashSet::new();
        let mut visited = HashSet::from([root]);
        while let Some((parent, replay)) = parents.get(root) {
            if !visited.insert(*parent) {
                break;
            }
            lineage.push(replay);
            inherited.extend(files[*parent].samples.iter().map(|sample| sample.key.as_str()));
            root = parent;
        }
        let origin = &files[root];
        let identity = if origin.state.session.is_empty() {
            origin.origin_path.as_deref().unwrap_or(root).to_string_lossy()
        } else {
            std::borrow::Cow::Borrowed(origin.state.session.as_str())
        };
        for sample in &file.samples {
            let mut record = sample.clone();
            if !sample.key.starts_with("response:") {
                let key = if inherited.contains(sample.key.as_str()) {
                    sample.key.as_str()
                } else {
                    lineage.iter().fold(sample.key.as_str(), |key, replay| {
                        replay.get(key).copied().unwrap_or(key)
                    })
                };
                record.replayed |= inherited.contains(key);
                record.key = if key.starts_with("response:") {
                    key.to_owned()
                } else {
                    format!("{identity}:{key}")
                };
            } else {
                record.replayed |= inherited.contains(sample.key.as_str());
            }
            out.push(record);
        }
    }
    super::normalize::unique("pi", out)
}
