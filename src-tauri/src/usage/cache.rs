use super::{parse, types::*};
use crate::agents::open_jsonl;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::{
    collections::BTreeMap,
    fs,
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(super) struct Stamp {
    pub len: u64,
    modified: i64,
    modified_ns: i64,
    inode: u64,
    device: u64,
    changed: i64,
    changed_ns: i64,
}
pub(super) fn stamp(path: &Path) -> std::io::Result<Stamp> {
    let m = fs::metadata(path)?;
    if !m.is_file() {
        return Err(std::io::Error::other("Source is not a regular file"));
    }
    Ok(Stamp {
        len: m.len(),
        modified: m.mtime(),
        modified_ns: m.mtime_nsec(),
        inode: m.ino(),
        device: m.dev(),
        changed: m.ctime(),
        changed_ns: m.ctime_nsec(),
    })
}
#[derive(Clone, Default, Deserialize, Serialize)]
pub(super) struct FileCache {
    /// 缓存结构/提取语义升级时重读现存源；不改变累计账本或旧缓存的定位键。
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
    /// 没有 usage 的失败记录也需要保留，但不参与 token 汇总。
    #[serde(default)]
    pub failures: Vec<Sample>,
    pub problem: Option<String>,
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
        let disk = path.and_then(|path| {
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(path.parent()?)
                .ok()?;
            let file = fs::OpenOptions::new()
                .write(true)
                .create(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
                .open(path)
                .ok()?;
            if !file.metadata().ok()?.is_file() {
                return None;
            }
            fs::set_permissions(path, fs::Permissions::from_mode(0o600)).ok()?;
            let db = Connection::open(path).ok()?;
            Self::prepare(&db).ok()?;
            Some(db)
        });
        let db = disk.unwrap_or_else(|| {
            let c = Connection::open_in_memory().expect("In-memory usage cache");
            Self::prepare(&c).expect("Usage cache schema");
            c
        });
        Self {
            db,
            ledger_path: ledger_path.map(Path::to_owned),
            ledger: super::ledger::Ledger::open(ledger_path),
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
    fn prepare(db: &Connection) -> rusqlite::Result<()> {
        db.busy_timeout(std::time::Duration::from_millis(200))?;
        // 可重建索引，不使用 WAL，避免每次写数字生成额外敏感文件。
        db.execute_batch("PRAGMA journal_mode=MEMORY; PRAGMA synchronous=OFF;
            CREATE TABLE IF NOT EXISTS usage_files(scope TEXT,path TEXT,data TEXT NOT NULL,PRIMARY KEY(scope,path));
            CREATE TABLE IF NOT EXISTS usage_file_stamps(scope TEXT,path TEXT,version INTEGER,stamps TEXT,PRIMARY KEY(scope,path));
            CREATE TABLE IF NOT EXISTS usage_scans(scope TEXT PRIMARY KEY,updated INTEGER NOT NULL);")
    }
    pub(super) fn load(&self, scope: &str) -> BTreeMap<PathBuf, FileCache> {
        let Ok(mut stmt) = self
            .db
            .prepare("SELECT path,data FROM usage_files WHERE scope=?1")
        else {
            return BTreeMap::new();
        };
        let Ok(rows) = stmt.query_map([scope], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        }) else {
            return BTreeMap::new();
        };
        rows.filter_map(Result::ok)
            .filter_map(|(p, s)| serde_json::from_str(&s).ok().map(|v| (PathBuf::from(p), v)))
            .collect()
    }
    pub(super) fn load_file(&self, scope: &str, path: &Path) -> Option<FileCache> {
        #[cfg(test)]
        self.file_loads.set(self.file_loads.get() + 1);
        self.db
            .query_row(
                "SELECT data FROM usage_files WHERE scope=?1 AND path=?2",
                params![scope, path.to_string_lossy()],
                |r| r.get::<_, String>(0),
            )
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
    }
    /// 未变化来源只读这个小索引，不反序列化该文件的全部历史计量。
    pub(super) fn unchanged(
        &self,
        scope: &str,
        path: &Path,
        stamps: &[Stamp],
        version: u8,
    ) -> bool {
        let Ok(stamps) = serde_json::to_string(stamps) else {
            return false;
        };
        self.db.query_row("SELECT 1 FROM usage_file_stamps WHERE scope=?1 AND path=?2 AND version=?3 AND stamps=?4",
            params![scope,path.to_string_lossy(),version,stamps], |_| Ok(true)).unwrap_or(false)
    }
    #[cfg(test)]
    pub fn updated(&self, scope: &str) -> Option<i64> {
        self.db
            .query_row(
                "SELECT updated FROM usage_scans WHERE scope=?1",
                [scope],
                |r| r.get(0),
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
                    "DELETE FROM usage_file_stamps WHERE scope=?1 AND path=?2",
                    params![scope, path.to_string_lossy()],
                )?;
                tx.execute(
                    "DELETE FROM usage_files WHERE scope=?1 AND path=?2",
                    params![scope, path.to_string_lossy()],
                )?;
            }
            for path in changed {
                if let Some(file) = files.get(path) {
                    let Ok(data) = serde_json::to_string(file) else {
                        continue;
                    };
                    tx.execute(
                        "INSERT OR REPLACE INTO usage_files VALUES (?1,?2,?3)",
                        params![scope, path.to_string_lossy(), data],
                    )?;
                    if file.problem.is_none() {
                        tx.execute(
                            "INSERT OR REPLACE INTO usage_file_stamps VALUES (?1,?2,?3,?4)",
                            params![
                                scope,
                                path.to_string_lossy(),
                                file.parser_version,
                                serde_json::to_string(&file.stamps).unwrap_or_default()
                            ],
                        )?;
                    } else {
                        tx.execute(
                            "DELETE FROM usage_file_stamps WHERE scope=?1 AND path=?2",
                            params![scope, path.to_string_lossy()],
                        )?;
                    }
                }
            }
            tx.execute(
                "INSERT OR REPLACE INTO usage_scans VALUES (?1,?2)",
                params![scope, at],
            )?;
            // 格式/数据根升级后旧数字索引可重建，限制保留量，避免长期积累多个完整副本。
            tx.execute("DELETE FROM usage_file_stamps WHERE scope IN (SELECT scope FROM usage_scans ORDER BY rowid DESC LIMIT -1 OFFSET 10)",[])?;
            tx.execute("DELETE FROM usage_files WHERE scope IN (SELECT scope FROM usage_scans ORDER BY rowid DESC LIMIT -1 OFFSET 10)",[])?;
            tx.execute("DELETE FROM usage_scans WHERE scope IN (SELECT scope FROM usage_scans ORDER BY rowid DESC LIMIT -1 OFFSET 10)",[])?;
            Ok(())
        };
        if save().is_ok() {
            let _ = tx.commit();
        }
    }
}

fn fingerprint(path: &Path, start: u64, len: u64) -> std::io::Result<String> {
    let mut f = open_jsonl(path)?;
    f.seek(SeekFrom::Start(start))?;
    let mut data = Vec::new();
    f.take(len).read_to_end(&mut data)?;
    Ok(format!("{:x}", Sha256::digest(data)))
}

pub(super) fn read_file(
    path: &Path,
    agent: &str,
    old: Option<&FileCache>,
    force: bool,
    cancel: &dyn Fn() -> bool,
) -> Result<(FileCache, u64), String> {
    let force = force || old.is_some_and(|old| old.parser_version < 3);
    let current = stamp(path).map_err(|e| e.to_string())?;
    if !force && old.is_some_and(|old| old.problem.is_none() && old.stamps == [current.clone()]) {
        return Ok((old.unwrap().clone(), 0));
    }
    let compressed = path.extension().is_some_and(|s| s == "zst");
    // 增长且身份及首尾锚点不变才走追加路径；等长重写、替换、截断都重新解析。
    // 手动强制刷新可重建索引，覆盖保留首尾锚点的非常规原地改写。
    let append = !force
        && !compressed
        && old.is_some_and(|old| {
            old.stamps.first().is_some_and(|s| {
                s.inode == current.inode
                    && s.device == current.device
                    && current.len > s.len
                    && old.offset <= current.len
            }) && fingerprint(path, 0, old.head_len).ok().as_ref() == Some(&old.head)
                && fingerprint(path, old.offset.saturating_sub(64), old.offset.min(64))
                    .ok()
                    .as_ref()
                    == Some(&old.tail)
        });
    let mut result = if append {
        old.unwrap().clone()
    } else {
        FileCache::default()
    };
    result.problem = None;
    if result.state.session.is_empty() {
        result.state.session = path.to_string_lossy().into_owned();
    }
    let mut file = open_jsonl(path).map_err(|e| e.to_string())?;
    let start = if compressed { 0 } else { result.offset };
    file.seek(SeekFrom::Start(start))
        .map_err(|e| e.to_string())?;
    let mut reader: Box<dyn BufRead> = if compressed {
        Box::new(BufReader::new(
            zstd::stream::read::Decoder::new(file).map_err(|e| e.to_string())?,
        ))
    } else {
        Box::new(BufReader::with_capacity(
            64 * 1024,
            file.take(current.len.saturating_sub(start)),
        ))
    };
    let mut position = start;
    let mut bytes_read = 0;
    let mut line = Vec::new();
    result.pending = false;
    result.parser_version = 3;
    loop {
        if cancel() {
            return Err("统计已取消".into());
        }
        let (bytes, terminated, oversized) =
            bounded_line(reader.as_mut(), &mut line).map_err(|e| e.to_string())?;
        if bytes == 0 {
            break;
        }
        bytes_read += bytes;
        if !terminated {
            result.pending = true;
            break;
        }
        let offset = position;
        position += bytes;
        result.offset = position;
        if oversized {
            // 原生 response_item 只是正文/工具记录，计量另在 token_count/usage_record；
            // 跳过超大正文不等于漏计，不应给用户伪报计量损坏。
            if agent == "codex" {
                let prefix = &line[..line.len().min(512)];
                if [
                    b"\"type\":\"response_item\"".as_slice(),
                    b"\"type\": \"response_item\"",
                ]
                .iter()
                .any(|tag| memchr::memmem::find(prefix, tag).is_some())
                {
                    continue;
                }
            }
            result.state.bad_lines += 1;
            continue;
        }
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        // Codex 工具正文占绝大部分字节；无需为与用量无关的巨大对象分配 JSON 树。
        if agent == "codex"
            && ![
                b"\"token_count\"".as_slice(),
                b"\"session_meta\"",
                b"\"turn_context\"",
                b"\"token_usage_record\"",
                b"\"compacted\"",
                b"\"task_started\"",
                b"\"task_complete\"",
                b"\"thread_settings_applied\"",
                b"\"item_completed\"",
                b"\"user_message\"",
                b"\"error\"",
            ]
            .iter()
            .any(|needle| memchr::memmem::find(&line, needle).is_some())
        {
            continue;
        }
        match serde_json::from_slice::<serde_json::Value>(&line) {
            Ok(v) => {
                let mut samples = parse::parse(agent, &v, offset, &mut result.state);
                super::request_parse::observe(agent, &v, offset, path, &mut result, &mut samples);
                result.samples.extend(samples);
            }
            Err(_) => result.state.bad_lines += 1,
        }
    }
    result.stamps = vec![current];
    if !compressed {
        result.head_len = result.offset.min(2048);
        result.head = fingerprint(path, 0, result.head_len).map_err(|e| e.to_string())?;
        result.tail = fingerprint(
            path,
            result.offset.saturating_sub(64),
            result.offset.min(64),
        )
        .map_err(|e| e.to_string())?;
    }
    Ok((result, bytes_read))
}

/// 巨大工具结果只消耗固定上限内存，超过上限仍读到换行以保持后续偏移正确。
fn bounded_line(
    reader: &mut dyn BufRead,
    line: &mut Vec<u8>,
) -> std::io::Result<(u64, bool, bool)> {
    line.clear();
    let mut bytes = 0u64;
    let mut large = false;
    loop {
        let buffer = reader.fill_buf()?;
        if buffer.is_empty() {
            return Ok((bytes, false, large));
        }
        let end = memchr::memchr(b'\n', buffer);
        let n = end.map_or(buffer.len(), |i| i + 1);
        if line.len() + n <= 16 * 1024 * 1024 && !large {
            line.extend_from_slice(&buffer[..n]);
        } else {
            large = true;
        }
        bytes += n as u64;
        reader.consume(n);
        if end.is_some() {
            return Ok((bytes, true, large));
        }
    }
}
