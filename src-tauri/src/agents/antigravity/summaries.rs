//! 会话索引提供官方标题、工作区和派生会话关系。
use super::storage;
use crate::agents::scanning::sqlite_snapshot;
use crate::agents::{escape_path_id, ms_to_iso, SessionMeta};
use std::{collections::HashMap, fs, io::ErrorKind, path::{Path, PathBuf}};

pub(super) const DATABASE: &str = "conversation_summaries.db";
pub(super) const DATABASE_WAL: &str = "conversation_summaries.db-wal";

pub(super) struct Summary {
    title: String,
    cwd: Option<String>,
    modified: Option<i64>,
    pub(super) parent_id: Option<String>,
    pub(super) child: bool,
}

pub(super) type Index = HashMap<String, Summary>;

pub(super) fn path(root: &Path) -> PathBuf {
    if root.ends_with("conversations") {
        if let Some(parent) = root.parent() {
            return parent.join(DATABASE);
        }
    }
    root.join(DATABASE)
}

pub(super) fn read(path: &Path) -> Result<Index, String> {
    if path.metadata().is_err_and(|error| error.kind() == ErrorKind::NotFound) {
        return Ok(HashMap::new());
    }
    sqlite_snapshot(path, |connection| {
        let schema = connection.prepare("SELECT * FROM conversation_summaries LIMIT 0")?;
        let columns = schema.column_names();
        let parent = if columns.contains(&"parent_conversation_id") { "nullif(trim(parent_conversation_id),'')" } else { "NULL" };
        let nested = if columns.contains(&"nesting_depth") { "coalesce(nesting_depth,0) > 0" } else { "0" };
        let mut stmt = connection.prepare(&format!("SELECT conversation_id,title,workspace_uris,last_modified_time,
            {parent}, {nested} FROM conversation_summaries"))?;
        let rows = stmt.query_map([], |row| {
            let workspaces: Option<String> = row.get(2)?;
            let cwd = workspaces.and_then(|value| serde_json::from_str::<Vec<String>>(&value).ok())
                .and_then(|uris| uris.iter().find_map(|uri| storage::file_path(uri)));
            let modified: Option<String> = row.get(3)?;
            let modified = modified.as_deref().and_then(|value| {
                chrono::DateTime::parse_from_rfc3339(value)
                    .or_else(|_| chrono::DateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S%.f%:z"))
                    .ok()
                    .map(|date| date.timestamp_millis())
            });
            let parent_id: Option<String> = row.get(4)?;
            let child = parent_id.is_some() || row.get(5)?;
            Ok((row.get(0)?, Summary { title: row.get(1)?, cwd, modified, parent_id, child }))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }).map_err(|error| format!("无法读取 Antigravity 会话索引 {}：{error}", path.display()))
}

pub(super) fn apply(meta: &mut SessionMeta, path: &Path, index: &Index) {
    let summary = index.get(&meta.session_id);
    if let Some(summary) = summary {
        if !summary.title.trim().is_empty() {
            meta.title = Some(summary.title.clone());
        }
        if meta.cwd.is_none() {
            if let Some(cwd) = &summary.cwd {
                meta.cwd = Some(cwd.clone());
                meta.project_dir = escape_path_id(cwd);
            }
        }
    }
    if meta.last_ts.is_none() {
        let modified = summary.and_then(|summary| summary.modified).or_else(|| {
            let time = fs::metadata(path).ok()?.modified().ok()?;
            Some(chrono::DateTime::<chrono::Utc>::from(time).timestamp_millis())
        });
        meta.last_ts = modified.and_then(ms_to_iso);
    }
}
