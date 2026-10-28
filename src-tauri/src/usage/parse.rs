//! 参考 ccusage 06a3343 的字段映射与快照去重口径；来源及 MIT 见本目录 README。
use super::types::*;
use chrono::DateTime;
use serde_json::Value;

pub(super) fn timestamp(value: Option<&Value>) -> Option<i64> {
    let value = value?;
    if value.is_number() {
        let n = i64::try_from(count(Some(value))?).ok()?;
        return if n <= 0 {
            None
        } else if n < 10_000_000_000 {
            n.checked_mul(1000)
        } else {
            Some(n)
        };
    }
    DateTime::parse_from_rfc3339(value.as_str()?.trim())
        .ok()
        .map(|d| d.timestamp_millis())
}
fn text(value: Option<&Value>) -> Option<&str> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}
fn number(value: &Value, key: &str) -> u64 {
    count(value.get(key)).unwrap_or(0)
}
fn count(value: Option<&Value>) -> Option<u64> {
    let value = value?;
    value
        .as_u64()
        .or_else(|| value.as_str()?.parse().ok())
        .or_else(|| {
            let n = value.as_f64().or_else(|| value.as_str()?.parse().ok())?;
            (n.is_finite() && n >= 0.0 && n.fract() == 0.0 && n < u64::MAX as f64)
                .then_some(n as u64)
        })
}
fn has_number(value: &Value, key: &str) -> bool {
    count(value.get(key)).is_some()
}
pub(super) fn recorded_fields(usage: &Value, agent: &str) -> RecordedFields {
    let (read, write, reason) = match agent {
        "codex" => (
            has_number(usage, "cached_input_tokens"),
            has_number(usage, "cache_write_input_tokens"),
            has_number(usage, "reasoning_output_tokens"),
        ),
        "opencode" => (
            has_number(&usage["cache"], "read"),
            has_number(&usage["cache"], "write"),
            has_number(usage, "reasoning"),
        ),
        "pi" => (
            has_number(usage, "cacheRead"),
            has_number(usage, "cacheWrite"),
            has_number(usage, "reasoning"),
        ),
        _ => (
            has_number(usage, "cache_read_input_tokens"),
            has_number(usage, "cache_creation_input_tokens")
                || has_number(&usage["cache_creation"], "ephemeral_5m_input_tokens")
                || has_number(&usage["cache_creation"], "ephemeral_1h_input_tokens"),
            has_number(&usage["output_tokens_details"], "reasoning_tokens")
                || has_number(&usage["output_tokens_details"], "thinking_tokens"),
        ),
    };
    RecordedFields {
        input_recorded: has_number(
            usage,
            if matches!(agent, "claude" | "qoder" | "codex") {
                "input_tokens"
            } else {
                "input"
            },
        ),
        output_recorded: has_number(
            usage,
            if matches!(agent, "claude" | "qoder" | "codex") {
                "output_tokens"
            } else {
                "output"
            },
        ),
        input_incomplete: !has_number(
            usage,
            if matches!(agent, "claude" | "qoder" | "codex") {
                "input_tokens"
            } else {
                "input"
            },
        ),
        output_incomplete: !has_number(
            usage,
            if matches!(agent, "claude" | "qoder" | "codex") {
                "output_tokens"
            } else {
                "output"
            },
        ),
        availability_known: true,
        cache_read_recorded: read,
        cache_write_recorded: write,
        reasoning_recorded: reason,
    }
}
fn canonical(provider: Option<&str>, model: Option<&str>) -> String {
    let model = model.unwrap_or("unknown");
    provider
        .filter(|p| !model.starts_with(&format!("{p}/")))
        .map(|p| format!("{p}/{model}"))
        .unwrap_or_else(|| model.into())
}

/// source.total 可能包含未提供分项的 token，保留差额而不伪造输出。
pub(super) fn normalize(usage: &Value, agent: &str) -> Option<(Tokens, bool)> {
    if !usage.is_object() {
        return None;
    }
    let snake = matches!(agent, "claude" | "qoder" | "codex");
    let input_key = if snake { "input_tokens" } else { "input" };
    let output_key = if snake { "output_tokens" } else { "output" };
    if !has_number(usage, input_key)
        && !has_number(usage, output_key)
        && !has_number(usage, "totalTokens")
        && !has_number(usage, "total_tokens")
        && !has_number(usage, "total")
    {
        return None;
    }
    let read = match agent {
        "codex" => number(usage, "cached_input_tokens"),
        "opencode" => number(&usage["cache"], "read"),
        "pi" => number(usage, "cacheRead"),
        _ => number(usage, "cache_read_input_tokens"),
    };
    let mut write = match agent {
        "codex" => number(usage, "cache_write_input_tokens"),
        "opencode" => number(&usage["cache"], "write"),
        "pi" => number(usage, "cacheWrite"),
        _ => number(usage, "cache_creation_input_tokens"),
    };
    if agent == "claude" {
        // TTL 是缓存创建总量的分项，旧版本可能只把完整值保存在这里，不能与聚合值叠加。
        let ttl = &usage["cache_creation"];
        write = write.max(
            number(ttl, "ephemeral_5m_input_tokens")
                .saturating_add(number(ttl, "ephemeral_1h_input_tokens")),
        );
    }
    let reasoning = match agent {
        "codex" => number(usage, "reasoning_output_tokens"),
        "pi" | "opencode" => number(usage, "reasoning"),
        _ => number(&usage["output_tokens_details"], "reasoning_tokens")
            .max(number(&usage["output_tokens_details"], "thinking_tokens")),
    };
    let mut tokens = Tokens {
        input_tokens: number(usage, input_key),
        output_tokens: number(usage, output_key),
        cache_read_tokens: read,
        cache_write_tokens: write,
        reasoning_tokens: reasoning,
        unclassified_tokens: 0,
    };
    if matches!(agent, "claude" | "pi" | "opencode") {
        tokens.input_tokens = tokens
            .input_tokens
            .saturating_add(read)
            .saturating_add(write);
    }
    if agent == "opencode" {
        tokens.output_tokens = tokens.output_tokens.saturating_add(reasoning);
    }
    let total_key = match agent {
        "pi" => "totalTokens",
        "opencode" => "total",
        _ => "total_tokens",
    };
    tokens.unclassified_tokens = number(usage, total_key).saturating_sub(tokens.total());
    // Qoder 缓存可能已包含在 input 中；只取已记录 input+output 作为下界，不再次相加。
    let partial = (!has_number(usage, input_key) || !has_number(usage, output_key))
        || (agent == "qoder" && read.saturating_add(write) > 0);
    Some((tokens, partial))
}

fn parse_one(agent: &str, value: &Value, offset: u64, state: &mut ParserState) -> Option<Sample> {
    if agent == "codex" {
        return codex(value, offset, state);
    }
    if agent == "pi" {
        return pi(value, offset, state);
    }
    let message = value.get("message")?;
    if value["type"] != "assistant" && message["role"] != "assistant" {
        return None;
    }
    state.saw_assistant = true;
    if let Some(id) = text(value.get("sessionId")) {
        state.session = id.into();
    }
    let model = text(message.get("model")).unwrap_or("unknown").to_owned();
    let mid = text(message.get("id"));
    let request =
        text(value.get("requestId")).or_else(|| text(message.pointer("/usage/request_id")));
    let key = claude_key(value, &state.session, offset);
    let Some((tokens, partial)) = message.get("usage").and_then(|u| normalize(u, agent)) else {
        if mid.is_some() || request.is_some() {
            state.unmeasured_ids.insert(key);
        } else {
            state.missing_usage += 1;
        }
        return None;
    };
    state.unmeasured_ids.remove(&key);
    Some(Sample {
        request: Default::default(),
        key,
        session: state.session.clone(),
        model,
        timestamp: timestamp(value.get("timestamp")).unwrap_or(0),
        tokens,
        details: recorded_fields(&message["usage"], agent),
        partial: partial || mid.is_none() && request.is_none(),
        offset,
        ordinal: None,
        complete: message.get("stop_reason").is_some_and(|v| !v.is_null()),
        replayed: value["isSidechain"].as_bool().unwrap_or(false),
        copy_fingerprint: None,
        cumulative: false,
        response_id: None,
        last_usage: None,
        thread_total: None,
    })
}

fn codex(value: &Value, offset: u64, state: &mut ParserState) -> Option<Sample> {
    let p = value.get("payload").unwrap_or(&Value::Null);
    let ordinal = value.get("ordinal").and_then(Value::as_u64);
    match value["type"].as_str()? {
        "session_meta" => {
            state.session = text(p.get("id"))
                .or_else(|| text(p.get("session_id")))
                .unwrap_or("")
                .into();
            state.parent_session = text(p.get("forked_from_id"))
                .or_else(|| text(p.get("parent_thread_id")))
                .or_else(|| text(p.pointer("/source/subagent/thread_spawn/parent_thread_id")))
                .map(str::to_owned);
            state.history_base = p
                .get("history_base")
                .and_then(|v| serde_json::from_value(v.clone()).ok());
            state.parent_before = p
                .get("forked_from_ordinal_exclusive")
                .and_then(Value::as_u64);
            // 外层时间是当前日志/fork 的创建时刻；payload.timestamp 可沿用父会话开始时间。
            state.created_at =
                timestamp(value.get("timestamp")).or_else(|| timestamp(p.get("timestamp")));
        }
        "turn_context" => {
            if let Some(model) = text(p.get("model")) {
                state.model = model.into();
            }
        }
        "compacted" => {
            if let Some(id) = text(p.get("response_id")) {
                state.compactions.insert(id.into());
            }
        }
        "token_usage_record" => {
            let (tokens, partial) = normalize(p.get("usage")?, "codex")?;
            let id = text(p.get("response_id"))?.to_owned();
            return Some(Sample {
                request: Default::default(),
                key: format!("response:{id}"),
                session: state.session.clone(),
                model: text(p.get("model")).unwrap_or(&state.model).to_owned(),
                timestamp: timestamp(value.get("timestamp")).unwrap_or(0),
                tokens,
                details: recorded_fields(&p["usage"], "codex"),
                partial,
                offset,
                ordinal,
                complete: true,
                replayed: false,
                copy_fingerprint: None,
                cumulative: false,
                response_id: Some(id),
                last_usage: None,
                thread_total: p
                    .get("thread_token_usage")
                    .and_then(|u| normalize(u, "codex"))
                    .map(|v| v.0),
            });
        }
        "event_msg" if p["type"] == "token_count" => {
            let info = p.get("info")?;
            if let Some(model) = text(info.get("model")).or_else(|| text(p.get("model"))) {
                state.model = model.into();
            }
            let (usage, cumulative) = if info["total_token_usage"].is_object() {
                (&info["total_token_usage"], true)
            } else {
                (&info["last_token_usage"], false)
            };
            let (tokens, partial) = normalize(usage, "codex")?;
            let last_usage = normalize(&info["last_token_usage"], "codex").map(|v| v.0);
            let details = recorded_fields(
                if last_usage.is_some() {
                    &info["last_token_usage"]
                } else {
                    usage
                },
                "codex",
            );
            return Some(Sample {
                request: Default::default(),
                key: String::new(),
                session: state.session.clone(),
                model: state.model.clone(),
                timestamp: timestamp(value.get("timestamp")).unwrap_or(0),
                tokens,
                details,
                partial: partial || !cumulative,
                offset,
                ordinal,
                complete: true,
                replayed: false,
                copy_fingerprint: None,
                cumulative,
                response_id: None,
                last_usage,
                thread_total: None,
            });
        }
        _ => {}
    }
    None
}

fn pi(value: &Value, offset: u64, state: &mut ParserState) -> Option<Sample> {
    let kind = value["type"].as_str()?;
    if kind == "session" {
        state.session = text(value.get("id")).unwrap_or("").into();
        state.parent_session = text(value.get("parentSession")).map(str::to_owned);
        return None;
    }
    if kind == "model_change" {
        state.model = canonical(text(value.get("provider")), text(value.get("modelId")));
        return None;
    }
    let source = if kind == "message" {
        let m = value.get("message")?;
        if !matches!(m["role"].as_str(), Some("assistant" | "toolResult")) {
            return None;
        }
        state.saw_assistant = true;
        m
    } else if matches!(kind, "usage" | "compaction" | "branch_summary") {
        value
    } else {
        return None;
    };
    let Some((tokens, partial)) = source.get("usage").and_then(|u| normalize(u, "pi")) else {
        if source["role"] == "assistant" {
            state.missing_usage += 1;
        }
        return None;
    };
    let model = if text(source.get("model"))
        .or_else(|| text(source.get("responseModel")))
        .is_some()
    {
        canonical(
            text(source.get("provider")),
            text(source.get("responseModel")).or_else(|| text(source.get("model"))),
        )
    } else if !state.model.is_empty() {
        state.model.clone()
    } else {
        "unknown".into()
    };
    let key = if let Some(id) = text(source.get("responseId")) {
        format!("response:{model}:{id}")
    } else {
        format!(
            "entry:{}",
            text(value.get("id"))
                .map(str::to_owned)
                .unwrap_or_else(|| offset.to_string())
        )
    };
    Some(Sample {
        request: Default::default(),
        key,
        session: state.session.clone(),
        model,
        tokens,
        details: recorded_fields(&source["usage"], "pi"),
        partial,
        timestamp: timestamp(value.get("timestamp"))
            .or_else(|| timestamp(source.get("timestamp")))
            .unwrap_or(0),
        offset,
        ordinal: None,
        complete: true,
        replayed: false,
        copy_fingerprint: None,
        cumulative: false,
        response_id: None,
        last_usage: None,
        thread_total: None,
    })
}

pub(super) fn parse(
    agent: &str,
    value: &Value,
    offset: u64,
    state: &mut ParserState,
) -> Vec<Sample> {
    let Some(primary) = parse_one(agent, value, offset, state) else {
        return Vec::new();
    };
    let mut result = vec![primary.clone()];
    if agent == "claude" {
        if let Some(iterations) = value
            .pointer("/message/usage/iterations")
            .and_then(Value::as_array)
        {
            // advisor_message 是独立模型消耗；普通 iterations 是主用量细分，不再次累加。
            // 普通细分可能在流式更新中插入，编号只取 advisor 序列，保持去重键稳定。
            for (index, item) in iterations
                .iter()
                .filter(|item| item["type"] == "advisor_message")
                .enumerate()
            {
                let Some(model) = text(item.get("model")) else {
                    continue;
                };
                let Some((tokens, partial)) =
                    normalize(item.get("usage").unwrap_or(item), "claude")
                else {
                    continue;
                };
                let mut record = primary.clone();
                record.key = format!("{}:advisor:{index}", record.key);
                record.model = model.into();
                record.tokens = tokens;
                record.details = recorded_fields(item.get("usage").unwrap_or(item), "claude");
                record.partial = primary.partial || partial;
                result.push(record);
            }
        }
    }
    result
}

pub(super) fn claude_key(value: &Value, session: &str, offset: u64) -> String {
    let message = &value["message"];
    let mid = text(message.get("id"));
    let request =
        text(value.get("requestId")).or_else(|| text(message.pointer("/usage/request_id")));
    match (mid, request) {
        (Some(id), _)
            if id.starts_with("msg_")
                || id.starts_with("chatcmpl")
                || crate::agents::claude_family::is_session_id(id) =>
        {
            format!("message:{id}")
        }
        (Some(id), Some(request)) => format!("request:{request}:{id}"),
        (Some(id), None) => format!("session:{}:message:{id}", session),
        (None, Some(request)) => format!("request:{request}"),
        _ => format!(
            "session:{}:event:{}",
            session,
            text(value.get("uuid"))
                .map(str::to_owned)
                .unwrap_or_else(|| offset.to_string())
        ),
    }
}
