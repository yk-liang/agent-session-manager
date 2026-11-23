use super::{events, storage};
use crate::agents::{self, context, ParseOutcome, SessionMeta};
use serde_json::{json, Value};
use std::{collections::{HashMap, HashSet}, path::Path};

pub(super) struct ParsedPi {
    pub(super) meta: SessionMeta,
    pub(super) events: Vec<Value>,
}

fn active_branch(entries: &[Value], version: i64) -> Vec<&Value> {
    if version < 2 {
        return entries.iter().collect();
    }
    let by_id: HashMap<_, _> = entries.iter()
        .filter_map(|entry| Some((entry["id"].as_str()?, entry)))
        .collect();
    let mut path = Vec::new();
    let mut seen = HashSet::new();
    let mut leaf = entries.iter().rev().find(|entry| entry["id"].is_string());
    while let Some(entry) = leaf {
        if !seen.insert(entry["id"].as_str().unwrap()) {
            break;
        }
        path.push(entry);
        leaf = entry["parentId"].as_str().and_then(|id| by_id.get(id)).copied();
    }
    path.reverse();
    path
}

fn nonempty(value: &Value) -> Option<&str> {
    value.as_str().map(str::trim).filter(|text| !text.is_empty())
}

pub(super) fn parse(path: &Path, collect_events: bool) -> ParseOutcome<ParsedPi> {
    let (header, lines, size_bytes) = match storage::open(path) {
        Ok(file) => file,
        Err(error) => return ParseOutcome::Failed(error),
    };
    let Some(id) = storage::session_id(&header, path) else {
        return ParseOutcome::Failed(format!("无法解析 Pi 会话 ID：{path:?}"));
    };
    let first_ts = events::timestamp(&header);
    let mut meta = SessionMeta {
        session_id: id.to_owned(),
        project_dir: path.parent().and_then(Path::file_name)
            .map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
        cwd: nonempty(&header["cwd"]).map(str::to_owned),
        title: None,
        model: None,
        version: None,
        client: None,
        git_branch: None,
        entrypoint: None,
        codex: None,
        last_ts: first_ts.clone(),
        first_ts,
        user_count: 0,
        assistant_count: 0,
        real_user_count: 0,
        assistant_text_count: 0,
        size_bytes,
        has_transcript: true,
        archived: false,
        stats_pending: false,
        source_files: vec![path.to_path_buf()],
    };

    let mut entries = Vec::new();
    let bad_lines = agents::for_each_jsonl_line(lines, |entry| entries.push(entry));
    let version = header["version"].as_i64().unwrap_or(1);
    let mut name = None;
    let mut first_user_text = None;
    let mut events = Vec::new();
    for entry in active_branch(&entries, version) {
        if events::is_conversation_message(entry) {
            if let Some(timestamp) = events::timestamp(entry) {
                if meta.first_ts.is_none() {
                    meta.first_ts = Some(timestamp.clone());
                }
                meta.last_ts = Some(timestamp);
            }
        }
        let kind = entry["type"].as_str();
        let message = &entry["message"];
        match kind {
            Some("session_info") => name = nonempty(&entry["name"]),
            Some("model_change") => {
                if let Some(model) = nonempty(&entry["modelId"]) {
                    meta.model = Some(model.to_owned());
                }
            }
            Some("message") if message["role"] == "assistant" => {
                if let Some(model) = nonempty(&message["model"]) {
                    meta.model = Some(model.to_owned());
                }
            }
            _ => {}
        }
        for mut event in events::entry(entry, meta.cwd.as_deref(), collect_events) {
            let content = &event["message"]["content"];
            if event["type"] == "user" && context::has_user_content(content) {
                meta.user_count += 1;
                meta.real_user_count += 1;
                if first_user_text.is_none() {
                    first_user_text = context::first_text(content).map(|text| text.chars().take(80).collect());
                }
            }
            if event["type"] == "assistant" && message["role"] == "assistant" {
                meta.assistant_count += 1;
                if context::has_text(content) {
                    meta.assistant_text_count += 1;
                }
                if let Some(model) = &meta.model {
                    event["message"]["model"] = json!(model);
                }
            }
            if collect_events {
                events.push(event);
            }
        }
    }
    meta.title = name.map(str::to_owned).or(first_user_text);
    agents::tool_ops::describe_tools(&mut events, super::tools::describe);
    let parsed = ParsedPi { meta, events };
    if bad_lines > 0 {
        ParseOutcome::Degraded(parsed, agents::degraded_message(path, bad_lines))
    } else {
        ParseOutcome::Ok(parsed)
    }
}
