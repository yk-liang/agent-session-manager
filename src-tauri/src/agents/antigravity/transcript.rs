use super::{
    is_cli,
    models::Model,
    proto::Message,
    steps::{self, Content},
    storage::{self, MODEL_RESPONSE, USER_PROMPT},
};
use crate::agents::{context, escape_path_id, ms_to_iso, SessionDetail, SessionMeta};
use serde_json::{json, Value};
use std::{collections::{BTreeMap, HashMap}, path::Path};

fn generation_models(
    connection: &rusqlite::Connection,
    warnings: &mut Vec<String>,
) -> storage::ReadResult<BTreeMap<i64, Model>> {
    let mut stmt = connection.prepare("SELECT idx,data FROM gen_metadata ORDER BY idx")?;
    let mut rows = stmt.query([])?;
    let mut models = BTreeMap::new();
    while let Some(row) = rows.next()? {
        let index = row.get(0)?;
        let blob: Vec<u8> = row.get(1)?;
        match Message::decode(&blob).and_then(|message| message.message(1)) {
            Ok(request) => {
                let model = Model::generation(&request);
                if model.is_recorded() {
                    models.insert(index, model);
                }
            }
            Err(error) => warnings.push(format!("Antigravity 生成记录 {index} 的模型无法读取：{error}")),
        }
    }
    Ok(models)
}

fn count_message(meta: &mut SessionMeta, kind: i64, blocks: &[Value]) {
    let first_text = blocks.iter().filter(|block| block["type"] == "text")
        .find_map(|block| block["text"].as_str());
    match kind {
        USER_PROMPT => {
            meta.user_count += 1;
            if first_text.is_some() || blocks.iter().any(context::is_attachment) {
                meta.real_user_count += 1;
                if meta.title.is_none() {
                    meta.title = first_text.map(|text| text.chars().take(160).collect());
                }
            }
        }
        MODEL_RESPONSE => {
            meta.assistant_count += 1;
            meta.assistant_text_count += u32::from(first_text.is_some());
        }
        _ => {}
    }
}

pub(super) fn read(path: &Path, events: bool) -> Result<SessionDetail, String> {
    storage::read(path, |connection, schema| {
        let session_id = storage::session_id(path)?;
        let wal = crate::agents::scanning::sqlite_wal_path(path);
        let size = storage::database_size(connection)?;
        let (trajectory, _) = storage::trajectory(connection)?;
        let cwd = trajectory.cwd;
        let created = trajectory.created;
        let mut meta = SessionMeta {
            session_id: session_id.into(),
            project_dir: cwd.as_deref().map(escape_path_id).unwrap_or_else(|| "-unknown".into()),
            cwd,
            title: None,
            model: None,
            version: None,
            client: storage::client_id(path).map(str::to_owned),
            git_branch: None,
            entrypoint: Some(if is_cli(path) { "cli" } else { "antigravity" }.into()),
            codex: None,
            subagent: None,
            first_ts: created.and_then(ms_to_iso),
            last_ts: None,
            user_count: 0,
            assistant_count: 0,
            real_user_count: 0,
            assistant_text_count: 0,
            size_bytes: Some(size),
            has_transcript: false,
            archived: false,
            stats_pending: false,
            source_files: vec![path.to_path_buf(), wal],
        };
        let mut detail_events = Vec::new();
        let mut warnings = Vec::new();
        let generation = if schema.generations {
            generation_models(connection, &mut warnings).unwrap_or_else(|error| {
                warnings.push(format!("Antigravity 会话模型无法读取：{error}"));
                BTreeMap::new()
            })
        } else {
            BTreeMap::new()
        };
        let mut names = HashMap::new();
        for model in generation.values() {
            model.remember(&mut names);
        }
        let mut current_model = None;
        let mut latest_model_index = None;
        let mut last_time = None;
        if let Some(storage::StepColumns { status, error }) = schema.steps {
            let [tool, error_step, checkpoint, notice] = steps::DETAIL_TYPES;
            let mut stmt = connection.prepare(&format!(
                "SELECT idx, step_type, metadata,
                    CASE WHEN step_type IN ({USER_PROMPT}, {MODEL_RESPONSE})
                        OR :detail AND step_type IN ({tool}, {error_step}, {checkpoint}, {notice})
                        THEN step_payload END,
                    CASE WHEN :detail AND step_type = {tool} THEN {status} END,
                    CASE WHEN :detail AND step_type = {tool} THEN {error} END
                 FROM steps ORDER BY idx"
            ))?;
            let mut rows = stmt.query(rusqlite::named_params! {":detail": events})?;
            while let Some(row) = rows.next()? {
                let index: i64 = row.get(0)?;
                let kind: i64 = row.get(1)?;
                let metadata = row.get::<_, Option<Vec<u8>>>(2)?.unwrap_or_default();
                let payload: Option<Vec<u8>> = row.get(3)?;
                let status: Option<i64> = row.get(4)?;
                let error: Option<Vec<u8>> = row.get(5)?;
                let read_step = || -> std::io::Result<_> {
                    let metadata = Message::decode(&metadata)?;
                    let started = metadata.millis(1)?.filter(|&time| time > 0);
                    let finished = metadata.millis(8)?.filter(|&time| time > 0);
                    let model = Model::step(&metadata)?;
                    let content = if payload.is_some() || events && steps::DETAIL_TYPES.contains(&kind) {
                        Some(steps::decode(kind, &metadata, payload.as_deref().unwrap_or_default(),
                            status, error.as_deref().unwrap_or_default(), events)?)
                    } else {
                        None
                    };
                    Ok((started, finished, model, content))
                };
                let (started, finished, model, content) = match read_step() {
                    Ok(step) => step,
                    Err(error) => {
                        warnings.push(format!("Antigravity 会话步骤 {index} 无法读取：{error}"));
                        continue;
                    }
                };
                model.remember(&mut names);
                if let Some(model) = generation.get(&index).or_else(|| model.is_recorded().then_some(&model)) {
                    current_model = Some(model.resolve(None, &names).0);
                    latest_model_index = Some(index);
                }
                last_time = last_time.max(finished.or(started));
                if meta.first_ts.is_none() {
                    meta.first_ts = started.and_then(ms_to_iso);
                }
                meta.has_transcript |= steps::is_notice(kind);
                let id = format!("{session_id}:{index}");
                let timestamp = started.and_then(ms_to_iso);
                match content {
                    Some(Content::Message(blocks)) => {
                        count_message(&mut meta, kind, &blocks);
                        meta.has_transcript |= !blocks.is_empty();
                        if events && !blocks.is_empty() {
                            let role = if kind == MODEL_RESPONSE { "assistant" } else { "user" };
                            let mut event = json!({"uuid":id,"type":role,"timestamp":timestamp,
                                "message":{"role":role,"content":blocks}});
                            if kind == MODEL_RESPONSE {
                                event["message"]["model"] = json!(current_model);
                            }
                            detail_events.push(event);
                        }
                    }
                    Some(Content::Context { kind, label, text }) => {
                        meta.has_transcript = true;
                        detail_events.push(json!({"uuid":id,"type":"context","timestamp":timestamp,
                            "context":{"kind":kind,"label":label},
                            "message":{"role":"system","content":text}}));
                    }
                    Some(Content::System(text)) => {
                        meta.has_transcript = true;
                        detail_events.push(json!({"uuid":id,"type":"system","timestamp":timestamp,"content":text}));
                    }
                    None => count_message(&mut meta, kind, &[]),
                }
            }
        }
        meta.last_ts = last_time.and_then(ms_to_iso);
        if let Some((index, model)) = generation.last_key_value() {
            if latest_model_index.is_none_or(|latest| *index > latest) {
                current_model = Some(model.resolve(None, &names).0);
            }
        }
        meta.model = current_model.filter(|name| name != "unknown");
        crate::agents::tool_ops::describe_tools(&mut detail_events, super::tools::describe);
        Ok(SessionDetail { meta, events: detail_events, warnings })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use std::fs;

    #[test]
    fn size_uses_logical_pages_instead_of_bloated_wal() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("active-session.db");
        let wal = crate::agents::scanning::sqlite_wal_path(&path);
        let writer = Connection::open(&path).unwrap();
        writer
            .execute_batch(
                "PRAGMA journal_mode = WAL;
                 PRAGMA wal_autocheckpoint = 0;
                 CREATE TABLE steps (idx INTEGER PRIMARY KEY, step_type INTEGER, metadata BLOB, step_payload BLOB);",
            )
            .unwrap();
        writer
            .execute(
                "INSERT INTO steps (idx, step_type, metadata, step_payload) VALUES (0, ?1, X'', ?2)",
                rusqlite::params![USER_PROMPT, vec![b'a'; 8192]],
            )
            .unwrap();
        writer.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);").unwrap();

        // 保持读事务打开以固定 WAL，模拟运行中连接反复重写同一批页导致的 WAL 膨胀。
        let reader = Connection::open(&path).unwrap();
        reader.execute_batch("BEGIN; SELECT count(*) FROM steps;").unwrap();
        for byte in 1u8..=40 {
            writer
                .execute(
                    "UPDATE steps SET step_payload = ?1 WHERE idx = 0",
                    [vec![byte; 8192]],
                )
                .unwrap();
        }
        let db_len = fs::metadata(&path).unwrap().len();
        let wal_len = fs::metadata(&wal).unwrap().len();
        assert!(wal_len > db_len * 3, "expected bloated WAL: db={db_len}, wal={wal_len}");

        let during_wal = read(&path, false).unwrap();
        drop(reader);
        writer.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);").unwrap();
        let checkpointed_len = fs::metadata(&path).unwrap().len();
        let after_checkpoint = read(&path, false).unwrap();

        assert_eq!(during_wal.meta.size_bytes, Some(checkpointed_len));
        assert_eq!(after_checkpoint.meta.size_bytes, Some(checkpointed_len));
    }
}
