use super::proto::Message;
use rusqlite::{Connection, ErrorCode, OpenFlags, OptionalExtension};
use std::{error::Error, fmt::Write, fs, os::unix::fs::MetadataExt, path::Path, time::Duration};

pub(crate) const USER_PROMPT: i64 = 14;
pub(crate) const MODEL_RESPONSE: i64 = 15;
pub(crate) const TOOL_RESULT: i64 = 132;

pub(crate) type ReadResult<T> = Result<T, Box<dyn Error>>;

pub(crate) struct Schema {
    pub steps: bool,
    pub generations: bool,
}

fn open(path: &Path, flags: OpenFlags) -> rusqlite::Result<Connection> {
    let connection = Connection::open_with_flags(path, flags)?;
    connection.busy_timeout(Duration::from_millis(300))?;
    connection.execute_batch("BEGIN")?;
    // SQLite 延迟到第一次读取才打开 WAL，提前确认可读并固定快照。
    connection.query_row("SELECT count(*) FROM sqlite_schema", [], |row| {
        row.get::<_, i64>(0)
    })?;
    Ok(connection)
}

fn immutable_uri(path: &Path) -> String {
    let mut uri = String::from("file:");
    for &byte in path.as_os_str().as_encoded_bytes() {
        if byte.is_ascii_alphanumeric() || b"/-._~".contains(&byte) {
            uri.push(char::from(byte));
        } else {
            write!(uri, "%{byte:02X}").unwrap();
        }
    }
    uri.push_str("?immutable=1");
    uri
}

fn revision(meta: fs::Metadata) -> (u64, u64, u64, i64, i64, i64, i64) {
    (
        meta.dev(),
        meta.ino(),
        meta.len(),
        meta.mtime(),
        meta.mtime_nsec(),
        meta.ctime(),
        meta.ctime_nsec(),
    )
}

pub(crate) fn read<T>(
    path: &Path,
    load: impl FnOnce(&Connection, Schema) -> ReadResult<T>,
) -> Result<T, String> {
    snapshot(path, |connection| {
        let schema = Schema {
            steps: has_table(connection, "steps")?,
            generations: has_table(connection, "gen_metadata")?,
        };
        if !schema.steps && !schema.generations && path.metadata()?.len() > 0 {
            return Err("缺少会话表".into());
        }
        load(connection, schema)
    })
    .map_err(|error| format!("读取 Antigravity 会话库 {} 失败：{error}", path.display()))
}

fn snapshot<T>(path: &Path, load: impl FnOnce(&Connection) -> ReadResult<T>) -> ReadResult<T> {
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let error = match open(path, flags) {
        Ok(connection) => return load(&connection),
        Err(error) => error,
    };
    let wal = crate::agents::scanning::sqlite_wal_path(path);
    if error.sqlite_error_code() != Some(ErrorCode::CannotOpen) || wal.try_exists()? {
        return Err(error.into());
    }
    // 旧会话可能保留 WAL 模式的库头，但已无 WAL 文件。只在主库静止时读取。
    let before = revision(fs::metadata(path)?);
    let uri = immutable_uri(path);
    let connection = open(Path::new(&uri), flags | OpenFlags::SQLITE_OPEN_URI)?;
    let result = load(&connection)?;
    if wal.try_exists()? || revision(fs::metadata(path)?) != before {
        return Err("会话库正在写入，请刷新后重试".into());
    }
    Ok(result)
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
    let Some(uri) = workspace.text(1) else {
        return Ok(None);
    };
    let Some(path) = uri
        .strip_prefix("file://")
        .filter(|path| path.starts_with('/'))
    else {
        return Ok(uri.starts_with('/').then(|| uri.to_owned()));
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
    Ok(String::from_utf8(decoded)
        .ok()
        .or_else(|| Some(path.to_owned())))
}
