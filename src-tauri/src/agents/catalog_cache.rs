//! 仅缓存列表摘要，不保存对话正文；文件版本变化后重新解析。
mod disk;
use super::{Catalog, SessionMeta};
use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum Revision {
    Missing,
    File {
        len: u64,
        modified: Option<SystemTime>,
        device: u64,
        inode: u64,
        changed: (i64, i64),
        mode: u32,
    },
}

fn revisions(paths: &[PathBuf]) -> Option<Vec<Revision>> {
    paths
        .iter()
        .map(|path| match fs::metadata(path) {
            Ok(meta) if meta.is_file() => Some(Revision::File {
                len: meta.len(),
                modified: meta.modified().ok(),
                device: meta.dev(),
                inode: meta.ino(),
                changed: (meta.ctime(), meta.ctime_nsec()),
                mode: meta.mode(),
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some(Revision::Missing),
            // 特殊文件和不可读的元数据必须走原读取路径，保留其告警与非阻塞保护。
            _ => None,
        })
        .collect()
}

#[derive(Hash, PartialEq, Eq)]
struct Key {
    kind: String,
    paths: Vec<PathBuf>,
}
struct Entry<T> {
    revision: Vec<Revision>,
    value: T,
    used: u64,
}
struct State<T> {
    entries: HashMap<Key, Entry<T>>,
    clock: u64,
}

struct FileCache<T> {
    state: Mutex<State<T>>,
    capacity: usize,
}
impl<T: Clone> FileCache<T> {
    fn new(capacity: usize) -> Self {
        Self {
            state: Mutex::new(State {
                entries: HashMap::new(),
                clock: 0,
            }),
            capacity,
        }
    }

    fn load(
        &self,
        kind: &str,
        paths: &[PathBuf],
        load: impl FnOnce() -> Result<T, String>,
        cacheable: impl Fn(&T) -> bool,
    ) -> Result<T, String> {
        let key = Key {
            kind: kind.to_string(),
            paths: paths.to_vec(),
        };
        let before = revisions(paths);
        if let Some(revision) = &before {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.clock += 1;
            let used = state.clock;
            if let Some(entry) = state
                .entries
                .get_mut(&key)
                .filter(|e| &e.revision == revision)
            {
                entry.used = used;
                return Ok(entry.value.clone());
            }
        }
        // 解析和外部 I/O 不持有全局锁，慢会话不会挡住其他 Agent 的缓存命中。
        let value = load()?;
        if cacheable(&value) && before.is_some() && before == revisions(paths) {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if !state.entries.contains_key(&key) && state.entries.len() >= self.capacity {
                if let Some(oldest) =
                    state
                        .entries
                        .iter()
                        .min_by_key(|(_, e)| e.used)
                        .map(|(k, _)| Key {
                            kind: k.kind.clone(),
                            paths: k.paths.clone(),
                        })
                {
                    state.entries.remove(&oldest);
                }
            }
            state.clock += 1;
            let used = state.clock;
            state.entries.insert(
                key,
                Entry {
                    revision: before.unwrap(),
                    value: value.clone(),
                    used,
                },
            );
        }
        Ok(value)
    }
}

/// 单文件摘要，或 Codex 有效分页链的合并摘要。分页边界变化也会使整条会话失效。
pub(super) fn summary(
    kind: &str,
    paths: &[PathBuf],
    load: impl FnOnce() -> Result<(SessionMeta, Vec<String>), String>,
) -> Result<(SessionMeta, Vec<String>), String> {
    summary_cache().load(
        kind,
        paths,
        || {
            let before = revisions(paths);
            if let (Some(store), Some(rev)) = (disk::store(), before.as_ref()) {
                if let Some(meta) = store.get(kind, paths, rev) {
                    return Ok((meta, vec![]));
                }
            }
            let result = load()?;
            if result.1.is_empty() && before.is_some() && before == revisions(paths) {
                if let Some(store) = disk::store() {
                    store.put(kind, paths, &before.unwrap(), &result.0);
                }
            }
            Ok(result)
        },
        |(_, warnings)| warnings.is_empty(),
    )
}

fn summary_cache() -> &'static FileCache<(SessionMeta, Vec<String>)> {
    static CACHE: OnceLock<FileCache<(SessionMeta, Vec<String>)>> = OnceLock::new();
    CACHE.get_or_init(|| FileCache::new(4096))
}

/// 只查已验证的摘要，未命中时不读正文，供列表首屏使用。
pub(super) fn peek_summary(kind: &str, paths: &[PathBuf]) -> Option<SessionMeta> {
    let revision = revisions(paths)?;
    let key = Key {
        kind: kind.into(),
        paths: paths.to_vec(),
    };
    if let Some(entry) = summary_cache().state.lock().ok()?.entries.get(&key) {
        if entry.revision == revision {
            return Some(entry.value.0.clone());
        }
    }
    disk::store()?.get(kind, paths, &revision)
}

/// SQLite 是共享数据库：同时校验主库与 WAL，任何提交均触发重新查询。
pub(super) fn database(
    paths: &[PathBuf],
    include_empty: bool,
    load: impl FnOnce() -> Result<Catalog, String>,
) -> Result<Catalog, String> {
    static CACHE: OnceLock<FileCache<Catalog>> = OnceLock::new();
    CACHE.get_or_init(|| FileCache::new(16)).load(
        if include_empty {
            "sqlite-all"
        } else {
            "sqlite"
        },
        paths,
        load,
        |catalog| catalog.warnings.is_empty(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn unchanged_files_reuse_summaries_but_changes_and_replacements_reload() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("log");
        fs::write(&file, "one").unwrap();
        let cache = FileCache::new(4);
        let reads = Cell::new(0);
        let read = || {
            reads.set(reads.get() + 1);
            Ok(fs::read_to_string(&file).unwrap())
        };
        assert_eq!(
            cache.load("a", &[file.clone()], read, |_| true).unwrap(),
            "one"
        );
        cache.load("a", &[file.clone()], read, |_| true).unwrap();
        assert_eq!(reads.get(), 1);
        // 同长度改写与原位替换都不能靠长度相同复用旧值。
        fs::write(&file, "two").unwrap();
        assert_eq!(
            cache.load("a", &[file.clone()], read, |_| true).unwrap(),
            "two"
        );
        let replacement = temp.path().join("new");
        fs::write(&replacement, "six").unwrap();
        fs::rename(&replacement, &file).unwrap();
        assert_eq!(
            cache.load("a", &[file.clone()], read, |_| true).unwrap(),
            "six"
        );
        cache.load("b", &[file.clone()], read, |_| true).unwrap();
        assert_eq!(reads.get(), 4);
    }

    #[test]
    fn chain_members_and_missing_wal_participate_in_revision() {
        let temp = tempfile::tempdir().unwrap();
        let a = temp.path().join("a");
        let b = temp.path().join("b");
        fs::write(&a, "a").unwrap();
        let cache = FileCache::new(4);
        let reads = Cell::new(0);
        let load = || {
            reads.set(reads.get() + 1);
            Ok(reads.get())
        };
        let paths = [a.clone(), b.clone()];
        cache.load("chain", &paths, load, |_| true).unwrap();
        cache.load("chain", &paths, load, |_| true).unwrap();
        fs::write(&b, "wal").unwrap();
        cache.load("chain", &paths, load, |_| true).unwrap();
        fs::remove_file(&b).unwrap();
        cache.load("chain", &paths, load, |_| true).unwrap();
        cache.load("chain", &[a], load, |_| true).unwrap();
        assert_eq!(reads.get(), 4);
    }

    #[test]
    fn warnings_and_files_changed_during_read_are_not_cached() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("a");
        fs::write(&file, "one").unwrap();
        let cache = FileCache::new(4);
        let reads = Cell::new(0);
        for _ in 0..2 {
            cache
                .load(
                    "a",
                    &[file.clone()],
                    || {
                        reads.set(reads.get() + 1);
                        Ok("degraded")
                    },
                    |_| false,
                )
                .unwrap();
        }
        cache
            .load(
                "a",
                &[file.clone()],
                || {
                    fs::write(&file, "changed while reading").unwrap();
                    Ok("partial")
                },
                |_| true,
            )
            .unwrap();
        assert_eq!(
            cache
                .load("a", &[file.clone()], || Ok("complete"), |_| true)
                .unwrap(),
            "complete"
        );
        assert_eq!(
            cache
                .load("a", &[file], || panic!("should reuse"), |_| true)
                .unwrap(),
            "complete"
        );
        assert_eq!(reads.get(), 2);
    }
}
