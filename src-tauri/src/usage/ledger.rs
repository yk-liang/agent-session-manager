//! 累计用量的持久化原始计量账本。源日志消失只影响读取缓存，不删除已采集计量。
use super::{cache::FileCache, source::Stamp, storage};
use rusqlite::{params, Connection, OptionalExtension};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    os::fd::AsRawFd,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

pub(super) mod sources;

pub(super) fn path() -> Result<PathBuf, String> {
    Ok(dirs::data_dir()
        .ok_or("缺少应用数据目录")?
        .join("Agent Session Manager/usage-ledger-v1.sqlite3"))
}

/// 前台、启动任务和登录服务共用一把可取消的写锁。等待导入不是读取失败。
pub(super) struct SyncLock(File);
impl SyncLock {
    pub fn acquire(path: &Path, cancel: &dyn Fn() -> bool) -> Result<Self, String> {
        let parent = path.parent().ok_or("用量账本路径无效")?;
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)
            .map_err(|e| format!("无法创建用量账本目录：{e}"))?;
        if fs::symlink_metadata(parent)
            .map_err(|e| format!("无法读取用量账本目录：{e}"))?
            .file_type()
            .is_symlink()
        {
            return Err("用量账本目录不能是符号链接".into());
        }
        let lock_path = path.with_extension("sync.lock");
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&lock_path)
            .map_err(|e| format!("无法打开用量同步锁：{e}"))?;
        if !file
            .metadata()
            .map_err(|e| format!("无法读取用量同步锁：{e}"))?
            .is_file()
        {
            return Err("用量同步锁必须是普通文件".into());
        }
        loop {
            if cancel() {
                return Err("采集已停止".into());
            }
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
                return Ok(Self(file));
            }
            let error = std::io::Error::last_os_error();
            if !matches!(
                error.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
            ) {
                return Err(format!("无法锁定用量账本：{error}"));
            }
            std::thread::sleep(std::time::Duration::from_millis(40));
        }
    }
}
impl Drop for SyncLock {
    fn drop(&mut self) {
        unsafe {
            libc::flock(self.0.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

/// 当前版本的查询使用只读连接。已发布的旧库在首次访问时升级；缺失账本仍在内存中初始化。
pub(super) fn reader(path: &Path, cancel: &dyn Fn() -> bool) -> Result<Connection, String> {
    let db = match fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Ledger::open(None)?.db),
        Err(e) => return Err(format!("无法读取用量账本：{e}")),
        Ok(meta) if !meta.is_file() => return Err("用量账本必须是普通文件".into()),
        Ok(_) => {
            // macOS 的 /var 本身是符号链接；只规范化父目录，仍拒绝账本文件被替换成链接。
            let canonical = path
                .parent()
                .ok_or("用量账本路径无效")?
                .canonicalize()
                .map_err(|e| format!("无法解析用量账本目录：{e}"))?
                .join(path.file_name().ok_or("用量账本路径无效")?);
            Connection::open_with_flags(
                canonical,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
                    | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX
                    | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW,
            )
            .map_err(|e| format!("无法打开用量账本：{e}"))?
        }
    };
    db.busy_timeout(std::time::Duration::from_secs(5))
        .map_err(|e| format!("无法设置用量账本超时：{e}"))?;
    let version: i64 = db
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .map_err(|e| format!("无法读取用量账本版本：{e}"))?;
    if version != storage::VERSION {
        check_version(&db, version)?;
        drop(db);
        let writer = SyncLock::acquire(path, cancel)?;
        drop(Ledger::open(Some(path))?);
        drop(writer);
        return reader(path, cancel);
    }
    Ok(db)
}

fn check_version(db: &Connection, version: i64) -> Result<(), String> {
    match version {
        4 | 5 | storage::VERSION => Ok(()),
        0 if db.query_row("SELECT NOT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table')", [], |r| r.get(0))
            .map_err(|e| format!("无法读取用量账本结构：{e}"))? => Ok(()),
        version if version > storage::VERSION => Err("用量账本来自较新版本，请先升级应用".into()),
        _ => Err(format!("不支持的用量账本结构版本 {version}，已保留原数据")),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SourceWrite {
    Scan,
    Recovery,
}

pub(super) struct Ledger {
    pub(super) db: Connection,
}

impl Ledger {
    pub fn open(path: Option<&Path>) -> Result<Self, String> {
        let mut db = if let Some(path) = path {
            let parent = path.parent().ok_or("用量账本路径无效")?;
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(parent)
                .map_err(|e| format!("无法创建用量账本目录：{e}"))?;
            if fs::symlink_metadata(parent)
                .map_err(|e| format!("无法读取用量账本目录：{e}"))?
                .file_type()
                .is_symlink()
            {
                return Err("用量账本目录不能是符号链接".into());
            }
            let file = fs::OpenOptions::new()
                .write(true)
                .create(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
                .open(path)
                .map_err(|e| format!("无法打开用量账本：{e}"))?;
            if !file
                .metadata()
                .map_err(|e| format!("无法读取用量账本：{e}"))?
                .is_file()
            {
                return Err("用量账本必须是普通文件".into());
            }
            fs::set_permissions(path, fs::Permissions::from_mode(0o600))
                .map_err(|e| format!("无法设置用量账本权限：{e}"))?;
            Connection::open(path)
        } else {
            Connection::open_in_memory()
        }
        .map_err(|e| format!("无法打开用量账本：{e}"))?;
        db.busy_timeout(std::time::Duration::from_millis(1500))
            .map_err(|e| format!("无法设置用量账本超时：{e}"))?;
        let version = db.pragma_query_value(None, "user_version", |row| row.get(0))
            .map_err(|e| format!("无法读取用量账本版本：{e}"))?;
        check_version(&db, version)?;
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
            PRAGMA journal_size_limit=16777216;")
            .map_err(|e| format!("无法初始化用量账本：{e}"))?;
        if version != storage::VERSION {
            let tx = db
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .map_err(|e| format!("无法启动用量账本迁移：{e}"))?;
            let version = tx.pragma_query_value(None, "user_version", |row| row.get(0))
                .map_err(|e| format!("无法读取用量账本版本：{e}"))?;
            check_version(&tx, version)?;
            if version != storage::VERSION {
                storage::initialize(&tx, version)?;
            }
            tx.commit().map_err(|e| format!("无法提交用量账本迁移：{e}"))?;
        }
        storage::compact_migrated(&db);
        Ok(Self { db })
    }

    #[cfg(test)]
    pub fn load(&self, agent: &str) -> Result<BTreeMap<PathBuf, FileCache>, String> {
        let mut statement = self
            .db
            .prepare("SELECT path,data FROM usage_ledger_sources WHERE agent=?1")
            .map_err(|_| "无法读取累计用量")?;
        let rows = statement
            .query_map([agent], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    super::storage::decode::<FileCache>(row.get_ref(1)?)?,
                ))
            })
            .map_err(|_| "无法读取累计用量")?;
        let mut files = BTreeMap::new();
        for row in rows {
            let (path, data) = row.map_err(|_| "无法读取累计用量记录")?;
            files.insert(path.into(), data);
        }
        Ok(files)
    }

    #[cfg(test)]
    pub fn revision(&self) -> Result<(i64, i64), String> {
        super::cache::db_revision(&self.db).map_err(|_| "无法读取用量账本版本".into())
    }

    #[cfg(test)]
    pub fn updated(&self, agent: &str) -> Option<i64> {
        self.db
            .query_row(
                "SELECT updated FROM usage_ledger_updates WHERE agent=?1",
                [agent],
                |r| r.get(0),
            )
            .ok()
    }

    pub fn unchanged(&self, agent: &str, path: &Path, stamps: &[Stamp], version: u8) -> Result<bool, String> {
        let stamps = serde_json::to_string(stamps).map_err(|_| "无法编码用量来源状态")?;
        self.db.prepare_cached("SELECT 1 FROM usage_ledger_stamps WHERE agent=?1 AND path=?2 AND version=?3 AND stamps=?4")
            .and_then(|mut stmt| stmt.exists(params![agent, path.to_string_lossy(), version, stamps]))
            .map_err(|e| format!("无法读取用量来源状态：{e}"))
    }

    pub fn commit(
        &mut self,
        agent: &str,
        files: &BTreeMap<PathBuf, FileCache>,
        changed: &BTreeSet<PathBuf>,
        at: i64,
    ) -> Result<(), String> {
        let sources = changed
            .iter()
            .filter_map(|path| files.get(path).map(|file| (path, file)));
        self.write_sources(agent, sources, at, SourceWrite::Scan)
    }

    pub(super) fn restore(
        &mut self,
        agent: &str,
        files: &BTreeMap<PathBuf, FileCache>,
        at: i64,
    ) -> Result<(), String> {
        self.write_sources(agent, files.iter(), at, SourceWrite::Recovery)
    }

    fn write_sources<'a>(
        &mut self,
        agent: &str,
        sources: impl Iterator<Item = (&'a PathBuf, &'a FileCache)>,
        at: i64,
        write: SourceWrite,
    ) -> Result<(), String> {
        let tx = self
            .db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|_| "无法锁定用量账本，请稍后重试")?;
        let mut changed = false;
        for (path, incoming) in sources {
            changed = true;
            // 事务内重读，避免两个应用进程同时采集时用旧快照覆盖另一个进程的新计量。
            let existing: Option<FileCache> = tx
                .query_row(
                    "SELECT data FROM usage_ledger_sources WHERE agent=?1 AND path=?2",
                    params![agent, path.to_string_lossy()],
                    |r| super::storage::decode(r.get_ref(0)?),
                )
                .optional()
                .map_err(|_| "无法读取累计用量")?;
            let existing = match existing {
                None if write == SourceWrite::Scan && agent == "codex" => {
                    sources::previous_codex_source(&tx, path, incoming)?
                }
                existing => existing,
            };
            let combined = match (write, existing.as_ref()) {
                (SourceWrite::Scan, Some(old)) => sources::merge(agent, old, incoming),
                (SourceWrite::Recovery, Some(old)) => sources::merge_restored(agent, old, incoming),
                (_, None) => incoming.clone(),
            };
            let data = super::storage::encode(&combined)?;
            tx.execute(
                "INSERT INTO usage_ledger_sources(agent,path,data) VALUES (?1,?2,?3)
                ON CONFLICT(agent,path) DO UPDATE SET data=excluded.data,dirty=1",
                params![agent, path.to_string_lossy(), data],
            )
            .map_err(|_| "无法保存累计用量，本次操作尚未删除会话")?;
            if let Some(origin) = incoming.origin_path.as_ref()
                .filter(|_| write == SourceWrite::Scan && combined.parser_version == incoming.parser_version)
            {
                let stamps = serde_json::to_string(&incoming.stamps).map_err(|_| "无法编码用量来源状态")?;
                tx.execute(
                    "INSERT OR REPLACE INTO usage_ledger_stamps VALUES (?1,?2,?3,?4)",
                    params![agent, origin.to_string_lossy(), incoming.parser_version, stamps],
                ).map_err(|_| "无法保存用量来源状态")?;
            }
            sources::source_keys(&tx, agent, path, &combined)?;
            if write == SourceWrite::Scan && agent == "codex" && path.extension().is_some_and(|ext| ext == "zst") {
                sources::replace_covered_archives(&tx, path, incoming)?;
            }
        }
        tx.execute(
            "INSERT OR REPLACE INTO usage_ledger_updates VALUES (?1,?2)",
            params![agent, at],
        )
        .map_err(|_| "无法保存累计用量")?;
        if changed {
            tx.execute(
                "INSERT INTO usage_request_index(agent,revision,indexed) VALUES (?1,1,0)
                ON CONFLICT(agent) DO UPDATE SET revision=revision+1",
                [agent],
            )
            .map_err(|_| "无法更新请求索引版本")?;
        }
        tx.commit()
            .map_err(|_| "累计用量未能落盘，请检查磁盘空间后重试".to_owned())
    }
}
