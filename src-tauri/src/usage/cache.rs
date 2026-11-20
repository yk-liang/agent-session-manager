use super::{source::Stamp, types::*};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

pub(super) const VERSION: i64 = 2;
pub(super) const SCHEMA: &str = "CREATE TABLE usage_files(
    scope TEXT,path TEXT,data BLOB NOT NULL,touched INTEGER NOT NULL,PRIMARY KEY(scope,path));
    CREATE TABLE usage_scans(scope TEXT PRIMARY KEY,updated INTEGER NOT NULL);";
const MAX_CACHED_USAGE_BYTES: i64 = 64 * 1024 * 1024;

pub(super) fn path() -> Option<PathBuf> {
    dirs::cache_dir()
        .map(|dir| dir.join("com.liangyukun.agent-session-manager/token-usage-v1.sqlite3"))
}

#[derive(Clone, Default, Deserialize, Serialize)]
pub(super) struct FileCache {
    /// 解析规则更新时重新读取现有来源。
    #[serde(default)]
    pub parser_version: u8,
    /// 账本中的来源键包含会话身份，原始路径仅用于解析父历史关系，不再用于读取正文。
    #[serde(default)]
    pub origin_path: Option<PathBuf>,
    pub stamps: Vec<Stamp>,
    /// 完整换行的字节边界；未完成尾行留到下一次追加再读取。
    pub offset: u64,
    pub head_len: u64,
    pub head: String,
    pub tail: String,
    pub pending: bool,
    pub state: ParserState,
    pub samples: Vec<Sample>,
    /// 保留缺少用量的失败记录，汇总时跳过其计量。
    #[serde(default)]
    pub failures: Vec<Sample>,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct StoreRevision {
    ledger: (i64, i64),
    cache: (i64, i64),
}
#[cfg(test)]
impl StoreRevision {
    pub fn same_external(&self, other: &Self) -> bool {
        self.ledger.0 == other.ledger.0 && self.cache.0 == other.cache.0
    }
}
#[cfg(test)]
pub(super) fn db_revision(db: &Connection) -> rusqlite::Result<(i64, i64)> {
    db.query_row(
        "SELECT data_version,total_changes() FROM pragma_data_version",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
}

pub(crate) struct Store {
    db: Connection,
    pending_cache: Option<PathBuf>,
    ledger_path: Option<PathBuf>,
    ledger: Result<super::ledger::Ledger, String>,
    #[cfg(test)]
    pub(super) snapshot: Option<super::ReportSnapshot>,
    #[cfg(test)]
    timezone: Option<(String, std::time::Instant)>,
    #[cfg(test)]
    pub(super) report_builds: usize,
    #[cfg(test)]
    pub(super) file_loads: std::cell::Cell<usize>,
}
impl Store {
    #[cfg(test)]
    pub fn open(path: Option<&Path>) -> Self {
        let ledger = path.map(|p| p.with_extension("ledger.sqlite3"));
        Self::open_with_ledger(path, ledger.as_deref())
    }
    pub fn open_with_ledger(path: Option<&Path>, ledger_path: Option<&Path>) -> Self {
        let ledger = super::ledger::Ledger::open(ledger_path);
        let disk = path.filter(|_| ledger.is_ok()).and_then(|path| {
            let db = Self::open_disk(path).ok()?;
            (db.pragma_query_value::<i64, _>(None, "user_version", |r| r.get(0)) == Ok(VERSION))
                .then_some(db)
        });
        let (db, pending_cache) = match disk {
            Some(db) => {
                Self::tune(&db);
                (db, None)
            }
            None => (Self::memory(), path.map(Path::to_owned)),
        };
        Self {
            db,
            pending_cache,
            ledger_path: ledger_path.map(Path::to_owned),
            ledger,
            #[cfg(test)]
            snapshot: None,
            #[cfg(test)]
            timezone: None,
            #[cfg(test)]
            report_builds: 0,
            #[cfg(test)]
            file_loads: std::cell::Cell::new(0),
        }
    }

    fn memory() -> Connection {
        let db = Connection::open_in_memory().expect("In-memory usage cache");
        db.execute_batch(SCHEMA).expect("Usage cache schema");
        db
    }

    fn open_disk(path: &Path) -> super::storage::MigrationResult<Connection> {
        let parent = path.parent().ok_or("解析缓存路径无效")?;
        fs::DirBuilder::new().recursive(true).mode(0o700).create(parent)?;
        let file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)?;
        if !file.metadata()?.is_file() {
            return Err("解析缓存必须是普通文件".into());
        }
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
        let canonical = parent.canonicalize()?.join(path.file_name().ok_or("解析缓存路径无效")?);
        let db = Connection::open_with_flags(canonical, rusqlite::OpenFlags::default()
            | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW)?;
        db.busy_timeout(std::time::Duration::from_millis(200))?;
        Ok(db)
    }

    pub(super) fn ledger(&mut self) -> Result<&mut super::ledger::Ledger, String> {
        if self.ledger.is_err() {
            #[cfg(test)]
            {
                self.snapshot = None;
            }
            self.ledger = super::ledger::Ledger::open(self.ledger_path.as_deref());
        }
        self.ledger.as_mut().map_err(|e| e.clone())
    }
    #[cfg(test)]
    pub(super) fn revision(&mut self) -> Result<StoreRevision, String> {
        Ok(StoreRevision {
            ledger: self.ledger()?.revision()?,
            cache: db_revision(&self.db).map_err(|_| "无法读取用量缓存版本")?,
        })
    }
    #[cfg(test)]
    pub(super) fn timezone_ready(&mut self, key: &str, now: std::time::Instant) -> bool {
        let (previous, ready_at) = self.timezone.get_or_insert_with(|| (key.into(), now));
        if previous != key {
            *previous = key.into();
            // Chrono 的线程本地时区缓存最多滞后一秒，不把过渡期结果长期缓存。
            *ready_at = now + std::time::Duration::from_secs(2);
        }
        now >= *ready_at
    }
    fn tune(db: &Connection) {
        // 当前缓存可重建，整理失败时仍继续使用现有表。
        if db.pragma_query_value::<i64, _>(None, "auto_vacuum", |row| row.get(0)).is_ok_and(|mode| mode != 2) {
            let _ = db.execute_batch("PRAGMA auto_vacuum=INCREMENTAL; VACUUM;");
        }
        let _ = db.execute_batch("PRAGMA journal_mode=MEMORY; PRAGMA synchronous=OFF;");
    }

    pub(super) fn recover_released_cache(
        &mut self,
        agent: &str,
        roots: &[PathBuf],
        cancel: &dyn Fn() -> bool,
    ) -> Result<(), String> {
        if cancel() {
            return Err("采集已停止".into());
        }
        let Some(path) = self.pending_cache.clone() else { return Ok(()) };
        self.ledger()?;
        let mut cache = Self::open_disk(&path)
            .map_err(|error| format!("无法打开解析缓存，已保留原数据：{error}"))?;
        let ledger = if self.ledger_path.is_some() {
            Some(self.ledger()?)
        } else {
            None
        };
        super::storage::released_cache::upgrade(
            &mut cache, ledger, agent, roots, cancel,
        )
        .map_err(|error| format!("无法打开或恢复解析缓存，已保留原数据：{error}"))?;
        Self::tune(&cache);
        self.db = cache;
        self.pending_cache = None;
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn load(&self, scope: &str) -> BTreeMap<PathBuf, FileCache> {
        let Ok(mut stmt) = self
            .db
            .prepare("SELECT path,data FROM usage_files WHERE scope=?1")
        else {
            return BTreeMap::new();
        };
        let Ok(rows) = stmt.query_map([scope], |r| {
            Ok((
                r.get::<_, String>(0)?,
                super::storage::decode::<FileCache>(r.get_ref(1)?)?,
            ))
        }) else {
            return BTreeMap::new();
        };
        rows.filter_map(Result::ok)
            .map(|(p, file)| (PathBuf::from(p), file))
            .collect()
    }
    pub(super) fn load_file(&self, scope: &str, path: &Path) -> Option<FileCache> {
        #[cfg(test)]
        self.file_loads.set(self.file_loads.get() + 1);
        self.db
            .query_row(
                "SELECT data FROM usage_files WHERE scope=?1 AND path=?2",
                params![scope, path.to_string_lossy()],
                |r| super::storage::decode(r.get_ref(0)?),
            )
            .ok()
    }
    pub(super) fn commit(
        &mut self,
        scope: &str,
        files: &BTreeMap<PathBuf, FileCache>,
        changed: &std::collections::BTreeSet<PathBuf>,
        deleted: &[PathBuf],
        at: i64,
    ) {
        let Ok(tx) = self.db.transaction() else {
            return;
        };
        let save = || -> rusqlite::Result<()> {
            for path in deleted {
                tx.execute(
                    "DELETE FROM usage_files WHERE scope=?1 AND path=?2",
                    params![scope, path.to_string_lossy()],
                )?;
            }
            for path in changed {
                if let Some(file) = files.get(path) {
                    let Ok(data) = super::storage::encode(file) else {
                        continue;
                    };
                    if data.len() as i64 <= MAX_CACHED_USAGE_BYTES {
                        tx.execute(
                            "INSERT OR REPLACE INTO usage_files VALUES (?1,?2,?3,?4)",
                            params![scope, path.to_string_lossy(), data, at],
                        )?;
                    } else {
                        tx.execute(
                            "DELETE FROM usage_files WHERE scope=?1 AND path=?2",
                            params![scope, path.to_string_lossy()],
                        )?;
                    }
                }
            }
            tx.execute(
                "INSERT OR REPLACE INTO usage_scans VALUES (?1,?2)",
                params![scope, at],
            )?;
            Self::trim(&tx, MAX_CACHED_USAGE_BYTES)?;
            tx.execute("DELETE FROM usage_files WHERE scope IN (SELECT scope FROM usage_scans ORDER BY rowid DESC LIMIT -1 OFFSET 10)", [])?;
            tx.execute("DELETE FROM usage_scans WHERE scope IN (SELECT scope FROM usage_scans ORDER BY rowid DESC LIMIT -1 OFFSET 10)", [])?;
            Ok(())
        };
        if save().is_ok() {
            let _ = tx.commit();
        } else {
            drop(tx);
        }
        // 限制已淘汰缓存留下的空闲页；每次最多回收 1 MiB，避免大整理阻塞增量采集。
        self.reclaim();
    }

    pub(super) fn trim(db: &Connection, budget: i64) -> rusqlite::Result<()> {
        let mut bytes: i64 = db.query_row(
            "SELECT coalesce(sum(length(data)),0) FROM usage_files",
            [],
            |r| r.get(0),
        )?;
        if bytes <= budget {
            return Ok(());
        }
        let mut stmt = db.prepare(
            "SELECT rowid,length(data) FROM usage_files ORDER BY touched,rowid",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))?;
        for row in rows {
            if bytes <= budget {
                break;
            }
            let (id, len) = row?;
            db.execute("DELETE FROM usage_files WHERE rowid=?1", [id])?;
            bytes -= len;
        }
        Ok(())
    }

    fn reclaim(&self) {
        // 此 PRAGMA 每回收一页返回一行，必须读到结束；execute_batch 只会执行第一步。
        let run = || -> rusqlite::Result<()> {
            let mut stmt = self.db.prepare("PRAGMA incremental_vacuum(256)")?;
            let mut rows = stmt.query([])?;
            while rows.next()?.is_some() {}
            Ok(())
        };
        let _ = run();
    }

    /// 仅在完整发现和采集成功后清理消失来源；指定文件通知不能代表完整目录。
    pub(super) fn prune_missing(
        &mut self,
        scope: &str,
        paths: &std::collections::BTreeSet<PathBuf>,
    ) {
        let Ok(tx) = self.db.transaction() else {
            return;
        };
        let prune = || -> rusqlite::Result<()> {
            let mut stmt = tx.prepare("SELECT path FROM usage_files WHERE scope=?1")?;
            let obsolete = stmt
                .query_map([scope], |r| r.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            for path in obsolete {
                if !paths.contains(Path::new(&path)) {
                    tx.execute(
                        "DELETE FROM usage_files WHERE scope=?1 AND path=?2",
                        params![scope, path],
                    )?;
                }
            }
            Ok(())
        };
        if prune().is_ok() {
            let _ = tx.commit();
        } else {
            drop(tx);
        }
        self.reclaim();
    }
}
