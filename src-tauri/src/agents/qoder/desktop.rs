//! Qoder 桌面端将已命名标题独立保存在 SQLite 中；这里只补展示标题，不改日志身份。
use crate::agents::SessionMeta;
use rusqlite::{params_from_iter, Connection, OpenFlags};
use std::{
    collections::{BTreeSet, HashMap},
    path::{Path, PathBuf},
    time::Duration,
};

pub(super) fn database_path() -> Option<PathBuf> {
    dirs::config_dir().map(|root| root.join("com.qoder.app.stable/main.sqlite"))
}

pub(super) fn apply(path: Option<&Path>, sessions: &mut [SessionMeta], warnings: &mut Vec<String>) {
    let Some(path) = path else {
        return;
    };
    if sessions.is_empty() {
        return;
    }
    match std::fs::metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Ok(meta) if meta.is_file() => {}
        Ok(_) => {
            warnings.push("Qoder 桌面会话数据库不是普通文件，暂用日志标题".into());
            return;
        }
        Err(error) => {
            warnings.push(format!(
                "无法读取 Qoder 桌面会话名（{}）：{error}；暂用日志标题",
                path.display()
            ));
            return;
        }
    }
    match read_titles(path, sessions) {
        Ok(titles) => {
            for session in sessions {
                if let Some(title) = titles.get(&session.session_id) {
                    session.title = Some(title.clone());
                }
            }
        }
        Err(error) => warnings.push(format!(
            "无法读取 Qoder 桌面会话名（{}）：{error}；暂用日志标题",
            path.display()
        )),
    }
}

fn read_titles(path: &Path, sessions: &[SessionMeta]) -> Result<HashMap<String, String>, String> {
    let read = || -> rusqlite::Result<HashMap<String, String>> {
        // 不使用 immutable：桌面端正在运行时，最新改名可能仍只存在 WAL 中。
        let db = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        db.busy_timeout(Duration::from_millis(150))?;
        let tx = db.unchecked_transaction()?;
        let columns: BTreeSet<String> = tx
            .prepare("PRAGMA table_info(chat_sessions)")?
            .query_map([], |row| row.get(1))?
            .collect::<Result<_, _>>()?;
        if !columns.contains("session_id") || !columns.contains("title") {
            return Err(rusqlite::Error::InvalidColumnName(
                "chat_sessions.session_id/title".into(),
            ));
        }
        let ids: Vec<_> = sessions
            .iter()
            .map(|s| s.session_id.as_str())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let mut titles = HashMap::new();
        for chunk in ids.chunks(200) {
            // 只查询已枚举日志的精确 ID，既不载入正文，也不按 origin_session_id 猜测父子归属。
            let placeholders = vec!["?"; chunk.len()].join(",");
            let visible = if columns.contains("deleted_at") {
                " AND deleted_at IS NULL"
            } else {
                ""
            };
            let sql = format!("SELECT session_id,substr(title,1,4097) FROM chat_sessions WHERE session_id IN ({placeholders}) AND typeof(title)='text'{visible}");
            let mut query = tx.prepare(&sql)?;
            let rows = query.query_map(params_from_iter(chunk.iter()), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            for row in rows {
                let (id, title) = row?;
                let title = title.trim();
                if !title.is_empty() && title.chars().count() <= 4096 && !title.contains('\0') {
                    titles.insert(id, title.to_string());
                }
            }
        }
        Ok(titles)
    };
    read().map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn fixture(root: &Path, id: &str) {
        let project = root.join("projects/source");
        fs::create_dir_all(&project).unwrap();
        fs::write(project.join(format!("{id}.jsonl")), format!("{}\n{}\n",
            serde_json::json!({"type":"user","sessionId":id,"message":{"content":"Initial task"}}),
            serde_json::json!({"type":"custom-title","customTitle":"Log title"}))).unwrap();
    }

    #[test]
    fn wal_only_rename_updates_cached_transcript_and_detail_without_writing_the_database() {
        let root = tempfile::tempdir().unwrap();
        fixture(root.path(), "aaaa");
        let path = root.path().join("main.sqlite");
        let writer = Connection::open(&path).unwrap();
        writer.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0; CREATE TABLE chat_sessions(session_id TEXT PRIMARY KEY,title TEXT,deleted_at INTEGER); INSERT INTO chat_sessions VALUES('aaaa','Desktop title',NULL)").unwrap();
        let transcript = root.path().join("projects/source/aaaa.jsonl");
        let original = fs::read(&transcript).unwrap();
        let load = || super::super::load_catalog(root.path(), Some(&path), false).unwrap();
        assert_eq!(load().sessions[0].title.as_deref(), Some("Desktop title"));
        writer
            .execute(
                "UPDATE chat_sessions SET title=?1 WHERE session_id='aaaa'",
                ["Renamed in desktop"],
            )
            .unwrap();
        let db_before = fs::read(&path).unwrap();
        let wal = path.with_file_name("main.sqlite-wal");
        let wal_before = fs::read(&wal).unwrap();
        assert_eq!(
            load().sessions[0].title.as_deref(),
            Some("Renamed in desktop")
        );
        let detail =
            super::super::load_session(root.path(), Some(&path), "source", "aaaa").unwrap();
        assert_eq!(detail.meta.title.as_deref(), Some("Renamed in desktop"));
        assert_eq!(fs::read(&transcript).unwrap(), original);
        assert_eq!(fs::read(&path).unwrap(), db_before);
        assert_eq!(fs::read(&wal).unwrap(), wal_before);
    }

    #[test]
    fn only_exact_non_deleted_nonempty_titles_overlay_existing_log_sessions() {
        let root = tempfile::tempdir().unwrap();
        for id in ["aaaa", "bbbb", "cccc", "dddd"] {
            fixture(root.path(), id);
        }
        let path = root.path().join("main.sqlite");
        let writer = Connection::open(&path).unwrap();
        writer.execute_batch("CREATE TABLE chat_sessions(session_id TEXT PRIMARY KEY,origin_session_id TEXT,title TEXT,deleted_at INTEGER);
            INSERT INTO chat_sessions VALUES('aaaa',NULL,'Desktop title',NULL);
            INSERT INTO chat_sessions VALUES('bbbb',NULL,'Deleted title',1);
            INSERT INTO chat_sessions VALUES('cccc',NULL,'   ',NULL);
            INSERT INTO chat_sessions VALUES('other','dddd','Wrong parent title',NULL)").unwrap();
        let data = super::super::load_catalog(root.path(), Some(&path), false).unwrap();
        assert_eq!(data.sessions.len(), 4);
        for session in data.sessions {
            assert_eq!(
                session.title.as_deref(),
                Some(if session.session_id == "aaaa" {
                    "Desktop title"
                } else {
                    "Log title"
                })
            );
        }
    }

    #[test]
    fn missing_legacy_corrupt_or_locked_metadata_preserves_log_titles() {
        let root = tempfile::tempdir().unwrap();
        fixture(root.path(), "aaaa");
        let path = root.path().join("main.sqlite");
        let load = || super::super::load_catalog(root.path(), Some(&path), false).unwrap();
        let missing = load();
        assert_eq!(missing.sessions[0].title.as_deref(), Some("Log title"));
        assert!(missing.warnings.is_empty());
        assert!(
            !path.exists(),
            "Read-only metadata lookup must not create an app database"
        );
        fs::write(&path, b"not a sqlite database").unwrap();
        let corrupt = load();
        assert_eq!(corrupt.sessions[0].title.as_deref(), Some("Log title"));
        assert_eq!(corrupt.warnings.len(), 1);
        assert_eq!(fs::read(&path).unwrap(), b"not a sqlite database");
        fs::remove_file(&path).unwrap();
        let writer = Connection::open(&path).unwrap();
        writer.execute_batch("CREATE TABLE chat_sessions(session_id TEXT PRIMARY KEY,title TEXT); INSERT INTO chat_sessions VALUES('aaaa','Legacy desktop title')").unwrap();
        assert_eq!(
            load().sessions[0].title.as_deref(),
            Some("Legacy desktop title")
        );
        writer
            .execute_batch("BEGIN EXCLUSIVE; UPDATE chat_sessions SET title='Uncommitted title'")
            .unwrap();
        let started = std::time::Instant::now();
        let locked = load();
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(locked.sessions[0].title.as_deref(), Some("Log title"));
        assert_eq!(locked.warnings.len(), 1);
        writer.execute_batch("ROLLBACK").unwrap();
        assert_eq!(
            load().sessions[0].title.as_deref(),
            Some("Legacy desktop title")
        );
    }
}
