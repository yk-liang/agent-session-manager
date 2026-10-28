//! 可丢弃的摘要索引：只保存元数据与文件版本，不复制正文或工具输出。
use super::*;
use rusqlite::{params, Connection};
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
        // 不跟随缓存文件符号链接；损坏、只读或权限异常时直接回退到日志解析。
        fs::OpenOptions::new()
            .write(true)
            .create(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)
            .ok()?;
        if !fs::symlink_metadata(path).ok()?.is_file() {
            return None;
        }
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).ok()?;
        let db = Connection::open(path).ok()?;
        db.busy_timeout(std::time::Duration::from_millis(50)).ok()?;
        db.execute_batch(
            "PRAGMA journal_mode=MEMORY; PRAGMA synchronous=OFF;
            CREATE TABLE IF NOT EXISTS summaries (key TEXT PRIMARY KEY, revision TEXT NOT NULL,
                meta TEXT NOT NULL, touched INTEGER NOT NULL);",
        )
        .ok()?;
        Some(Self(Mutex::new(db)))
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
    fn disk_survives_reopen_but_rejects_changes_and_broken_database() {
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
        assert!(Store::open(&path).is_none());
    }
}
