use super::{
    is_cli,
    models::Model,
    proto::Message,
    storage::{self, MODEL_RESPONSE, TOOL_RESULT, USER_PROMPT},
};
use crate::agents::{escape_path_id, ms_to_iso, SessionDetail, SessionMeta};
use serde_json::{json, Value};
use std::{collections::HashMap, fs, path::Path};

fn first_text(content: &[Value]) -> Option<&str> {
    content
        .iter()
        .find_map(|block| block.get("text").and_then(Value::as_str))
}

fn content(kind: i64, payload: &[u8]) -> std::io::Result<Vec<Value>> {
    let payload = Message::decode(payload)?;
    let mut blocks = Vec::new();
    match kind {
        USER_PROMPT => {
            let prompt = payload.message(19)?;
            if let Some(text) = prompt.text(2).or(prompt.message(3)?.text(1)) {
                blocks.push(json!({"type":"text", "text":text}));
            }
        }
        MODEL_RESPONSE => {
            let response = payload.message(20)?;
            if let Some(text) = response.text(1).or_else(|| response.text(8)) {
                blocks.push(json!({"type":"text", "text":text}));
            }
            if let Some(thinking) = response.text(3) {
                blocks.push(json!({"type":"thinking", "thinking":thinking}));
            }
            for call in response.repeated(7) {
                let call = Message::decode(call)?;
                if let Some(name) = call.text(2) {
                    let input = call
                        .text(3)
                        .map(|text| serde_json::from_str(text).unwrap_or_else(|_| json!(text)))
                        .unwrap_or_else(|| json!({}));
                    blocks.push(
                        json!({"type":"tool_use", "id":call.text(1), "name":name, "input":input}),
                    );
                }
            }
        }
        _ => {}
    }
    Ok(blocks)
}

fn tool_result(
    metadata: &Message<'_>,
    payload: &[u8],
    status: Option<i64>,
    error: &[u8],
) -> std::io::Result<Vec<Value>> {
    let call = metadata.message(4)?;
    let output = Message::decode(payload)?.message(140)?.message(2)?;
    let error_details = Message::decode(error)?;
    let text = output
        .text(1)
        .or_else(|| error_details.text(2))
        .or_else(|| error_details.text(3));
    if call.text(1).is_none() && text.is_none() {
        return Ok(Vec::new());
    }
    let status = match status {
        Some(3) => Some("completed"),
        Some(7) => Some("error"),
        _ if !error.is_empty() => Some("error"),
        _ => None,
    };
    if status.is_none() && text.is_none() {
        return Ok(Vec::new());
    }
    let mut result = json!({"type":"tool_result", "tool_use_id":call.text(1), "content":text});
    if let Some(status) = status {
        result["status"] = json!(status);
        result["is_error"] = json!(status == "error");
    }
    Ok(vec![result])
}

fn latest_model(connection: &rusqlite::Connection) -> storage::ReadResult<Option<Model>> {
    let mut stmt = connection.prepare("SELECT data FROM gen_metadata ORDER BY idx DESC")?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let blob: Vec<u8> = row.get(0)?;
        let request = Message::decode(&blob)?.message(1)?;
        let model = Model::generation(&request);
        if model.is_recorded() {
            return Ok(Some(model));
        }
    }
    Ok(None)
}

pub(super) fn read(path: &Path, events: bool) -> Result<SessionDetail, String> {
    storage::read(path, |connection, schema| {
        let session_id = storage::session_id(path)?;
        let stats = fs::metadata(path)?;
        let (trajectory, _) = storage::trajectory(connection)?;
        let cwd = trajectory.cwd;
        let created = trajectory.created;
        let fallback_time = stats
            .modified()
            .ok()
            .map(|time| chrono::DateTime::<chrono::Utc>::from(time).timestamp_millis());
        let mut meta = SessionMeta {
            session_id: session_id.into(),
            project_dir: cwd.as_deref().map(escape_path_id).unwrap_or_default(),
            cwd,
            title: None,
            model: None,
            version: None,
            git_branch: None,
            entrypoint: Some(if is_cli(path) { "cli" } else { "antigravity" }.into()),
            first_ts: created.and_then(ms_to_iso),
            last_ts: None,
            user_count: 0,
            assistant_count: 0,
            real_user_count: 0,
            assistant_text_count: 0,
            size_bytes: Some(stats.len()),
            has_transcript: false,
            archived: false,
            stats_pending: false,
            source_files: vec![
                path.to_path_buf(),
                crate::agents::scanning::sqlite_wal_path(path),
            ],
        };
        let mut detail_events = Vec::new();
        let mut warnings = Vec::new();
        let mut latest = Model::default();
        let mut last_time = created;
        if schema.steps {
            let columns = connection.prepare("SELECT * FROM steps LIMIT 0")?;
            let status = if columns.column_names().contains(&"status") {
                "status"
            } else {
                "NULL"
            };
            let error = if columns.column_names().contains(&"error_details") {
                "error_details"
            } else {
                "NULL"
            };
            let mut stmt = connection.prepare(&format!(
                "SELECT idx, step_type, metadata,
                    CASE WHEN step_type IN (:user, :response) OR :detail AND step_type = :tool THEN step_payload END,
                    CASE WHEN :detail AND step_type = :tool THEN {status} END,
                    CASE WHEN :detail AND step_type = :tool THEN {error} END
                 FROM steps ORDER BY idx"
            ))?;
            let mut rows = stmt.query(rusqlite::named_params! {
                ":user": USER_PROMPT, ":response": MODEL_RESPONSE,
                ":tool": TOOL_RESULT, ":detail": events,
            })?;
            while let Some(row) = rows.next()? {
                let index: i64 = row.get(0)?;
                let kind: i64 = row.get(1)?;
                let metadata = row.get::<_, Option<Vec<u8>>>(2)?.unwrap_or_default();
                let payload = row.get::<_, Option<Vec<u8>>>(3)?;
                let step = || -> std::io::Result<_> {
                    let metadata = Message::decode(&metadata)?;
                    let started = metadata.millis(1)?.filter(|&time| time > 0);
                    let finished = metadata.millis(8)?.filter(|&time| time > 0);
                    let model = Model::step(&metadata)?;
                    let blocks = if events && kind == TOOL_RESULT {
                        let status = row
                            .get::<_, Option<i64>>(4)
                            .map_err(std::io::Error::other)?;
                        let error = row
                            .get::<_, Option<Vec<u8>>>(5)
                            .map_err(std::io::Error::other)?;
                        tool_result(
                            &metadata,
                            payload.as_deref().unwrap_or_default(),
                            status,
                            error.as_deref().unwrap_or_default(),
                        )?
                    } else if let Some(payload) = payload {
                        content(kind, &payload)?
                    } else {
                        Vec::new()
                    };
                    Ok((started, finished, model, blocks))
                };
                let (started, finished, model, blocks) = match step() {
                    Ok(step) => step,
                    Err(error) => {
                        warnings.push(format!("Antigravity 会话步骤 {index} 无法读取：{error}"));
                        continue;
                    }
                };
                if model.is_recorded() {
                    latest = model;
                }
                last_time = last_time.max(finished.or(started));
                if meta.first_ts.is_none() {
                    meta.first_ts = started.and_then(ms_to_iso);
                }
                match kind {
                    USER_PROMPT => {
                        meta.user_count += 1;
                        if let Some(text) = first_text(&blocks) {
                            meta.real_user_count += 1;
                            if meta.title.is_none() {
                                meta.title = Some(text.chars().take(160).collect());
                            }
                        }
                    }
                    MODEL_RESPONSE => {
                        meta.assistant_count += 1;
                        meta.assistant_text_count += u32::from(first_text(&blocks).is_some());
                    }
                    _ => {}
                }
                meta.has_transcript |= !blocks.is_empty();
                if events && !blocks.is_empty() {
                    let role = if kind == MODEL_RESPONSE {
                        "assistant"
                    } else {
                        "user"
                    };
                    detail_events.push(json!({
                        "uuid": format!("{session_id}:{index}"),
                        "type": role, "timestamp": started.and_then(ms_to_iso),
                        "message": {"role":role, "content":blocks}
                    }));
                }
            }
        }
        meta.last_ts = last_time.or(fallback_time).and_then(ms_to_iso);
        if schema.generations {
            match latest_model(connection) {
                Ok(Some(model)) => latest = model,
                Ok(None) => {}
                Err(error) => warnings.push(format!("Antigravity 会话模型无法读取：{error}")),
            }
        }
        let (name, _) = latest.resolve(None, &HashMap::new());
        if name != "unknown" {
            meta.model = Some(name);
        }
        Ok(SessionDetail {
            meta,
            events: detail_events,
            warnings,
        })
    })
}
