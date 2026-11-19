//! 请求元数据与计量共用一次增量读取。只保存明确的错误字段，不收集对话正文。
use super::{codex, parse::timestamp};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RequestOutcome {
    #[default]
    #[serde(alias = "unknown")]
    Success,
    Failed,
}

impl RequestOutcome {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Failed => "failed",
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct RequestError {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl RequestError {
    pub(super) fn new(kind: Option<&str>, message: Option<&str>) -> Option<Self> {
        let read = |value: Option<&str>, limit| {
            value
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|value| {
                    let mut chars = value.chars();
                    let mut text: String = chars.by_ref().take(limit).collect();
                    if chars.next().is_some() {
                        text.push('…');
                    }
                    text
                })
        };
        let kind = read(kind, 128);
        let message = read(message, 4096);
        (kind.is_some() || message.is_some()).then_some(Self { kind, message })
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RequestMeta {
    pub request_id: Option<String>,
    pub provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native_session: Option<String>,
    pub project: Option<String>,
    pub client: Option<String>,
    pub kind: Option<String>,
    pub effort: Option<String>,
    pub tier: Option<String>,
    pub version: Option<String>,
    pub source: Option<String>,
    pub stop: Option<String>,
    pub status: Option<RequestOutcome>,
    pub http_status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RequestError>,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub timing: Option<String>,
    pub latency_ms: Option<i64>,
    pub ttft_ms: Option<i64>,
    /// Codex task_complete 的 TTFT 是整轮计时，只附在首个请求上并明确标注。
    pub ttft_scope: Option<String>,
    /// 单次请求为 request，累计差额为 usage_delta。
    pub scope: Option<String>,
}

impl RequestMeta {
    pub(super) fn failed(&self) -> bool {
        self.status == Some(RequestOutcome::Failed)
    }

    pub(super) fn normalize_codex_kind(&mut self) {
        if !matches!(self.kind.as_deref(), None | Some("cli" | "vscode")) {
            return;
        }
        let kind = match self.client.as_deref() {
            Some("codex-tui" | "codex_cli_rs" | "codex_cli") => "cli",
            Some("codex_exec") => "exec",
            _ => return,
        };
        if self.kind.as_deref() != Some(kind) {
            self.kind = Some(kind.into());
        }
    }

    pub fn merge(&mut self, next: &Self) {
        macro_rules! fill { ($($field:ident),*) => { $(if next.$field.is_some() { self.$field = next.$field.clone(); })* }; }
        fill!(
            request_id,
            provider,
            native_session,
            project,
            client,
            kind,
            effort,
            tier,
            version,
            source,
            stop,
            http_status,
            scope
        );
        if next.ttft_ms.is_some()
            && (self.ttft_scope.as_deref() != Some("request")
                || next.ttft_scope.as_deref() == Some("request"))
        {
            self.ttft_ms = next.ttft_ms;
            self.ttft_scope.clone_from(&next.ttft_scope);
        }
        if next.failed() {
            self.status = next.status;
            if let Some(error) = &next.error {
                let current = self.error.get_or_insert_with(RequestError::default);
                if error.kind.is_some() {
                    current.kind.clone_from(&error.kind);
                }
                if error.message.is_some() {
                    current.message.clone_from(&error.message);
                }
            }
        }
        let quality = |s: Option<&str>| match s {
            Some("recorded") => 3,
            Some("observed") => 2,
            Some(codex::BOUNDARY_ESTIMATE) => 1,
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
            self.latency_ms = self.latency_ms.max(next.latency_ms);
        }
        if self.timing.as_deref() == Some("observed") || next.timing == self.timing {
            self.finished_at = self.finished_at.max(next.finished_at);
            if self.timing.is_some() {
                self.duration();
            }
        }
    }

    pub(super) fn merge_claude_start(&mut self, earlier: &Self) {
        self.effort = self.effort.take().or_else(|| earlier.effort.clone());
        self.kind = self.kind.take().or_else(|| earlier.kind.clone());
        self.request_id = self
            .request_id
            .take()
            .or_else(|| earlier.request_id.clone());
        if self.started_at.is_none() {
            self.started_at = earlier.started_at;
            self.timing = earlier.timing.clone();
            self.duration();
        }
    }

    pub(super) fn merge_codex(&mut self, next: &Self) {
        let compacted = self.kind.as_deref() == Some("compaction")
            || next.kind.as_deref() == Some("compaction");
        self.clear_legacy_codex_timing();
        if next.timing.as_deref() == Some(codex::LEGACY_ESTIMATE) {
            let mut next = next.clone();
            next.clear_legacy_codex_timing();
            self.merge(&next);
        } else {
            self.merge(next);
        }
        if compacted {
            self.mark_compaction();
        }
    }

    pub(super) fn mark_compaction(&mut self) {
        self.kind = Some("compaction".into());
        let started_at = self.started_at;
        self.clear_timing();
        self.started_at = started_at;
    }

    pub(super) fn clear_timing(&mut self) {
        self.started_at = None;
        self.timing = None;
        self.latency_ms = None;
        self.ttft_ms = None;
        self.ttft_scope = None;
    }

    pub(super) fn clear_legacy_codex_timing(&mut self) {
        if self.timing.as_deref() == Some(codex::LEGACY_ESTIMATE) {
            let turn_ttft = self
                .ttft_ms
                .filter(|_| self.ttft_scope.as_deref() == Some("turn"));
            self.clear_timing();
            self.ttft_ms = turn_ttft;
            self.ttft_scope = turn_ttft.map(|_| "turn".into());
        }
    }

    pub(super) fn duration(&mut self) {
        if let Some((start, end)) = self.started_at.zip(self.finished_at) {
            self.latency_ms = elapsed_ms(start, end);
        }
    }
}

fn elapsed_ms(start: i64, end: i64) -> Option<i64> {
    end.checked_sub(start).filter(|&ms| start > 0 && ms > 0)
}

pub(super) fn string(v: &Value) -> Option<String> {
    v.as_str()
        .filter(|s| !s.is_empty())
        .map(|s| s.chars().take(2048).collect())
}
pub(super) fn at(v: &Value) -> Option<i64> {
    timestamp(Some(v))
}

pub(super) fn opencode(v: &Value, path: &Path, time: i64) -> RequestMeta {
    let started_at = at(&v["created"]).or_else(|| at(&Value::from(time)));
    let finished_at = ["streamed", "completed"]
        .into_iter()
        .filter_map(|key| at(&v[key]))
        .find(|end| started_at.is_none_or(|start| *end > start));
    let ttft_ms = at(&v["firstToken"])
        .filter(|first| finished_at.is_none_or(|end| *first <= end))
        .zip(started_at)
        .and_then(|(first, start)| elapsed_ms(start, first));
    let provider = string(&v["model"]["providerID"]).or_else(|| string(&v["providerID"]));
    let native_session = (provider.as_deref() == Some("claude-agent"))
        .then(|| super::parse::text(v.get("nativeSession")).map(str::to_owned))
        .flatten();
    let mut meta = RequestMeta {
        provider,
        native_session,
        project: string(&v["directory"]).or_else(|| string(&v["path"]["cwd"])),
        client: string(&v["agent"]).or_else(|| string(&v["mode"])),
        kind: Some(
            if v["role"] == "compaction" {
                "compaction"
            } else if string(&v["parent"]).is_some() {
                "subagent"
            } else {
                "main"
            }
            .into(),
        ),
        version: string(&v["version"]),
        effort: string(&v["model"]["variant"]).or_else(|| string(&v["variant"])),
        tier: string(&v["model"]["serviceTier"])
            .or_else(|| string(&v["model"]["service_tier"]))
            .or_else(|| string(&v["model"]["tier"]))
            .or_else(|| string(&v["tier"])),
        http_status: v["httpStatus"]
            .as_u64()
            .or_else(|| v["error"]["status"].as_u64())
            .and_then(|value| u16::try_from(value).ok()),
        error: RequestError::new(v["error"]["kind"].as_str(), v["error"]["message"].as_str()),
        started_at,
        finished_at,
        ttft_ms,
        ttft_scope: ttft_ms.map(|_| "request".into()),
        stop: string(&v["finish"]),
        scope: Some("request".into()),
        source: Some(path.to_string_lossy().into_owned()),
        status: (v["failed"] == true || v["failed"] == 1).then_some(RequestOutcome::Failed),
        ..Default::default()
    };
    meta.duration();
    meta.timing = meta.latency_ms.or(ttft_ms).map(|_| "recorded".into());
    meta
}
