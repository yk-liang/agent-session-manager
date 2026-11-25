//! 删除前只保存会被移除的日志；不把其他活跃会话的同步放进删除等待链路。
use super::{source, source_error, Store};
use crate::agents::deletion::{PreserveUsage, Unchanged};
use crate::agents::{verify_within_root, TargetKind};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::PathBuf,
    sync::MutexGuard,
};

type Snapshot = BTreeMap<PathBuf, Vec<source::Stamp>>;

/// 保留正常扫描使用的路径键，避免 /var 与 /private/var 等别名产生另一份用量缓存。
fn snapshot(
    agent: &str,
    roots: &[PathBuf],
    targets: &[PathBuf],
) -> Result<Snapshot, String> {
    if targets.is_empty() {
        return Err("缺少待删会话文件，已中止删除".into());
    }
    let mut canonical_roots = Vec::new();
    for root in roots {
        match root.canonicalize() {
            Ok(canonical) => canonical_roots.push((root, canonical)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(source_error(root, error)),
        }
    }
    let mut paths = BTreeSet::new();
    for target in targets {
        let canonical = target
            .canonicalize()
            .map_err(|e| source_error(target, e))?;
        let (root, canonical_root) = canonical_roots
            .iter()
            .find(|(_, root)| canonical.starts_with(root))
            .ok_or("待删会话文件越出数据目录，已中止删除")?;
        let metadata = fs::symlink_metadata(target).map_err(|e| source_error(target, e))?;
        let is_dir = metadata.is_dir();
        verify_within_root(
            root,
            target,
            if is_dir {
                TargetKind::Dir
            } else {
                TargetKind::File
            },
        )?;
        let relative = canonical.strip_prefix(canonical_root).unwrap();
        let path = if relative.as_os_str().is_empty() {
            root.to_path_buf()
        } else {
            root.join(relative)
        };
        if is_dir {
            paths.extend(super::discover(agent, &[path], &|| false)?);
        } else {
            paths.insert(path);
        }
    }
    paths
        .into_iter()
        .map(|path| {
            let root = roots
                .iter()
                .find(|root| path.starts_with(root))
                .ok_or("待删用量文件越出数据目录，已中止删除")?;
            // 状态目录里的日志也逐个验证；不能跟随子目录中的链接读到范围外的用量。
            verify_within_root(root, &path, TargetKind::File)?;
            let stamp = source::Source::for_agent(agent).stamps(&path).map_err(|e| source_error(&path, e))?;
            Ok((path, stamp))
        })
        .collect()
}

/// 删除期间持有账本写锁，保存的用量先于删除持久化。
pub(super) struct Preservation {
    agent: String,
    roots: Vec<PathBuf>,
    store: MutexGuard<'static, Store>,
    _writer: super::ledger::SyncLock,
}

impl Preservation {
    pub(super) fn new(agent: &str, roots: Vec<PathBuf>, writer: super::ledger::SyncLock, store: MutexGuard<'static, Store>) -> Self {
        Self { agent: agent.to_owned(), roots, store, _writer: writer }
    }
}

impl PreserveUsage for Preservation {
    fn save(&mut self, targets: &[PathBuf]) -> Result<Unchanged, String> {
        let saved = save(&self.agent, &self.roots, &mut self.store, targets)?;
        let (agent, roots, targets) = (self.agent.clone(), self.roots.clone(), targets.to_vec());
        Ok(Box::new(move || unchanged(&agent, &roots, &targets, &saved)))
    }
}

/// 只读取删除目标涉及的日志；同步期间日志变化则不算保存成功。
pub(super) fn save(agent: &str, roots: &[PathBuf], store: &mut Store, targets: &[PathBuf]) -> Result<Snapshot, String> {
    save_with(agent, roots, targets, |paths| {
        // 保留原 roots / scope，仅限制本次读取的路径；不能让局部同步清理其他会话缓存，
        // 也不能把它标记为整个 Agent 已完成首次同步。
        super::ingest::sync_paths(agent, roots, store, Some(paths), &|| false).map(drop)
    })
}

fn save_with(
    agent: &str,
    roots: &[PathBuf],
    targets: &[PathBuf],
    sync: impl FnOnce(&BTreeSet<PathBuf>) -> Result<(), String>,
) -> Result<Snapshot, String> {
    let before = snapshot(agent, roots, targets)?;
    let input = source::Source::for_agent(agent);
    let paths = before.keys().filter(|path| {
        roots.iter().any(|root| input.changed_file(root, path).as_ref() == Some(*path))
    }).cloned().collect();
    sync(&paths).map_err(|e| format!("删除前未能保存累计用量，会话已保留：{e}"))?;
    if snapshot(agent, roots, targets)? != before {
        return Err("会话日志在保存用量期间发生变化，请重试删除".into());
    }
    Ok(before)
}

fn unchanged(agent: &str, roots: &[PathBuf], targets: &[PathBuf], saved: &Snapshot) -> Result<(), String> {
    match snapshot(agent, roots, targets)? == *saved {
        true => Ok(()),
        false => Err("会话日志在保存用量后发生变化，请重试删除".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::{fs, io::Write, path::Path};

    fn record(path: &Path, session: &str, request: &str, tokens: u64) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, format!("{}\n", json!({"type":"assistant","sessionId":session,
            "timestamp":"2026-11-17T00:00:00Z","message":{"id":request,"role":"assistant",
            "model":"fixture","stop_reason":"end_turn","usage":{"input_tokens":tokens,"output_tokens":0}}}))).unwrap();
    }

    fn total(store: &mut Store) -> u64 {
        super::super::summary::read_from(store, Default::default(), &|| false)
            .unwrap()
            .rows
            .iter()
            .map(|row| row.usage.total_tokens)
            .sum()
    }

    fn targets(root: &Path, id: &str) -> Vec<PathBuf> {
        crate::agents::claude_family::trash_plan(root, "project", id).unwrap().targets
    }

    #[test]
    fn deletion_snapshot_includes_codex_history_file_root_when_present() {
        let temp = tempfile::tempdir().unwrap();
        let sessions = temp.path().join("sessions");
        let rollout = sessions.join("rollout.jsonl");
        let history = temp.path().join("thread_history_1.sqlite");
        fs::create_dir(&sessions).unwrap();
        fs::write(&rollout, b"").unwrap();
        let roots = [sessions, history.clone()];

        let before = snapshot("codex", &roots, std::slice::from_ref(&rollout)).unwrap();
        assert_eq!(before.into_keys().collect::<Vec<_>>(), [rollout.clone()]);

        fs::write(&history, b"").unwrap();
        let after = snapshot("codex", &roots, &[rollout.clone(), history.clone()]).unwrap();
        assert_eq!(after.into_keys().collect::<Vec<_>>(), [rollout, history]);
    }

    #[test]
    fn targeted_deletion_preserves_parent_and_nested_usage_without_syncing_other_sessions() {
        for agent in ["qoder", "claude"] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().join("projects");
            let parent = root.join("project/aaaa.jsonl");
            let children = root.join("project/aaaa");
            record(&parent, "aaaa", "parent", 11);
            record(&children.join("subagents/child.jsonl"), "bbbb", "child", 22);
            record(&root.join("other/cccc.jsonl"), "cccc", "unrelated", 990);
            let ledger = temp.path().join("ledger.sqlite3");
            let mut store = Store::open_with_ledger(None, Some(&ledger));
            let targets = targets(&root, "aaaa");
            save(agent, &[temp.path().join("missing"), root], &mut store, &targets).unwrap();
            fs::remove_file(&parent).unwrap();
            fs::remove_dir_all(&children).unwrap();
            drop(store);
            let mut reopened = Store::open_with_ledger(None, Some(&ledger));
            assert_eq!(
                total(&mut reopened),
                33,
                "Deletion synchronized unrelated session usage"
            );
            assert_eq!(
                reopened
                    .ledger()
                    .unwrap()
                    .db
                    .query_row("SELECT count(*) FROM usage_sync_state", [], |row| row
                        .get::<_, i64>(0))
                    .unwrap(),
                0,
                "Target-only deletion must not mark a full agent sync complete"
            );
        }
    }

    #[test]
    fn warm_targeted_deletion_keeps_other_cache_and_saves_last_reply_once() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("projects");
        let parent = root.join("project/aaaa.jsonl");
        let other = root.join("other/bbbb.jsonl");
        record(&parent, "aaaa", "first", 11);
        record(&other, "bbbb", "unrelated-first", 77);
        let mut store = Store::open(None);
        super::super::ingest::sync_at("qoder", &[root.clone()], &mut store, &|| false).unwrap();
        let extra = temp.path().join("extra.jsonl");
        record(&extra, "aaaa", "last", 22);
        fs::OpenOptions::new()
            .append(true)
            .open(&parent)
            .unwrap()
            .write_all(&fs::read(&extra).unwrap())
            .unwrap();
        record(&extra, "bbbb", "unrelated-new", 990);
        fs::OpenOptions::new()
            .append(true)
            .open(&other)
            .unwrap()
            .write_all(&fs::read(&extra).unwrap())
            .unwrap();
        let scope = super::super::scope("qoder", &[root.clone()]);
        let old_other = serde_json::to_value(store.load_file(&scope, &other).unwrap()).unwrap();
        let targets = targets(&root, "aaaa");
        save("qoder", &[root.clone()], &mut store, &targets).unwrap();
        fs::remove_file(&parent).unwrap();
        assert_eq!(total(&mut store), 110);
        assert_eq!(
            serde_json::to_value(store.load_file(&scope, &other).unwrap()).unwrap(),
            old_other
        );
        super::super::ingest::sync_at("qoder", &[root], &mut store, &|| false).unwrap();
        assert_eq!(
            total(&mut store),
            1100,
            "Later full sync must retain deleted usage without duplication"
        );
    }

    #[test]
    fn unreadable_targets_or_ledger_fail_only_their_own_save() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("projects");
        let path = root.join("project/aaaa.jsonl");
        let other = root.join("project/bbbb.jsonl");
        record(&path, "aaaa", "first", 11);
        record(&other, "bbbb", "other", 22);
        let bad = temp.path().join("broken-ledger.sqlite3");
        fs::write(&bad, "not a database").unwrap();
        let mut broken = Store::open_with_ledger(None, Some(&bad));
        assert!(save("qoder", &[root.clone()], &mut broken, &[path.clone()]).is_err());
        assert_eq!(fs::read_to_string(&bad).unwrap(), "not a database");

        let child_dir = root.join("project/aaaa");
        fs::create_dir(&child_dir).unwrap();
        std::os::unix::fs::symlink(temp.path().join("missing"), child_dir.join("child.jsonl"))
            .unwrap();
        let mut store = Store::open(None);
        assert!(save("qoder", &[root.clone()], &mut store, &[path, child_dir]).is_err());
        save("qoder", &[root], &mut store, &[other]).unwrap();
        assert_eq!(total(&mut store), 22);

        let rollouts = temp.path().join("sessions");
        let corrupt = rollouts.join("rollout-broken.jsonl.zst");
        fs::create_dir(&rollouts).unwrap();
        fs::write(&corrupt, b"not a zstd stream").unwrap();
        let error = save("codex", &[rollouts], &mut store, &[corrupt]).unwrap_err();
        assert!(error.contains("rollout-broken.jsonl.zst"), "{error}");
    }

    #[test]
    fn writes_replacements_and_new_children_during_or_after_saving_abort_deletion() {
        let change = |change: &str, path: &Path, children: &Path, temp: &Path| match change {
            "append" => fs::OpenOptions::new().append(true).open(path).unwrap().write_all(b"\n").unwrap(),
            "replace" => {
                fs::rename(path, temp.join("original")).unwrap();
                record(path, "aaaa", "replacement", 22);
            }
            _ => record(&children.join("new.jsonl"), "bbbb", "child", 33),
        };
        for kind in ["append", "replace", "child"] {
            for after_saving in [false, true] {
                let temp = tempfile::tempdir().unwrap();
                let root = temp.path().join("projects");
                let path = root.join("project/aaaa.jsonl");
                let children = root.join("project/aaaa");
                record(&path, "aaaa", "first", 11);
                fs::create_dir(&children).unwrap();
                let targets = [path.clone(), children.clone()];
                let mut store = Store::open(None);
                let saved = save_with("qoder", &[root.clone()], &targets, |paths| {
                    super::super::ingest::sync_paths("qoder", &[root.clone()], &mut store, Some(paths), &|| false)?;
                    if !after_saving {
                        change(kind, &path, &children, temp.path());
                    }
                    Ok(())
                });
                let error = if after_saving {
                    change(kind, &path, &children, temp.path());
                    unchanged("qoder", &[root], &targets, &saved.unwrap()).unwrap_err()
                } else {
                    saved.unwrap_err()
                };
                assert!(error.contains("发生变化"), "{kind} {after_saving}");
            }
        }
    }

    #[test]
    fn state_only_target_does_not_scan_siblings_and_outside_targets_are_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("projects");
        let children = root.join("project/aaaa");
        fs::create_dir_all(&children).unwrap();
        record(&root.join("project/bbbb.jsonl"), "bbbb", "unrelated", 99);
        let mut store = Store::open(None);
        let targets = targets(&root, "aaaa");
        save("qoder", &[root.clone()], &mut store, &targets).unwrap();
        assert_eq!(total(&mut store), 0);
        let outside = temp.path().join("outside.jsonl");
        record(&outside, "cccc", "outside", 100);
        assert!(save("qoder", &[root.clone()], &mut store, &[outside]).is_err(), "Outside targets must never be deleted");
        assert!(save("qoder", &[root], &mut store, &[]).is_err(), "Empty targets must never be deleted");
    }
}
