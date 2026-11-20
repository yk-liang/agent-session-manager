use super::{decode, normalize_source, MigrationResult};
use crate::usage::{
    cache::{self, FileCache},
    events,
    ledger::{self, Ledger},
    roots,
};
use rusqlite::{types::ValueRef, Connection};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashSet},
    path::{Path, PathBuf},
};

const RELEASED_AGENTS: [&str; 5] = ["qoder", "claude", "codex", "opencode", "pi"];

fn has_history(db: &Connection, version: i64) -> rusqlite::Result<bool> {
    if version == 0 {
        let empty: bool = db.query_row(
            "SELECT NOT EXISTS(SELECT 1 FROM sqlite_schema
             WHERE type='table' AND name NOT GLOB 'sqlite_*')",
            [],
            |row| row.get(0),
        )?;
        if empty {
            return Ok(false);
        }
    }
    let filter = history_filter(version)?;
    db.query_row(
        &format!("SELECT EXISTS(SELECT 1 FROM usage_files {filter})"),
        [],
        |row| row.get(0),
    )
}

fn history_filter(version: i64) -> rusqlite::Result<&'static str> {
    match version {
        // v0.1.33–v0.1.37 的解析缓存没有持久化标记。
        0 => Ok(""),
        // v0.1.38–v0.1.39 仅把已确认入账的记录标为 durable。
        1 => Ok("WHERE durable=0"),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

// v0.1.33–v0.1.39 的缓存命名空间。元组里的版本与 FileCache.parser_version 独立。
fn released_scope(agent: &str, paths: &[&Path]) -> String {
    let namespace = match agent {
        "claude" => 3,
        "codex" => 4,
        _ => 1,
    };
    let encoded = serde_json::to_vec(&(2, namespace, agent, paths)).unwrap_or_default();
    format!("{:x}", Sha256::digest(encoded))
}

struct ReleasedScopes(BTreeMap<String, &'static str>);

impl ReleasedScopes {
    fn new(agent: &str, active_roots: &[PathBuf]) -> Self {
        let mut scopes = Self(BTreeMap::new());
        for id in RELEASED_AGENTS {
            if let Ok(configured) = roots(id) {
                scopes.add(id, &configured);
            }
            if id == agent {
                scopes.add(id, active_roots);
            }
        }
        // 发布版将整个 CLAUDE_CONFIG_DIR 解释成一个目录，默认只扫描 ~/.claude/projects。
        if let Some(dir) = std::env::var("CLAUDE_CONFIG_DIR").ok().filter(|dir| !dir.is_empty()) {
            let root = crate::agents::expand_tilde(&dir).join("projects");
            scopes.0.insert(released_scope("claude", &[&root]), "claude");
        }
        scopes
    }

    fn add(&mut self, agent: &'static str, paths: &[PathBuf]) {
        let paths: Vec<_> = paths
            .iter()
            .map(PathBuf::as_path)
            .filter(|path| !crate::usage::source::Source::for_agent(agent).is_file_root(path))
            .collect();
        if agent == "claude" {
            for path in paths {
                self.0.insert(released_scope(agent, &[path]), agent);
            }
        } else if !paths.is_empty() {
            self.0.insert(released_scope(agent, &paths), agent);
        }
    }

    fn owner(&mut self, key: &str, path: &Path) -> Option<&'static str> {
        if let Some(&agent) = self.0.get(key) {
            return Some(agent);
        }
        // 配置和目录可能已被移走。用历史路径重建候选根，仅接受原 scope 的精确匹配。
        for root in path.ancestors().skip(1).filter(|root| !root.as_os_str().is_empty()) {
            let folder = root.file_name().and_then(|name| name.to_str());
            let agents: &[&str] = match folder {
                Some("projects") => &["claude", "qoder", "opencode"],
                Some("sessions") => &["pi", "opencode"],
                _ => &["opencode"],
            };
            if let Some(agent) = agents.iter().copied().find(|agent| released_scope(agent, &[root]) == key) {
                self.0.insert(key.into(), agent);
                return Some(agent);
            }
            if matches!(folder, Some("sessions" | "archived_sessions")) {
                let parent = root.parent()?;
                if released_scope("codex", &[&parent.join("sessions"), &parent.join("archived_sessions")]) == key {
                    self.0.insert(key.into(), "codex");
                    return Some("codex");
                }
            }
        }
        None
    }
}

fn recorded(ledger: &Connection, agent: &str, path: &Path, cached: &FileCache) -> MigrationResult<bool> {
    // 比较之前已经持久化的样本。会话键也能找到移动或归档后的来源。
    let mut statement = ledger.prepare_cached(
        "SELECT s.data FROM
           (SELECT DISTINCT source FROM usage_source_keys WHERE key IN (?2,?3)) k
         CROSS JOIN usage_ledger_sources s ON s.id=k.source WHERE s.agent=?1",
    )?;
    let mut rows = statement.query([
        agent,
        &format!("file:{}", path.display()),
        &format!("session:{}", cached.state.session),
    ])?;
    let expected = measurements(agent, cached)?;
    while let Some(row) = rows.next()? {
        let existing: FileCache = decode(row.get_ref(0)?)?;
        if ledger::sources::same_source(agent, &existing, cached)
            && expected.is_subset(&measurements(agent, &existing)?)
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn measurements(agent: &str, file: &FileCache) -> MigrationResult<HashSet<Vec<u8>>> {
    let observations = if agent == "codex" {
        ledger::sources::observation_ids(&file.samples)
    } else {
        file.samples.iter().map(|sample| sample.key.clone()).collect()
    };
    observations.iter().map(String::as_str).zip(&file.samples)
        .chain(file.failures.iter().map(|sample| (sample.key.as_str(), sample)))
        .map(|(id, sample)| {
            let mut sample = sample.clone();
            sample.request.source = None;
            Ok(serde_json::to_vec(&(id, sample))?)
        })
        .collect()
}

fn read_source(raw: ValueRef<'_>) -> MigrationResult<Value> {
    match raw {
        ValueRef::Text(bytes) => Ok(serde_json::from_slice(bytes)?),
        raw => Ok(decode(raw)?),
    }
}

pub(in crate::usage) fn upgrade(
    cache: &mut Connection,
    ledger: Option<&mut Ledger>,
    agent: &str,
    roots: &[PathBuf],
    cancel: &dyn Fn() -> bool,
) -> MigrationResult {
    let tx = cache.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let version: i64 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version == cache::VERSION {
        return Ok(());
    }
    if !matches!(version, 0 | 1) {
        return Err(format!("不支持的解析缓存结构版本 {version}").into());
    }
    if has_history(&tx, version)? {
        let ledger = ledger.ok_or("缺少持久用量账本路径")?;
        restore(&tx, version, ledger, agent, roots, cancel)?;
    }
    if cancel() {
        return Err("采集已停止".into());
    }
    // 账本和索引已提交，此后才可清理旧缓存。
    tx.execute_batch("DROP TABLE IF EXISTS usage_files;
        DROP TABLE IF EXISTS usage_file_stamps;
        DROP TABLE IF EXISTS usage_scans;")?;
    tx.execute_batch(cache::SCHEMA)?;
    tx.pragma_update(None, "user_version", cache::VERSION)?;
    tx.commit()?;
    Ok(())
}

fn restore(
    cache: &Connection,
    version: i64,
    ledger: &mut Ledger,
    agent: &str,
    active_roots: &[PathBuf],
    cancel: &dyn Fn() -> bool,
) -> MigrationResult {
    let mut scopes = ReleasedScopes::new(agent, active_roots);
    let filter = history_filter(version)?;
    let mut imports = BTreeMap::<&str, BTreeMap<PathBuf, FileCache>>::new();
    let mut errors = 0;
    let mut first_error = None;
    let mut statement = cache.prepare(&format!(
        "SELECT scope,path,data FROM usage_files {filter} ORDER BY scope,path"
    ))?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        if cancel() {
            return Err("采集已停止".into());
        }
        let path = row.get::<_, String>(1).map(PathBuf::from);
        let result = (|| -> MigrationResult {
            let path = path.as_ref().map_err(|error| error.to_string())?;
            let key: String = row.get(0)?;
            let mut content = read_source(row.get_ref(2)?)?;
            let owner = scopes.owner(&key, path);
            let mut file = normalize_source(owner.unwrap_or_default(), &mut content)?;
            if file.samples.is_empty() && file.failures.is_empty() {
                return Ok(());
            }
            let owner = owner.ok_or("无法确认旧用量来源所属的 Agent")?;
            let history = imports.entry(owner).or_default();
            if !recorded(&ledger.db, owner, path, &file)? {
                let key = ledger::sources::source_path(owner, path, &file);
                file.origin_path = Some(path.clone());
                ledger::sources::clear_source_coverage(&mut file);
                if let Some(old) = history.remove(&key) {
                    file = ledger::sources::merge_restored(owner, &old, &file);
                }
                history.insert(key, file);
            }
            Ok(())
        })();
        if let Err(error) = result {
            errors += 1;
            first_error.get_or_insert_with(|| match path {
                Ok(path) => format!("{}：{error}", path.display()),
                Err(_) => format!("缺少有效的来源路径：{error}"),
            });
        }
    }
    for (owner, history) in imports {
        if cancel() {
            return Err("采集已停止".into());
        }
        if !history.is_empty() {
            ledger.restore(owner, &history, chrono::Utc::now().timestamp_millis())?;
        }
        events::refresh(&mut ledger.db, owner, cancel)?;
    }
    match first_error {
        Some(error) => Err(format!("{errors} 条旧缓存尚未恢复，首条错误为 {error}").into()),
        None => Ok(()),
    }
}
