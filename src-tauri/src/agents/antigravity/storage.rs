use super::proto::Message;
use crate::agents::scanning::sqlite_snapshot;
use rusqlite::{Connection, OptionalExtension};
use std::{error::Error, path::Path};

pub(crate) const USER_PROMPT: i64 = 14;
pub(crate) const MODEL_RESPONSE: i64 = 15;
pub(crate) const TOOL_RESULT: i64 = 132;

pub(crate) type ReadResult<T> = Result<T, Box<dyn Error>>;

pub(crate) struct Schema {
    pub steps: Option<StepColumns>,
    pub generations: bool,
}

pub(crate) struct StepColumns {
    pub status: &'static str,
    pub error: &'static str,
}

pub(crate) fn read<T>(
    path: &Path,
    load: impl FnOnce(&Connection, Schema) -> ReadResult<T>,
) -> Result<T, String> {
    sqlite_snapshot(path, |connection| {
        let steps = if has_table(connection, "steps")? {
            let stmt = connection.prepare("SELECT * FROM steps LIMIT 0")?;
            let columns = stmt.column_names();
            let column = |name: &'static str| if columns.contains(&name) { name } else { "NULL" };
            Some(StepColumns { status: column("status"), error: column("error_details") })
        } else {
            None
        };
        let schema = Schema {
            steps,
            generations: has_table(connection, "gen_metadata")?,
        };
        if schema.steps.is_none() && !schema.generations && path.metadata()?.len() > 0 {
            return Err("缺少会话表".into());
        }
        load(connection, schema)
    })
    .map_err(|error| format!("读取 Antigravity 会话库 {} 失败：{error}", path.display()))
}

fn has_table(connection: &Connection, name: &str) -> rusqlite::Result<bool> {
    connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name=?1)",
        [name],
        |row| row.get(0),
    )
}

pub(crate) struct Trajectory {
    pub cwd: Option<String>,
    pub created: Option<i64>,
}

pub(crate) fn session_id(path: &Path) -> ReadResult<&str> {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .ok_or_else(|| "会话文件名无效".into())
}

pub(super) fn database_size(connection: &Connection) -> ReadResult<u64> {
    let bytes: i64 = connection.query_row(
        "SELECT page_count * page_size FROM pragma_page_count(), pragma_page_size()",
        [],
        |row| row.get(0),
    )?;
    Ok(bytes.max(0) as u64)
}

pub(crate) fn client_id(path: &Path) -> Option<&str> {
    path.parent()?
        .parent()?
        .file_name()?
        .to_str()
        .filter(|name| name.starts_with("antigravity"))
}

pub(crate) fn trajectory(connection: &Connection) -> ReadResult<(Trajectory, u64)> {
    let data: Vec<u8> = if has_table(connection, "trajectory_metadata_blob")? {
        connection
            .query_row(
                "SELECT data FROM trajectory_metadata_blob LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()?
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let metadata = Message::decode(&data)?;
    Ok((
        Trajectory {
            cwd: workspace(&metadata)?,
            created: metadata.millis(2)?.filter(|&time| time > 0),
        },
        data.len() as u64,
    ))
}

fn workspace(trajectory: &Message<'_>) -> std::io::Result<Option<String>> {
    let workspace = trajectory.message(1)?;
    Ok(workspace.text(1).and_then(file_path).or_else(|| trajectory.text(7).and_then(file_path)))
}

pub(super) fn file_path(uri: &str) -> Option<String> {
    let Some(path) = uri
        .strip_prefix("file://")
        .filter(|path| path.starts_with('/'))
    else {
        return uri.starts_with('/').then(|| uri.to_owned());
    };
    let mut decoded = Vec::with_capacity(path.len());
    let mut remaining = path.as_bytes();
    while let Some((&byte, rest)) = remaining.split_first() {
        if byte == b'%' {
            if let Some(value) = rest
                .get(..2)
                .and_then(|hex| std::str::from_utf8(hex).ok())
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
            {
                decoded.push(value);
                remaining = &rest[2..];
                continue;
            }
        }
        decoded.push(byte);
        remaining = rest;
    }
    String::from_utf8(decoded).ok().or_else(|| Some(path.to_owned()))
}
