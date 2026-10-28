//! 请求元数据白名单。与计量共用一次增量读取，不持久化提示词、响应正文或认证信息。
use super::{cache::FileCache, parse::timestamp, types::*};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, path::Path};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RequestMeta {
    pub request_id: Option<String>,
    pub provider: Option<String>,
    pub project: Option<String>,
    pub client: Option<String>,
    pub kind: Option<String>,
    pub effort: Option<String>,
    pub tier: Option<String>,
    pub version: Option<String>,
    pub source: Option<String>,
    pub stop: Option<String>,
    pub status: Option<String>,
    pub http_status: Option<u16>,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    /// recorded = 源明确计时；observed = 请求/响应日志时间差；estimated = 相邻事件边界。
    pub timing: Option<String>,
    pub latency_ms: Option<i64>,
    pub ttft_ms: Option<i64>,
    /// Codex task_complete 的 TTFT 是整轮计时，只附在首个请求上并明确标注。
    pub ttft_scope: Option<String>,
    /// request / usage_delta。累计差额不冒充一次模型请求。
    pub scope: Option<String>,
    pub cost_usd: Option<f64>,
}

impl RequestMeta {
    pub fn merge(&mut self, next: &Self) {
        macro_rules! fill { ($($field:ident),*) => { $(if next.$field.is_some() { self.$field = next.$field.clone(); })* }; }
        fill!(
            request_id,
            provider,
            project,
            client,
            kind,
            effort,
            tier,
            version,
            source,
            stop,
            http_status,
            ttft_ms,
            ttft_scope,
            scope
        );
        self.cost_usd = match (self.cost_usd, next.cost_usd) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        };
        if next.status.as_deref().is_some_and(|s| s != "unknown") || self.status.is_none() {
            self.status = next.status.clone();
        }
        let quality = |s: Option<&str>| match s {
            Some("recorded") => 3,
            Some("observed") => 2,
            Some("estimated") => 1,
            _ => 0,
        };
        if quality(next.timing.as_deref()) > quality(self.timing.as_deref()) {
            self.started_at = next.started_at;
            self.finished_at = next.finished_at;
            self.latency_ms = next.latency_ms;
            self.timing = next.timing.clone();
        } else if next.timing == self.timing {
            self.started_at = match (self.started_at, next.started_at) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            };
            self.finished_at = self.finished_at.max(next.finished_at);
            self.latency_ms = self.latency_ms.max(next.latency_ms);
            self.duration();
        }
    }
    fn duration(&mut self) {
        if let Some((start, end)) = self.started_at.zip(self.finished_at) {
            self.latency_ms = (start > 0 && end > start).then_some(end - start);
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub(super) struct RequestState {
    context: RequestMeta,
    boundary: Option<i64>,
    turn_start: Option<i64>,
    turn_id: String,
    first_offset: Option<u64>,
    first_response: bool,
    claude: BTreeMap<String, RequestMeta>,
}

fn string(v: &Value) -> Option<String> {
    v.as_str()
        .filter(|s| !s.is_empty())
        .map(|s| s.chars().take(2048).collect())
}
fn at(v: &Value) -> Option<i64> {
    timestamp(Some(v))
}
fn nonnegative(v: &Value) -> Option<f64> {
    v.as_f64().filter(|n| n.is_finite() && *n >= 0.0)
}

pub(super) fn observe(
    agent: &str,
    v: &Value,
    offset: u64,
    path: &Path,
    file: &mut FileCache,
    samples: &mut Vec<Sample>,
) {
    if agent == "codex" {
        codex(v, offset, path, file, samples);
        return;
    }
    let time = at(&v["timestamp"]);
    if agent == "claude" && v["type"] == "api-request" {
        if let Some(id) = string(&v["id"]) {
            file.state.requests.claude.insert(
                id,
                RequestMeta {
                    started_at: time,
                    kind: string(&v["querySource"]),
                    effort: string(&v["params"]["output_config"]["effort"]),
                    ..Default::default()
                },
            );
            // 只保留有可能关联后续响应的近期开始事件，不随历史无限增长。
            if file.state.requests.claude.len() > 256 {
                let oldest = file
                    .state
                    .requests
                    .claude
                    .iter()
                    .min_by_key(|(_, m)| m.started_at)
                    .map(|(id, _)| id.clone());
                if let Some(id) = oldest {
                    file.state.requests.claude.remove(&id);
                }
            }
        }
    }
    let m = &v["message"];
    let failed = v["isApiErrorMessage"] == true
        || matches!(m["stopReason"].as_str(), Some("error" | "aborted"));
    if failed && samples.is_empty() && (v["type"] == "assistant" || m["role"] == "assistant") {
        let session = string(&v["sessionId"]).unwrap_or_else(|| file.state.session.clone());
        let key = super::parse::claude_key(v, &session, offset);
        let model = string(&v["requestedModel"])
            .or_else(|| string(&m["model"]))
            .unwrap_or_else(|| "unknown".into());
        let mut failure = empty_sample(key, session, model, time.unwrap_or(0), offset);
        failure.request = common(agent, v, time, path, &file.state.requests);
        failure.request.status = Some("failed".into());
        file.failures.push(failure);
    }
    for (index, sample) in samples.iter_mut().enumerate() {
        sample.request = common(agent, v, time, path, &file.state.requests);
        if sample.model == "<synthetic>" && failed {
            sample.model = string(&v["requestedModel"]).unwrap_or_else(|| "unknown".into());
        }
        if index > 0 {
            sample.request.kind = Some("advisor".into());
            sample.request.started_at = None;
            sample.request.latency_ms = None;
            sample.request.timing = None;
        }
    }
}

fn common(
    agent: &str,
    v: &Value,
    time: Option<i64>,
    path: &Path,
    state: &RequestState,
) -> RequestMeta {
    let m = &v["message"];
    let usage = &m["usage"];
    let request_id = string(&v["requestRef"]).or_else(|| string(&v["requestId"]));
    let start = request_id.as_ref().and_then(|id| state.claude.get(id));
    let stop = string(&m["stop_reason"]).or_else(|| string(&m["stopReason"]));
    let failed =
        v["isApiErrorMessage"] == true || matches!(stop.as_deref(), Some("error" | "aborted"));
    let mut meta = RequestMeta {
        request_id,
        provider: string(&m["provider"]),
        project: string(&v["cwd"]),
        client: string(&v["entrypoint"]),
        version: string(&v["version"]),
        kind: start
            .and_then(|s| s.kind.clone())
            .or_else(|| (v["isSidechain"] == true).then(|| "sidechain".into())),
        effort: string(&v["effort"]).or_else(|| start.and_then(|s| s.effort.clone())),
        tier: string(&usage["speed"])
            .filter(|s| s != "standard")
            .or_else(|| string(&usage["service_tier"])),
        status: Some(
            if failed {
                "failed"
            } else if stop.is_some() {
                "success"
            } else {
                "unknown"
            }
            .into(),
        ),
        stop,
        http_status: v["apiErrorStatus"]
            .as_u64()
            .and_then(|n| u16::try_from(n).ok()),
        source: Some(path.to_string_lossy().into_owned()),
        finished_at: time,
        started_at: start.and_then(|s| s.started_at),
        timing: start.map(|_| "observed".into()),
        scope: Some("request".into()),
        cost_usd: if agent == "pi" {
            nonnegative(&usage["cost"]["total"])
        } else {
            None
        },
        ..Default::default()
    };
    meta.duration();
    meta
}

fn codex(v: &Value, offset: u64, path: &Path, file: &mut FileCache, samples: &mut [Sample]) {
    let p = &v["payload"];
    let time = at(&v["timestamp"]);
    let state = &mut file.state.requests;
    match v["type"].as_str() {
        Some("session_meta") => {
            state.context.provider = string(&p["model_provider"]);
            state.context.project = string(&p["cwd"]);
            state.context.client = string(&p["originator"]);
            state.context.version = string(&p["cli_version"]);
            state.context.kind = string(&p["source"])
                .or_else(|| p["source"].get("subagent").map(|_| "subagent".into()));
        }
        Some("turn_context") => {
            state.context.effort = string(&p["effort"])
                .or_else(|| string(&p["collaboration_mode"]["settings"]["reasoning_effort"]));
            state.context.project = string(&p["cwd"]).or(state.context.project.clone());
        }
        Some("event_msg") => match p["type"].as_str() {
            Some("task_started") => {
                state.boundary = time;
                state.turn_start = time;
                state.turn_id = string(&p["turn_id"]).unwrap_or_default();
                state.first_offset = None;
                state.first_response = false;
            }
            Some("item_completed") if p["item"]["type"] == "UserMessage" => {
                state.boundary = state.boundary.max(time).max(at(&p["completed_at_ms"]));
            }
            Some("user_message") => {
                state.boundary = state.boundary.max(time);
            }
            Some("thread_settings_applied") => {
                let s = &p["thread_settings"];
                state.context.provider =
                    string(&s["model_provider_id"]).or(state.context.provider.clone());
                if s.get("service_tier").is_some() {
                    state.context.tier = string(&s["service_tier"]);
                }
            }
            Some("task_complete") if p["turn_id"].as_str() == Some(state.turn_id.as_str()) => {
                if let Some(ttft) = p["time_to_first_token_ms"].as_i64().filter(|t| *t > 0) {
                    if let Some(sample) = file
                        .samples
                        .iter_mut()
                        .find(|s| Some(s.offset) == state.first_offset)
                    {
                        sample.request.ttft_ms = Some(ttft);
                        sample.request.ttft_scope = Some("turn".into());
                    }
                }
            }
            Some("error") => {
                let key = format!(
                    "error:{}:{}:{}",
                    file.state.session,
                    time.unwrap_or(0),
                    offset
                );
                let mut failure = empty_sample(
                    key,
                    file.state.session.clone(),
                    file.state.model.clone(),
                    time.unwrap_or(0),
                    offset,
                );
                failure.request = state.context.clone();
                failure.request.status = Some("failed".into());
                // error 可能是工具/连接错误；不假装它有一次成功请求的耗时或计量。
                failure.request.scope = Some("error".into());
                failure.request.source = Some(path.to_string_lossy().into_owned());
                file.failures.push(failure);
            }
            _ => {}
        },
        _ => {}
    }
    for sample in samples.iter_mut() {
        if sample.response_id.is_some()
            && file.state.parent_session.is_some()
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
        meta.started_at = state
            .boundary
            .filter(|start| time.is_some_and(|end| end > *start));
        meta.finished_at = time;
        meta.timing = meta.started_at.map(|_| "estimated".into());
        meta.status = Some(
            if sample.response_id.is_some() {
                "success"
            } else {
                "unknown"
            }
            .into(),
        );
        meta.scope = Some(
            if sample.response_id.is_some() || sample.last_usage.is_some() {
                "request"
            } else {
                "usage_delta"
            }
            .into(),
        );
        meta.duration();
        sample.request = meta;
        if sample.response_id.is_some() && !state.first_response {
            state.first_offset = Some(offset);
            state.first_response = true;
        } else {
            state.first_offset.get_or_insert(offset);
        }
    }
    if v["type"] == "token_usage_record" || p["type"] == "token_count" {
        state.boundary = state.boundary.max(time);
    }
}

fn empty_sample(
    key: String,
    session: String,
    model: String,
    timestamp: i64,
    offset: u64,
) -> Sample {
    Sample {
        key,
        session,
        model,
        timestamp,
        offset,
        tokens: Tokens::default(),
        details: RecordedFields {
            availability_known: true,
            ..Default::default()
        },
        request: RequestMeta::default(),
        partial: true,
        ordinal: None,
        complete: true,
        replayed: false,
        copy_fingerprint: None,
        cumulative: false,
        response_id: None,
        last_usage: None,
        thread_total: None,
    }
}

pub(super) fn opencode(v: &Value, path: &Path) -> RequestMeta {
    let mut meta = RequestMeta {
        provider: string(&v["providerID"]),
        project: string(&v["path"]["cwd"]),
        kind: string(&v["agent"]),
        started_at: at(&v["created"]),
        finished_at: at(&v["completed"]),
        timing: Some("recorded".into()),
        stop: string(&v["finish"]),
        scope: Some("request".into()),
        source: Some(path.to_string_lossy().into_owned()),
        cost_usd: nonnegative(&v["cost"]),
        status: Some(
            if v["failed"] == true || v["failed"] == 1 {
                "failed"
            } else if at(&v["completed"]).is_some() {
                "success"
            } else {
                "unknown"
            }
            .into(),
        ),
        ..Default::default()
    };
    meta.duration();
    meta
}
