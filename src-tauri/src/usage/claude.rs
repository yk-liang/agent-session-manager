use super::{
    cache::FileCache,
    parse::{normalize, recorded_fields, text, timestamp},
    request_parse::{string, RequestError, RequestMeta, RequestOutcome},
    types::Sample,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(super) struct RequestStart {
    at: Option<i64>,
    model: Option<String>,
    kind: Option<String>,
    effort: Option<String>,
}

pub(super) fn read_line(
    agent: &str,
    outer: &Value,
    offset: u64,
    path: &Path,
    file: &mut FileCache,
) {
    if agent == "claude" && outer["type"] == "api-request" {
        if let Some(id) = string(&outer["id"]) {
            let starts = &mut file.state.claude;
            starts.insert(
                id,
                RequestStart {
                    at: timestamp(outer.get("timestamp")),
                    model: string(&outer["params"]["model"]),
                    kind: string(&outer["querySource"]),
                    effort: string(&outer["params"]["output_config"]["effort"]),
                },
            );
            if starts.len() > 256 {
                if let Some(id) = starts
                    .iter()
                    .min_by_key(|(_, start)| start.at)
                    .map(|(id, _)| id.clone())
                {
                    starts.remove(&id);
                }
            }
        }
        return;
    }
    let nested = (outer["type"] == "progress")
        .then(|| outer.pointer("/data/message"))
        .flatten()
        .filter(|line| line.is_object());
    let line = nested.unwrap_or(outer);
    let message = &line["message"];
    if line["type"] != "assistant" && message["role"] != "assistant" {
        return;
    }
    let field = |name: &str| string(&line[name]).or_else(|| string(&outer[name]));
    let stop = string(&message["stop_reason"]).or_else(|| string(&message["stopReason"]));
    let failed =
        line["isApiErrorMessage"] == true || matches!(stop.as_deref(), Some("error" | "aborted"));
    let model = text(message.get("model"));
    if model == Some("<synthetic>") && !failed {
        return;
    }

    let parent = path.ancestors().find(|dir| dir.ends_with("subagents"));
    if let Some(id) = string(&outer["sessionId"]) {
        file.state.session = id;
    } else if file.state.session.is_empty() {
        file.state.session = parent
            .and_then(Path::parent)
            .and_then(Path::file_name)
            .or_else(|| path.file_stem())
            .map(|id| id.to_string_lossy().into_owned())
            .unwrap_or_default();
    }
    let session = field("sessionId").unwrap_or_else(|| file.state.session.clone());
    let usage = &message["usage"];
    let request_id = text(line.get("requestId"))
        .or_else(|| text(line.get("requestRef")))
        .or_else(|| text(usage.get("request_id")));
    let message_id = text(message.get("id"));
    let fallback = (agent == "claude")
        .then(|| {
            usage["iterations"]
                .as_array()?
                .iter()
                .rfind(|item| item["type"] == "fallback_message")
        })
        .flatten();
    let measured_usage = fallback
        .map(iteration_usage)
        .filter(|usage| normalize(usage, agent).is_some())
        .unwrap_or(usage);
    let measurement = normalize(measured_usage, agent);
    if measurement.is_none() && !failed {
        return;
    }

    let request_ref = string(&line["requestRef"]).or_else(|| request_id.map(str::to_owned));
    let start = request_ref
        .as_ref()
        .and_then(|id| file.state.claude.get(id));
    let finished_at =
        timestamp(line.get("timestamp")).or_else(|| timestamp(outer.get("timestamp")));
    let key = key(line, &session, offset, finished_at, message_id, request_id);
    let started_at = start
        .and_then(|start| start.at)
        .filter(|at| finished_at.is_some_and(|end| *at <= end));
    let sidechain = nested.is_some() || line["isSidechain"] == true || parent.is_some();
    let error = if line["isApiErrorMessage"] == true {
        let content = &message["content"];
        let message = content
            .as_array()
            .into_iter()
            .flatten()
            .filter(|block| block["type"] == "text")
            .filter_map(|block| block["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n");
        RequestError::new(
            line["error"].as_str(),
            content
                .as_str()
                .or_else(|| (!message.is_empty()).then_some(message.as_str())),
        )
    } else if failed {
        RequestError::new(line["error"].as_str(), line["error"]["message"].as_str())
    } else {
        None
    };
    let mut request = RequestMeta {
        request_id: request_ref,
        provider: string(&message["provider"]),
        project: field("cwd"),
        client: field("entrypoint"),
        version: field("version"),
        kind: start
            .and_then(|start| start.kind.clone())
            .or_else(|| sidechain.then(|| "sidechain".into())),
        effort: field("effort").or_else(|| start.and_then(|start| start.effort.clone())),
        tier: string(&usage["speed"])
            .filter(|s| s != "standard")
            .or_else(|| string(&usage["service_tier"]).filter(|s| s != "standard")),
        status: failed.then_some(RequestOutcome::Failed),
        error,
        stop,
        http_status: line["apiErrorStatus"]
            .as_u64()
            .and_then(|n| u16::try_from(n).ok()),
        source: Some(path.to_string_lossy().into_owned()),
        started_at,
        finished_at,
        timing: started_at.map(|_| "observed".into()),
        scope: Some("request".into()),
        ..Default::default()
    };
    request.duration();
    let model = fallback
        .and_then(|item| text(item.get("model")))
        .or(model)
        .filter(|model| *model != "<synthetic>")
        .map(str::to_owned)
        .or_else(|| field("requestedModel"))
        .or_else(|| start.and_then(|start| start.model.clone()))
        .unwrap_or_else(|| "unknown".into());
    let mut sample = Sample::unmeasured(
        key,
        &session,
        model,
        finished_at.unwrap_or_default(),
        offset,
    );
    sample.request = request;
    sample.replayed = sidechain;
    let Some((tokens, partial)) = measurement else {
        file.failures.push(sample);
        return;
    };
    sample.tokens = tokens;
    sample.details = recorded_fields(measured_usage, agent);
    sample.partial = partial || message_id.is_none() && request_id.is_none();
    sample.complete = sample.request.stop.is_some();
    if agent == "claude" {
        sample.claude_fallback = fallback.is_some();
        read_iterations(usage, &sample, &mut file.samples);
    }
    file.samples.push(sample);
}

fn iteration_usage(item: &Value) -> &Value {
    item.get("usage").unwrap_or(item)
}

fn read_iterations(usage: &Value, primary: &Sample, samples: &mut Vec<Sample>) {
    let Some(iterations) = usage["iterations"].as_array() else {
        return;
    };
    let mut advisor_index = 0;
    let mut fallback_index = 0;
    for item in iterations {
        let Some(model) = string(&item["model"]) else {
            continue;
        };
        let usage = iteration_usage(item);
        let Some((tokens, partial)) = normalize(usage, "claude") else {
            continue;
        };
        let (kind, index) = match item["type"].as_str() {
            Some("advisor_message") => ("advisor", &mut advisor_index),
            Some("message") if primary.claude_fallback && tokens.total() > 0 => {
                ("fallback", &mut fallback_index)
            }
            _ => continue,
        };
        let mut record = primary.clone();
        record.key = format!("{}:{kind}:{index}", primary.key);
        record.model = model;
        record.tokens = tokens;
        record.details = recorded_fields(usage, "claude");
        record.partial = partial;
        record.request.kind = Some(kind.into());
        record.claude_fallback = false;
        record.request.status = None;
        record.request.effort = None;
        record.request.stop = None;
        record.request.error = None;
        record.request.http_status = None;
        record.timestamp = primary.request.started_at.unwrap_or(primary.timestamp);
        record.request.clear_timing();
        record.request.finished_at = None;
        samples.push(record);
        *index += 1;
    }
}

pub(super) fn is_global_message_id(id: &str) -> bool {
    id.starts_with("msg_")
        || id.starts_with("resp_")
        || id.starts_with("chatcmpl")
        || id.len() == 36
            && id.split('-').map(str::len).eq([8, 4, 4, 4, 12])
            && id.bytes().all(|c| c == b'-' || c.is_ascii_hexdigit())
}

fn key(
    line: &Value,
    session: &str,
    offset: u64,
    at: Option<i64>,
    message: Option<&str>,
    request: Option<&str>,
) -> String {
    match (message, request) {
        (Some(id), _) if is_global_message_id(id) => format!("message:{id}"),
        (Some(id), Some(request)) => format!("request:{request}:{id}"),
        (Some(id), None) => {
            let occurrence = at
                .map(|at| at.to_string())
                .or_else(|| string(&line["uuid"]))
                .unwrap_or_else(|| offset.to_string());
            format!("session:{session}:message:{id}:{occurrence}")
        }
        (None, Some(request)) => format!("request:{request}"),
        _ => format!(
            "session:{session}:event:{}",
            string(&line["uuid"]).unwrap_or_else(|| offset.to_string())
        ),
    }
}
