//! 累计用量的持久化原始计量账本。源日志消失只影响读取缓存，不删除已采集计量。
use super::{cache::FileCache, normalize, types::Sample};
use crate::agents::diagnostics;
use rusqlite::{params, Connection};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs::{self, File},
    os::fd::AsRawFd,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

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
            .map_err(|e| diagnostics::io("usage_sync", "create_ledger_directory", parent, &e))?;
        if fs::symlink_metadata(parent)
            .map_err(|e| diagnostics::io("usage_sync", "ledger_directory_metadata", parent, &e))?
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
            .map_err(|e| diagnostics::io("usage_sync", "open_sync_lock", &lock_path, &e))?;
        if !file
            .metadata()
            .map_err(|e| diagnostics::io("usage_sync", "sync_lock_metadata", &lock_path, &e))?
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
                return Err(diagnostics::io("usage_sync", "lock", &lock_path, &error));
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

/// 查询连接不持有采集器的 Mutex，不迁移/刷新索引，也不创建磁盘上的空账本。
pub(super) fn reader(path: &Path) -> Result<Connection, String> {
    let db = match fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Ledger::open(None)?.db),
        Err(e) => {
            return Err(diagnostics::io(
                "usage_query",
                "ledger_path_metadata",
                path,
                &e,
            ))
        }
        Ok(meta) if !meta.is_file() => return Err("用量账本必须是普通文件".into()),
        Ok(_) => {
            // macOS 的 /var 本身是符号链接；只规范化父目录，仍拒绝账本文件被替换成链接。
            let canonical = path
                .parent()
                .ok_or("用量账本路径无效")?
                .canonicalize()
                .map_err(|e| diagnostics::io("usage_query", "resolve_ledger_directory", path, &e))?
                .join(path.file_name().ok_or("用量账本路径无效")?);
            Connection::open_with_flags(
                canonical,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
                    | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX
                    | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW,
            )
            .map_err(|e| diagnostics::database("usage_query", "open_ledger", path, &e))?
        }
    };
    db.busy_timeout(std::time::Duration::from_secs(5))
        .map_err(|e| diagnostics::database("usage_query", "ledger_busy_timeout", path, &e))?;
    let version: i64 = db
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .map_err(|e| diagnostics::database("usage_query", "ledger_read_version", path, &e))?;
    if version > super::storage::VERSION {
        return Err("用量账本来自较新版本，请先升级应用".into());
    }
    if version < 4 {
        return Ok(Ledger::open(None)?.db);
    }
    Ok(db)
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
                .map_err(|e| {
                    diagnostics::io("usage_sync", "create_ledger_directory", parent, &e)
                })?;
            if fs::symlink_metadata(parent)
                .map_err(|e| {
                    diagnostics::io("usage_sync", "ledger_directory_metadata", parent, &e)
                })?
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
                .map_err(|e| diagnostics::io("usage_sync", "open_ledger_file", path, &e))?;
            if !file
                .metadata()
                .map_err(|e| diagnostics::io("usage_sync", "ledger_file_metadata", path, &e))?
                .is_file()
            {
                return Err("用量账本必须是普通文件".into());
            }
            fs::set_permissions(path, fs::Permissions::from_mode(0o600))
                .map_err(|e| diagnostics::io("usage_sync", "ledger_permissions", path, &e))?;
            Connection::open(path)
                .map_err(|e| diagnostics::database("usage_sync", "open_ledger", path, &e))?
        } else {
            Connection::open_in_memory().map_err(|e| {
                diagnostics::database("usage_sync", "open_ledger", Path::new(":memory:"), &e)
            })?
        };
        let error = |stage, e| {
            diagnostics::database(
                "usage_sync",
                stage,
                path.unwrap_or(Path::new(":memory:")),
                &e,
            )
        };
        db.busy_timeout(std::time::Duration::from_millis(1500))
            .map_err(|e| error("ledger_busy_timeout", e))?;
        let version: i64 = db
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .map_err(|e| error("ledger_read_version", e))?;
        if version > super::storage::VERSION {
            return Err("用量账本来自较新版本，请先升级应用".into());
        }
        // 账本是不可重建的用户数据，提交必须落盘；不能沿用可重建缓存的 MEMORY/OFF。
        // WAL 允许大批导入期间查询上一份完整快照；同步仍为 FULL，旁文件继承 0600。
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
            PRAGMA journal_size_limit=16777216;
            CREATE TABLE IF NOT EXISTS usage_ledger_sources(agent TEXT,path TEXT,data TEXT NOT NULL,PRIMARY KEY(agent,path));
            CREATE TABLE IF NOT EXISTS usage_ledger_updates(agent TEXT PRIMARY KEY,updated INTEGER NOT NULL);")
            .map_err(|e| error("ledger_initialize", e))?;
        let tx = db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|e| error("ledger_migration_lock", e))?;
        let version: i64 = tx
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .map_err(|e| error("ledger_read_version", e))?;
        if version > super::storage::VERSION {
            return Err("用量账本来自较新版本，请先升级应用".into());
        }
        super::events::schema(&tx)?;
        if version < 4 {
            tx.execute_batch(
                "ALTER TABLE usage_ledger_sources ADD COLUMN dirty INTEGER NOT NULL DEFAULT 1;
                ALTER TABLE usage_ledger_sources ADD COLUMN health TEXT;
                ALTER TABLE usage_request_events ADD COLUMN source TEXT NOT NULL DEFAULT '';
                DROP TABLE IF EXISTS usage_request_health;",
            )
            .map_err(|e| error("ledger_migrate_events", e))?;
            tx.execute("UPDATE usage_request_index SET indexed=0", [])
                .map_err(|e| error("ledger_migrate_event_index", e))?;
            tx.pragma_update(None, "user_version", 4)
                .map_err(|e| error("ledger_write_version", e))?;
        }
        tx.execute_batch("CREATE TABLE IF NOT EXISTS usage_sync_state(agent TEXT PRIMARY KEY,completed_at INTEGER NOT NULL);")
            .map_err(|e| error("ledger_initialize_sync_state", e))?;
        if version < super::storage::VERSION {
            tx.execute_batch(
                "CREATE TABLE IF NOT EXISTS usage_source_keys(
            agent TEXT NOT NULL,path TEXT NOT NULL,key TEXT NOT NULL,PRIMARY KEY(agent,path,key));
            CREATE INDEX IF NOT EXISTS usage_source_key_lookup ON usage_source_keys(agent,key,path);
            CREATE INDEX IF NOT EXISTS usage_source_dirty ON usage_ledger_sources(agent,dirty,path);
            CREATE INDEX IF NOT EXISTS usage_request_source ON usage_request_events(agent,source);",
            )
            .map_err(|e| error("ledger_initialize_source_index", e))?;
            super::storage::migrate(&tx)?;
        }
        tx.commit()
            .map_err(|e| error("ledger_migration_commit", e))?;
        super::storage::compact_migrated(&db);
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

    pub fn updated(&self, agent: &str) -> Option<i64> {
        self.db
            .query_row(
                "SELECT updated FROM usage_ledger_updates WHERE agent=?1",
                [agent],
                |r| r.get(0),
            )
            .ok()
    }

    pub fn commit(
        &mut self,
        agent: &str,
        files: &BTreeMap<PathBuf, FileCache>,
        changed: &BTreeSet<PathBuf>,
        at: i64,
    ) -> Result<(), String> {
        let tx = self
            .db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|_| "无法锁定用量账本，请稍后重试")?;
        for path in changed {
            let Some(incoming) = files.get(path) else {
                continue;
            };
            // 事务内重读，避免两个应用进程同时采集时用旧快照覆盖另一个进程的新计量。
            let existing: Option<FileCache> = tx
                .query_row(
                    "SELECT data FROM usage_ledger_sources WHERE agent=?1 AND path=?2",
                    params![agent, path.to_string_lossy()],
                    |r| super::storage::decode(r.get_ref(0)?),
                )
                .optional()
                .map_err(|_| "无法读取累计用量")?;
            let combined = match existing {
                Some(old) => merge(agent, &old, incoming),
                None => incoming.clone(),
            };
            let data = super::storage::encode(&combined)?;
            tx.execute(
                "INSERT INTO usage_ledger_sources(agent,path,data) VALUES (?1,?2,?3)
                ON CONFLICT(agent,path) DO UPDATE SET data=excluded.data,dirty=1",
                params![agent, path.to_string_lossy(), data],
            )
            .map_err(|_| "无法保存累计用量，本次操作尚未删除会话")?;
            source_keys(&tx, agent, path, &combined)?;
        }
        tx.execute(
            "INSERT OR REPLACE INTO usage_ledger_updates VALUES (?1,?2)",
            params![agent, at],
        )
        .map_err(|_| "无法保存累计用量")?;
        if !changed.is_empty() {
            tx.execute(
                "INSERT INTO usage_request_index(agent,revision,indexed) VALUES (?1,1,0)
                ON CONFLICT(agent) DO UPDATE SET revision=revision+1",
                [agent],
            )
            .map_err(|_| "无法更新请求事件版本")?;
        }
        tx.commit()
            .map_err(|_| "累计用量未能落盘，请检查磁盘空间后重试".to_owned())
    }
}

use rusqlite::OptionalExtension;

pub(super) fn source_keys(
    db: &Connection,
    agent: &str,
    path: &Path,
    file: &FileCache,
) -> Result<(), String> {
    let source: i64 = db
        .query_row(
            "SELECT id FROM usage_ledger_sources WHERE agent=?1 AND path=?2",
            params![agent, path.to_string_lossy()],
            |r| r.get(0),
        )
        .map_err(|_| "无法定位请求来源")?;
    let mut insert = db
        .prepare_cached("INSERT OR IGNORE INTO usage_source_keys VALUES (?1,?2)")
        .map_err(|_| "无法更新请求来源索引")?;
    // 保留旧身份连接：日志改写或父历史迟到后，先前生成的差额仍需一起重新归并。
    for key in normalize::source_keys(agent, path, file) {
        insert
            .execute(params![source, key])
            .map_err(|_| "无法更新请求来源索引")?;
    }
    Ok(())
}

pub(super) fn observation_ids(samples: &[Sample]) -> Vec<String> {
    let mut counts = HashMap::<String, usize>::new();
    samples
        .iter()
        .map(|sample| {
            let identity = if let Some(id) = &sample.response_id {
                format!("response:{id}")
            } else {
                let encoded = serde_json::to_vec(&(
                    sample.session.as_str(),
                    sample.timestamp,
                    sample.ordinal,
                    sample.model.as_str(),
                    &sample.tokens,
                    &sample.last_usage,
                    sample.cumulative,
                ))
                .unwrap_or_default();
                format!("{:x}", Sha256::digest(encoded))
            };
            let n = counts.entry(identity.clone()).or_default();
            *n += 1;
            format!("{identity}:{n}")
        })
        .collect()
}

/// 合并采集观测，不按当前文件/数据库剩余内容覆盖已经发生的调用。
pub(super) fn merge(agent: &str, old: &FileCache, incoming: &FileCache) -> FileCache {
    let mut merged = incoming.clone();
    if incoming.state.session.is_empty() && !old.state.session.is_empty() {
        merged.state = old.state.clone();
    }
    merged.offset = old.offset.max(incoming.offset);
    merged.failures = normalize::unique(old.failures.iter().chain(&incoming.failures).cloned());
    if agent != "codex" {
        merged.samples = normalize::unique(old.samples.iter().chain(&incoming.samples).cloned());
        return merged;
    }
    // 累计通知需要保持首次观察的原始顺序。指纹的出现次数保留同毫秒的真实相同调用，
    // 同时使追加重读、头部改写和截断后的重叠区不会被当成新增消耗。
    let mut samples = old.samples.clone();
    let mut positions: HashMap<_, _> = observation_ids(&samples)
        .into_iter()
        .enumerate()
        .map(|(i, k)| (k, i))
        .collect();
    for (key, sample) in observation_ids(&incoming.samples)
        .into_iter()
        .zip(&incoming.samples)
    {
        if let Some(&index) = positions.get(&key) {
            let mut details = samples[index].details.clone();
            details.refine(&sample.details);
            let mut request = samples[index].request.clone();
            request.merge(&sample.request);
            if normalize::prefer(sample, &samples[index]) {
                samples[index] = sample.clone();
            }
            samples[index].details = details;
            samples[index].request = request;
        } else {
            positions.insert(key, samples.len());
            samples.push(sample.clone());
        }
    }
    merged.samples = samples;
    merged
}

pub(super) fn absorb(
    agent: &str,
    history: &mut BTreeMap<PathBuf, FileCache>,
    incoming: &BTreeMap<PathBuf, FileCache>,
    changed: &mut BTreeSet<PathBuf>,
) {
    for (path, file) in incoming {
        // 同一路径可被新会话或分页复用，累计序列必须绑定完整 segment 身份。
        // 最后仍保留原文件名，Codex history_base 的物理线程 ID 可以继续解析。
        let identity = if agent == "codex" {
            normalize::segment_id(file)
        } else {
            file.state.session.clone()
        };
        let identity = format!("{:x}", Sha256::digest(identity.as_bytes()));
        let key = path
            .join(format!("ledger-{identity}"))
            .join(path.file_name().unwrap_or_default());
        if history.get(&key).is_some_and(|old| {
            old.stamps == file.stamps
                && old.problem == file.problem
                && old.parser_version == file.parser_version
        }) {
            continue;
        }
        let mut file = file.clone();
        file.origin_path = Some(path.clone());
        let combined = history
            .get(&key)
            .map(|old| merge(agent, old, &file))
            .unwrap_or_else(|| file.clone());
        history.insert(key.clone(), combined);
        changed.insert(key);
    }
}
