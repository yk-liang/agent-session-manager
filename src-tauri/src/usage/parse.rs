//! 参考 ccusage 06a3343 的字段映射与快照去重口径；来源及 MIT 见本目录 README。
use super::{cache::FileCache, types::*};
use chrono::DateTime;
use serde_json::Value;
use std::path::Path;

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
pub(super) fn text(value: Option<&Value>) -> Option<&str> {
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
fn io_keys(agent: &str) -> (&'static str, &'static str) {
    if matches!(agent, "claude" | "qoder" | "codex") {
        ("input_tokens", "output_tokens")
    } else {
        ("input", "output")
    }
}

pub(super) fn recorded_fields(usage: &Value, agent: &str) -> RecordedFields {
    let (read, write, reason) = match agent {
        "codex" => (
            has_number(usage, "cached_input_tokens"),
            has_number(usage, "cache_write_input_tokens")
                || has_number(usage, "cache_creation_input_tokens"),
            has_number(usage, "reasoning_output_tokens"),
        ),
        "opencode" => (
            has_number(&usage["cache"], "read"),
            has_number(&usage["cache"], "write"),
            has_number(usage, "reasoning"),
        ),
        "pi" => (
            has_number(usage, "cacheRead"),
            has_number(usage, "cacheWrite") || has_number(usage, "cacheWrite1h"),
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
    let (input, output) = io_keys(agent);
    let input_recorded = has_number(usage, input);
    let output_recorded = has_number(usage, output);
    RecordedFields {
        input_recorded,
        output_recorded,
        input_incomplete: !input_recorded,
        output_incomplete: !output_recorded,
        cache_read_recorded: read,
        cache_write_recorded: write,
        reasoning_recorded: reason,
    }
}
/// source.total 可能包含未提供分项的 Token，保留差额而不伪造输出。
pub(super) fn normalize(usage: &Value, agent: &str) -> Option<(Tokens, bool)> {
    if !usage.is_object() {
        return None;
    }
    let (input_key, output_key) = io_keys(agent);
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
        "codex" => number(usage, "cache_write_input_tokens")
            .max(number(usage, "cache_creation_input_tokens")),
        "opencode" => number(&usage["cache"], "write"),
        "pi" => number(usage, "cacheWrite"),
        _ => number(usage, "cache_creation_input_tokens"),
    };
    let mut write_1h = 0;
    if matches!(agent, "claude" | "qoder") {
        // TTL 是缓存创建总量的分项，旧版本可能只把完整值保存在这里，不能与聚合值叠加。
        let (short, long) = if agent == "claude" {
            super::claude::message_cache_creation(usage)
        } else {
            None
        }.unwrap_or_else(|| {
            let ttl = &usage["cache_creation"];
            (number(ttl, "ephemeral_5m_input_tokens"), number(ttl, "ephemeral_1h_input_tokens"))
        });
        write_1h = long;
        write = write.max(short.saturating_add(long));
    }
    if agent == "pi" {
        write_1h = number(usage, "cacheWrite1h");
        write = write.max(write_1h);
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
        cache_write_1h_tokens: write_1h,
    };
    if matches!(agent, "claude" | "pi" | "opencode") {
        tokens.input_tokens = tokens
            .input_tokens
            .saturating_add(read)
            .saturating_add(write);
    }
    if agent == "opencode" || agent == "pi" && usage["totalTokens"].as_u64()
        .is_some_and(|total| tokens.total().checked_add(reasoning) == Some(total))
    {
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

pub(super) fn read_line(
    agent: &str,
    value: &Value,
    offset: u64,
    path: &Path,
    file: &mut FileCache,
) {
    match agent {
        "codex" => super::codex::read_line(value, offset, path, file),
        "pi" => super::pi::read_line(value, offset, path, file),
        _ => super::claude::read_line(agent, value, offset, path, file),
    }
}
