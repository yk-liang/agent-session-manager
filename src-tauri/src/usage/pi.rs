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
pub(super) struct Context {
    project: Option<String>,
    provider: Option<String>,
    effort: Option<String>,
}

pub(super) fn read_line(value: &Value, offset: u64, path: &Path, file: &mut FileCache) {
    let Some(kind) = value["type"].as_str() else {
        return;
    };
    let state = &mut file.state;
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

    let source = match kind {
        "message"
            if matches!(
                value["message"]["role"].as_str(),
                Some("assistant" | "toolResult")
            ) =>
        {
            &value["message"]
        }
        "usage" | "compaction" | "branch_summary" => value,
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
    let finished_at =
        timestamp(source.get("timestamp")).or_else(|| timestamp(value.get("timestamp")));
    let mut sample = Sample::unmeasured(
        key,
        &state.session,
        model,
        finished_at.or(state.created_at).unwrap_or_default(),
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
        kind: matches!(kind, "compaction" | "branch_summary").then(|| kind.into()),
        status: failed.then_some(RequestOutcome::Failed),
        error: failed
            .then(|| RequestError::new(None, source["errorMessage"].as_str()))
            .flatten(),
        stop,
        source: Some(path.to_string_lossy().into_owned()),
        finished_at,
        scope: Some("request".into()),
        ..Default::default()
    };
    file.samples.push(sample);
}

pub(super) fn records(files: &BTreeMap<PathBuf, FileCache>) -> Vec<Sample> {
    let mut out = Vec::new();
    for (path, file) in files {
        let mut root = path.clone();
        let mut inherited = HashSet::new();
        let mut inherited_usage = HashMap::<_, Vec<_>>::new();
        let mut occurrences = HashMap::<_, usize>::new();
        let mut visited = HashSet::from([root.clone()]);
        while let Some(parent) = super::normalize::parent_path(&root, &files[&root], files) {
            if !visited.insert(parent.clone()) {
                break;
            }
            let mut positions = HashMap::<_, usize>::new();
            for sample in &files[parent].samples {
                inherited.insert(sample.key.as_str());
                if sample.timestamp > 0
                    && file
                        .state
                        .created_at
                        .is_some_and(|time| sample.timestamp <= time)
                {
                    let key = usage_identity(sample);
                    let index = positions.entry(key.clone()).or_default();
                    let keys = inherited_usage.entry(key).or_default();
                    if keys.len() == *index {
                        keys.push(sample.key.as_str());
                    }
                    *index += 1;
                }
            }
            root = parent.clone();
        }
        for sample in &file.samples {
            let mut sample = sample.clone();
            if sample.timestamp > 0 && !sample.key.starts_with("response:") {
                let identity = usage_identity(&sample);
                let position = occurrences.entry(identity.clone()).or_default();
                if !inherited.contains(sample.key.as_str()) {
                    if let Some(key) = inherited_usage
                        .get(&identity)
                        .and_then(|keys| keys.get(*position))
                    {
                        sample.key = (*key).to_owned();
                    }
                }
                *position += 1;
            }
            sample.replayed |= inherited.contains(sample.key.as_str());
            if !sample.key.starts_with("response:") {
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
            out.push(sample);
        }
    }
    super::normalize::unique("pi", out)
}

fn usage_identity(sample: &Sample) -> String {
    serde_json::to_string(&(
        sample.timestamp,
        &sample.model,
        &sample.tokens,
        &sample.request.kind,
        &sample.request.stop,
    ))
    .unwrap_or_default()
}
