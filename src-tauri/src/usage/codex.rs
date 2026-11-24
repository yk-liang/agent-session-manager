use super::{
    cache::FileCache,
    parse::{normalize, recorded_fields, text, timestamp},
    request_parse::{at, string, RequestError, RequestMeta, RequestOutcome},
    types::{ParserState, Sample, Tokens},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;

pub(super) mod failures;

pub(in crate::usage) const BOUNDARY_ESTIMATE: &str = "codex_boundary";
pub(super) const COUNTER_BASELINE: &str = "counter_baseline";
// 解析版本 4 首次区分分叉历史与子智能体归属。
pub(super) const FORK_METADATA_VERSION: u8 = 4;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub(in crate::usage) struct RequestState {
    context: RequestMeta,
    metadata_seen: bool,
    counter: Option<Tokens>,
    boundary: Option<i64>,
    last_response: Option<i64>,
    turn_start: Option<i64>,
    turn_id: String,
    first_request: Option<(u64, bool)>,
}

fn read_header(state: &mut ParserState, p: &Value, time: Option<i64>) {
    if state.requests.metadata_seen {
        return;
    }
    state.requests.metadata_seen = true;
    state.session = text(p.get("id"))
        .or_else(|| text(p.get("session_id")))
        .map(str::to_owned)
        .unwrap_or_else(|| state.session.clone());
    state.forked_from = text(p.get("forked_from_id")).map(str::to_owned);
    state.parent_session = text(p.get("parent_thread_id"))
        .or_else(|| text(p.pointer("/source/subagent/thread_spawn/parent_thread_id")))
        .map(str::to_owned);
    state.history_base = p
        .get("history_base")
        .and_then(|v| serde_json::from_value(v.clone()).ok());
    state.parent_before = p
        .get("forked_from_ordinal_exclusive")
        .and_then(Value::as_u64);
    state.created_at = time.or_else(|| timestamp(p.get("timestamp")));
    let context = &mut state.requests.context;
    context.provider = string(&p["model_provider"]);
    context.project = string(&p["cwd"]);
    context.client = string(&p["originator"]);
    context.version = string(&p["cli_version"]);
    let source = &p["source"];
    context.kind = string(source).or_else(|| {
        ["subagent", "exec", "cli", "vscode"]
            .into_iter()
            .find(|key| source.get(*key).is_some())
            .map(str::to_owned)
    });
    context.parent_session_id.clone_from(&state.parent_session);
    context.normalize_codex_kind();
}

pub(super) fn read_line(v: &Value, offset: u64, path: &Path, file: &mut FileCache) {
    let p = &v["payload"];
    let time = at(&v["timestamp"]);
    if v["type"] == "session_meta" {
        read_header(&mut file.state, p, time);
        return;
    }
    let sample = measurement(v, offset, time.unwrap_or(0), &mut file.state);
    let state = &mut file.state.requests;
    match v["type"].as_str() {
        Some("turn_context") => {
            if let Some(model) = codex_model(p) {
                file.state.model = model.into();
            }
            state.context.effort = string(&p["effort"])
                .or_else(|| string(&p["collaboration_mode"]["settings"]["reasoning_effort"]));
            state.context.project = string(&p["cwd"]).or(state.context.project.clone());
        }
        Some("response_item")
            if matches!(p["type"].as_str(), Some(
                "function_call_output" | "custom_tool_call_output" | "local_shell_call_output"
            )) =>
        {
            state.boundary = state.boundary.max(time);
        }
        Some("compacted") => {
            if let Some(id) = compaction_response_id(p) {
                file.state.compactions.insert(id.into());
                for sample in file
                    .samples
                    .iter_mut()
                    .filter(|s| s.response_id.as_deref() == Some(id))
                {
                    sample.request.mark_compaction();
                    if state
                        .first_request
                        .is_some_and(|(offset, _)| offset == sample.offset)
                    {
                        state.first_request = None;
                    }
                }
            }
        }
        Some("event_msg") => match p["type"].as_str() {
            Some("task_started") => {
                state.boundary = time;
                state.last_response = None;
                state.turn_start = time;
                state.turn_id = string(&p["turn_id"]).unwrap_or_default();
                state.first_request = None;
            }
            // 用户输入与工具结果回传分别开启下一次模型调用。
            Some("item_completed")
                if matches!(
                    p["item"]["type"].as_str(),
                    Some("UserMessage" | "user_message")
                ) =>
            {
                state.boundary = state.boundary.max(time).max(at(&p["completed_at_ms"]));
            }
            Some("user_message") => {
                state.boundary = state.boundary.max(time);
            }
            Some("thread_settings_applied") => {
                let s = &p["thread_settings"];
                if let Some(model) = text(s.get("model")) {
                    file.state.model = model.into();
                }
                state.context.provider =
                    string(&s["model_provider_id"]).or(state.context.provider.clone());
                if let Some(tier) = s.get("service_tier") {
                    state.context.tier = string(tier);
                }
            }
            Some("task_complete") => {
                let turn = text(p.get("turn_id"));
                let current = turn == Some(state.turn_id.as_str());
                if let Some(ttft) = p["time_to_first_token_ms"].as_i64().filter(|t| current && *t > 0) {
                    if let Some(sample) = file.samples.iter_mut().rev().find(|s| {
                        state
                            .first_request
                            .is_some_and(|(offset, _)| offset == s.offset)
                    }) {
                        if sample.request.kind.as_deref() != Some("compaction") {
                            let request = &mut sample.request;
                            let within_request = state
                                .turn_start
                                .zip(request.started_at)
                                .and_then(|(turn_start, started_at)| {
                                    turn_start.checked_add(ttft)?.checked_sub(started_at)
                                })
                                .filter(|&ms| {
                                    ms > 0
                                        && request.latency_ms.is_some_and(|duration| ms <= duration)
                                });
                            request.ttft_ms = within_request;
                            request.ttft_scope = within_request.map(|_| "request".into());
                        }
                    }
                }
                let finished_at = time.or_else(|| at(&p["completed_at"]));
                if let Some(mut failure) = turn.and_then(|turn| failures::terminal(
                    &file.state.session, turn, codex_model_or_unknown(&file.state.model),
                    &p["error"], finished_at, offset,
                )) {
                    let mut request = state.context.clone();
                    request.merge(&failure.request);
                    request.finished_at = finished_at;
                    failure.request = request;
                    failure.request.source = Some(path.to_string_lossy().into_owned());
                    file.failures.push(failure);
                }
                if current {
                    state.turn_id.clear();
                }
            }
            Some("error") => {
                let key = format!(
                    "error:{}:{}:{}",
                    file.state.session,
                    time.unwrap_or(0),
                    offset
                );
                let mut failure = Sample::unmeasured(
                    key,
                    &file.state.session,
                    codex_model_or_unknown(&file.state.model).to_owned(),
                    time.unwrap_or(0),
                    offset,
                );
                failure.request = state.context.clone();
                let turn = text(p.get("turn_id")).or_else(|| (!state.turn_id.is_empty()).then_some(state.turn_id.as_str()));
                failures::assign(&mut failure, turn, p);
                failure.request.source = Some(path.to_string_lossy().into_owned());
                file.failures.push(failure);
            }
            _ => {}
        },
        _ => {}
    }
    if let Some(mut sample) = sample {
        if sample.response_id.is_some()
            && file.state.forked_from.is_some()
            && file
                .state
                .created_at
                .is_some_and(|created| sample.timestamp > 0 && sample.timestamp < created)
        {
            sample.replayed = true;
        }
        let mut meta = state.context.clone();
        meta.source = Some(path.to_string_lossy().into_owned());
        meta.request_id = sample.response_id.clone();
        meta.finished_at = time;
        let advance = !sample.cumulative || state.counter.as_ref() != Some(&sample.tokens);
        let request = sample.response_id.is_some() || advance && sample.last_usage.is_some();
        meta.scope = sample
            .request
            .scope
            .take()
            .or_else(|| Some(if request { "request" } else { "usage_delta" }.into()));
        if request {
            meta.started_at = state
                .boundary
                .max(state.last_response)
                .filter(|start| time.is_some_and(|end| end > *start));
            meta.duration();
            meta.timing = meta.latency_ms.map(|_| BOUNDARY_ESTIMATE.into());
        }
        if sample
            .response_id
            .as_deref()
            .is_some_and(|id| file.state.compactions.contains(id))
        {
            meta.mark_compaction();
        }
        sample.request = meta;
        let response = sample.response_id.is_some();
        if request
            && sample.request.kind.as_deref() != Some("compaction")
            && state
                .first_request
                .is_none_or(|(_, first_response)| response && !first_response)
        {
            state.first_request = Some((offset, response));
        }
        if sample.response_id.is_some() {
            state.last_response = state.last_response.max(time);
        }
        if p["type"] == "token_count" {
            if request {
                state.boundary = state.boundary.max(time);
            }
            if sample.cumulative {
                state.counter = Some(sample.tokens.clone());
            }
        }
        file.samples.push(sample);
    }
}

fn codex_model(v: &Value) -> Option<&str> {
    text(v.get("model"))
        .or_else(|| text(v.get("model_id")))
        .or_else(|| text(v.get("model_name")))
        .or_else(|| text(v.pointer("/metadata/model")))
        .or_else(|| text(v.pointer("/metadata/model_id")))
        .or_else(|| text(v.pointer("/metadata/model_name")))
}

fn codex_model_or_unknown(model: &str) -> &str {
    if model.is_empty() { "unknown" } else { model }
}

fn compaction_response_id(payload: &Value) -> Option<&str> {
    text(payload.get("compaction_response_id")).or_else(|| text(payload.get("response_id")))
}

fn measurement(value: &Value, offset: u64, time: i64, state: &mut ParserState) -> Option<Sample> {
    let p = &value["payload"];
    let mut sample = match value["type"].as_str()? {
        "token_usage_record" => {
            let id = text(p.get("response_id"))?;
            let model = codex_model(p).unwrap_or_else(|| codex_model_or_unknown(&state.model)).to_owned();
            let mut sample = Sample::unmeasured(
                format!("response:{id}"),
                &state.session,
                model,
                time,
                offset,
            );
            (sample.tokens, sample.partial) = normalize(&p["usage"], "codex")?;
            sample.details = recorded_fields(&p["usage"], "codex");
            sample.response_id = Some(id.to_owned());
            sample.thread_total =
                normalize(&p["thread_token_usage"], "codex").map(|(tokens, _)| tokens);
            sample
        }
        "event_msg" if p["type"] == "token_count" => {
            let info = &p["info"];
            if let Some(model) = codex_model(p).or_else(|| codex_model(info)) {
                state.model = model.into();
            }
            let cumulative = info["total_token_usage"].is_object();
            let last_usage = &info["last_token_usage"];
            let last = normalize(last_usage, "codex");
            let context_only = last.as_ref().is_some_and(|(tokens, _)| {
                tokens.input_tokens == 0
                    && tokens.output_tokens == 0
                    && tokens.unclassified_tokens > 0
            });
            let last =
                last.filter(|(tokens, _)| tokens.input_tokens > 0 || tokens.output_tokens > 0);
            let usage = if cumulative {
                &info["total_token_usage"]
            } else {
                last_usage
            };
            let (tokens, partial) = normalize(usage, "codex")?;
            if last.is_none()
                && (!cumulative || tokens.input_tokens == 0 && tokens.output_tokens == 0)
            {
                return None;
            }
            let mut sample = Sample::unmeasured(
                String::new(),
                &state.session,
                codex_model_or_unknown(&state.model).to_owned(),
                time,
                offset,
            );
            sample.tokens = tokens;
            sample.cumulative = cumulative;
            if context_only {
                sample.request.scope = Some(COUNTER_BASELINE.into());
            }
            sample.partial = !cumulative || last.as_ref().map_or(partial, |(_, partial)| *partial);
            sample.details =
                recorded_fields(if last.is_some() { last_usage } else { usage }, "codex");
            sample.last_usage = last.map(|(tokens, _)| tokens);
            sample
        }
        _ => return None,
    };
    sample.complete = true;
    sample.ordinal = value.get("ordinal").and_then(Value::as_u64);
    Some(sample)
}
