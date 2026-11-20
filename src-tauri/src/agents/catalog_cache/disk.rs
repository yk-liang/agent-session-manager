//! 可丢弃的摘要索引：只保存元数据与文件版本，不复制正文或工具输出。
use super::*;
use rusqlite::{params, Connection, ErrorCode, OpenFlags};
use sha2::{Digest, Sha256};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};

pub(super) struct Store(Mutex<Connection>);
impl Store {
    fn open(path: &std::path::Path) -> Option<Self> {
        let parent = path.parent()?;
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)
            .ok()?;
        let path = parent.canonicalize().ok()?.join(path.file_name()?);
        let path = path.as_path();
        let file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)
            .ok()?;
        let meta = file.metadata().ok()?;
        if !meta.is_file() || meta.nlink() != 1 {
            return None;
        }
        if meta.permissions().mode() & 0o777 != 0o600 {
            file.set_permissions(fs::Permissions::from_mode(0o600)).ok()?;
        }
        let meta = file.metadata().ok()?;
        match Self::connect(path) {
            Ok(store) => Some(store),
            Err(error) if matches!(error.sqlite_error_code(),
                Some(ErrorCode::NotADatabase | ErrorCode::DatabaseCorrupt)) => {
                let current = fs::symlink_metadata(path).ok()?;
                if !current.is_file() || current.ino() != meta.ino() || current.dev() != meta.dev()
                    || current.nlink() != 1
                    || current.ctime() != meta.ctime() || current.ctime_nsec() != meta.ctime_nsec()
                {
                    return None;
                }
                file.set_len(0).ok()?;
                Self::connect(path).ok()
            }
            Err(_) => None,
        }
    }

    fn connect(path: &std::path::Path) -> rusqlite::Result<Self> {
        let db = Connection::open_with_flags(path,
            OpenFlags::default() | OpenFlags::SQLITE_OPEN_NOFOLLOW)?;
        db.busy_timeout(std::time::Duration::from_millis(50))?;
        db.execute_batch(
            "PRAGMA journal_mode=MEMORY; PRAGMA synchronous=OFF;
            CREATE TABLE IF NOT EXISTS summaries (key TEXT PRIMARY KEY, revision TEXT NOT NULL,
                meta TEXT NOT NULL, touched INTEGER NOT NULL);",
        )?;
        Ok(Self(Mutex::new(db)))
    }

    fn key(kind: &str, paths: &[PathBuf]) -> String {
        // 解析语义改变时升级版本，旧索引不参与读取。
        let bytes = serde_json::to_vec(&(3, kind, paths)).unwrap_or_default();
        format!("{:x}", Sha256::digest(bytes))
    }
    pub(super) fn get(
        &self,
        kind: &str,
        paths: &[PathBuf],
        revision: &[Revision],
    ) -> Option<SessionMeta> {
        let db = self.0.lock().ok()?;
        let (saved, json): (String, String) = db
            .query_row(
                "SELECT revision, meta FROM summaries WHERE key=?1",
                [Self::key(kind, paths)],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .ok()?;
        if saved != serde_json::to_string(revision).ok()? {
            return None;
        }
        let mut meta: SessionMeta = serde_json::from_str(&json).ok()?;
        meta.source_files = paths.to_vec();
        Some(meta)
    }
    pub(super) fn put(
        &self,
        kind: &str,
        paths: &[PathBuf],
        revision: &[Revision],
        meta: &SessionMeta,
    ) {
        let Ok(db) = self.0.lock() else { return };
        let (Ok(revision), Ok(json)) =
            (serde_json::to_string(revision), serde_json::to_string(meta))
        else {
            return;
        };
        let _ = db.execute(
            "INSERT OR REPLACE INTO summaries VALUES (?1,?2,?3,unixepoch())",
            params![Self::key(kind, paths), revision, json],
        );
        let _ = db.execute("DELETE FROM summaries WHERE key IN (SELECT key FROM summaries ORDER BY touched DESC LIMIT -1 OFFSET 4096)", []);
    }
}

pub(super) fn store() -> Option<&'static Store> {
    // 单元测试不得向真实用户缓存写入临时路径。
    if cfg!(test) {
        return None;
    }
    static STORE: OnceLock<Option<Store>> = OnceLock::new();
    STORE
        .get_or_init(|| {
            Store::open(
                &dirs::cache_dir()?
                    .join("com.liangyukun.agent-session-manager/session-metadata-v3.sqlite3"),
            )
        })
        .as_ref()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn disk_survives_reopen_and_rebuilds_a_broken_database() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.sqlite");
        let log = dir.path().join("log");
        fs::write(&log, "one").unwrap();
        let paths = vec![log.clone()];
        let rev = revisions(&paths).unwrap();
        let meta: SessionMeta = serde_json::from_value(serde_json::json!({
            "sessionId":"test", "projectDir":"p", "cwd":null,"title":"hello","model":null,
            "version":null,"gitBranch":null,"entrypoint":null,"firstTs":null,"lastTs":null,
            "userCount":1,"assistantCount":1,"realUserCount":1,"assistantTextCount":1,
            "sizeBytes":3,"hasTranscript":true
        }))
        .unwrap();
        Store::open(&path).unwrap().put("test", &paths, &rev, &meta);
        let reopened = Store::open(&path).unwrap();
        assert_eq!(
            reopened.get("test", &paths, &rev).unwrap().source_files,
            paths
        );
        fs::write(&log, "two").unwrap();
        assert!(reopened
            .get("test", &paths, &revisions(&paths).unwrap())
            .is_none());
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        drop(reopened);
        fs::write(&path, "broken cache").unwrap();
        let rebuilt = Store::open(&path).unwrap();
        let rev = revisions(&paths).unwrap();
        assert!(rebuilt.get("test", &paths, &rev).is_none());
        rebuilt.put("test", &paths, &rev, &meta);
        assert!(rebuilt.get("test", &paths, &rev).is_some());
    }
}
