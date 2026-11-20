//! 已结束的模型调用失败。SQLite 历史在 rollout 未保存终止事件时补充记录。
use super::{FileCache, RequestError, RequestMeta, RequestOutcome, Sample};
use crate::agents::scanning::sqlite_snapshot;
use rusqlite::Connection;
use serde_json::Value;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

pub(in crate::usage) const DATABASE: &str = "thread_history_1.sqlite";
pub(in crate::usage) const PARSER_VERSION: u8 = 1;
const TURN_KEY: &str = "codex-turn-failure:";

pub(in crate::usage) fn database(codex_home: &Path) -> PathBuf {
    let configured = std::fs::read_to_string(codex_home.join("config.toml"))
        .ok()
        .and_then(|text| toml::from_str::<toml::Value>(&text).ok())
        .and_then(|config| config.get("sqlite_home")?.as_str().map(str::to_owned));
    configured
        .or_else(|| std::env::var("CODEX_SQLITE_HOME").ok())
        .filter(|value| !value.trim().is_empty())
        .map(|value| codex_home.join(crate::agents::expand_tilde(value.trim())))
        .unwrap_or_else(|| codex_home.to_path_buf())
        .join(DATABASE)
}

fn turn_key(session: &str, turn: &str) -> String {
    if turn.len() == 36 && turn.bytes().enumerate().all(|(i, byte)| {
        if matches!(i, 8 | 13 | 18 | 23) { byte == b'-' } else { byte.is_ascii_hexdigit() }
    }) {
        format!("{TURN_KEY}{turn}")
    } else {
        format!("{TURN_KEY}{session}:{turn}")
    }
}

fn request_error(error: &Value) -> RequestMeta {
    let info = error.get("codex_error_info").or_else(|| error.get("codexErrorInfo"));
    let detail = info.and_then(Value::as_object).and_then(|fields| fields.iter().next());
    let kind = info.and_then(Value::as_str).or_else(|| detail.map(|(name, _)| name.as_str()));
    let message = error["message"].as_str();
    let status = detail
        .and_then(|(_, fields)| fields.get("http_status_code").or_else(|| fields.get("httpStatusCode")))
        .and_then(Value::as_u64)
        .and_then(|status| u16::try_from(status).ok());
    RequestMeta {
        status: Some(RequestOutcome::Failed),
        error: RequestError::new(kind, message),
        http_status: status.or_else(|| message.and_then(response_http_status)),
        scope: Some("error".into()),
        ..Default::default()
    }
}

fn response_http_status(message: &str) -> Option<u16> {
    let (status, _) = message.strip_prefix("unexpected status ")?.split_at_checked(3)?;
    let status = status.parse::<u16>().ok()?;
    let url = message.rsplit_once(", url: ")?.1.split([',', '?', '#']).next()?;
    ((400..600).contains(&status) && (url.starts_with("https://") || url.starts_with("http://"))
        && (url.ends_with("/responses") || url.ends_with("/chat/completions"))).then_some(status)
}

fn normalized_kind(kind: &str) -> String {
    kind.replace('_', "").to_ascii_lowercase()
}

pub(super) fn terminal(
    session: &str,
    turn: &str,
    model: &str,
    error: &Value,
    at: Option<i64>,
    offset: u64,
) -> Option<Sample> {
    if session.is_empty() || turn.is_empty() {
        return None;
    }
    let mut request = request_error(error);
    let kind = normalized_kind(request.error.as_ref()?.kind.as_deref()?);
    let provider_error = matches!(
        kind.as_str(),
        "httpconnectionfailed" | "responsestreamconnectionfailed" | "responsestreamdisconnected"
            | "responsetoomanyfailedattempts" | "usagelimitexceeded" | "ratelimitexceeded"
            | "serveroverloaded" | "flexunavailable" | "contextwindowexceeded" | "modelnotfound"
    );
    if !provider_error && !(kind == "other" && request.http_status.is_some()) {
        return None;
    }
    let key = turn_key(session, turn);
    let mut sample = Sample::unmeasured(key.clone(), session, model.into(), at.unwrap_or(0), offset);
    sample.request_identities.keys.insert(key);
    request.finished_at = at;
    sample.request = request;
    Some(sample)
}

pub(super) fn assign(sample: &mut Sample, turn: Option<&str>, error: &Value) {
    let request = request_error(error);
    sample.request.status = request.status;
    sample.request.error = request.error;
    sample.request.http_status = request.http_status;
    sample.request.scope = request.scope;
    if let Some(turn) = turn.filter(|turn| !turn.is_empty() && !sample.session.is_empty()) {
        sample.request_identities.keys.insert(turn_key(&sample.session, turn));
    }
}

pub(in crate::usage) fn read(
    path: &Path,
    old: Option<&FileCache>,
    force: bool,
    cancel: &dyn Fn() -> bool,
) -> Result<(FileCache, u64), String> {
    let stamps = crate::usage::source::sqlite_stamps(path).map_err(|error| error.to_string())?;
    if let Some(old) = old.filter(|old| !force && old.parser_version == PARSER_VERSION && old.stamps == stamps) {
        return Ok((old.clone(), 0));
    }
    let mut result = FileCache { stamps, parser_version: PARSER_VERSION, ..Default::default() };
    let bytes = sqlite_snapshot(path, |db| read_failed_turns(db, path, &mut result, cancel))
        .map_err(|error| error.to_string())?;
    Ok((result, bytes))
}

fn read_failed_turns(
    db: &Connection,
    path: &Path,
    result: &mut FileCache,
    cancel: &dyn Fn() -> bool,
) -> Result<u64, Box<dyn std::error::Error>> {
    let mut statement = db.prepare("SELECT thread_id, turn_id, completed_at, error_json FROM thread_turns
        WHERE status='failed' AND error_json IS NOT NULL")?;
    let mut rows = statement.query([])?;
    let mut bytes = 0;
    while let Some(row) = rows.next()? {
        if cancel() {
            return Err("采集已停止".into());
        }
        let session: String = row.get(0)?;
        let turn: String = row.get(1)?;
        let at = row.get::<_, Option<i64>>(2)?.and_then(|at| super::at(&Value::from(at)));
        let raw = row.get_ref(3)?.as_str()?;
        bytes += raw.len() as u64;
        let error: Value = serde_json::from_str(raw)?;
        if let Some(mut sample) = terminal(&session, &turn, "unknown", &error, at, 0) {
            sample.request.source = Some(path.to_string_lossy().into_owned());
            result.failures.push(sample);
        }
    }
    Ok(bytes)
}

pub(in crate::usage) fn reconcile(files: &BTreeMap<PathBuf, FileCache>) -> Vec<Sample> {
    let mut explicit = BTreeMap::<&str, Vec<&Sample>>::new();
    let mut terminal = BTreeMap::new();
    let mut retained = Vec::new();
    for (path, file) in files {
        let origin = file.origin_path.as_deref().unwrap_or(path);
        let rollout = origin.file_name().is_none_or(|name| name != DATABASE);
        for sample in &file.failures {
            let Some(turn) = sample.request_identities.keys.iter().find(|key| key.starts_with(TURN_KEY)) else {
                retained.push(sample.clone());
                continue;
            };
            if &sample.key != turn {
                explicit.entry(turn).or_default().push(sample);
                retained.push(sample.clone());
                continue;
            }
            let rank = (rollout, sample.model != "unknown", sample.timestamp);
            let entry = terminal.entry(turn.as_str()).or_insert((rank, sample));
            if rank > entry.0 {
                *entry = (rank, sample);
            }
        }
    }
    for (turn, (_, sample)) in terminal {
        // 轮次结束和 SQLite 可描述同一个失败。显式错误只在内容一致时取代这条补充记录。
        if !explicit.get(turn).is_some_and(|requests| requests.iter().any(|request| {
            let (Some(a), Some(b)) = (&request.request.error, &sample.request.error) else {
                return false;
            };
            a.message == b.message
                && a.kind.as_deref().map(normalized_kind) == b.kind.as_deref().map(normalized_kind)
        })) {
            retained.push(sample.clone());
        }
    }
    retained
}
