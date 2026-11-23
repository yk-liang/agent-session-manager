//! 删除前仅保存本次会被移除的日志；不把其他活跃会话的同步放进删除等待链路。
use super::{source, source_error, Store};
use crate::agents::{verify_within_root, TargetKind};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::PathBuf,
};

/// 保留正常扫描使用的路径键，避免 /var 与 /private/var 等别名产生另一份用量缓存。
fn snapshot(
    agent: &str,
    roots: &[PathBuf],
    targets: &[PathBuf],
) -> Result<BTreeMap<PathBuf, Vec<source::Stamp>>, String> {
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
        let path = root.join(canonical.strip_prefix(canonical_root).unwrap());
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

pub(super) fn preserve_at(
    agent: &str,
    roots: &[PathBuf],
    store: &mut Store,
    targets: &[PathBuf],
    delete: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    preserve_validated_at(agent, roots, store, targets, || Ok(()), delete)
}

pub(super) fn preserve_validated_at(
    agent: &str,
    roots: &[PathBuf],
    store: &mut Store,
    targets: &[PathBuf],
    validate: impl FnOnce() -> Result<(), String>,
    delete: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    preserve_with_sync(
        agent,
        roots,
        targets,
        |paths| {
            // 保留原 roots / scope，仅限制本次读取的路径；不能让局部同步清理其他会话缓存，
            // 也不能把它标记为整个 Agent 已完成首次同步。
            super::ingest::sync_paths(agent, roots, store, Some(paths), &|| false)?;
            validate()
        },
        delete,
    )
}

fn preserve_with_sync(
    agent: &str,
    roots: &[PathBuf],
    targets: &[PathBuf],
    sync: impl FnOnce(&BTreeSet<PathBuf>) -> Result<(), String>,
    delete: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    let before = snapshot(agent, roots, targets)?;
    let paths = before.keys().cloned().collect();
    sync(&paths).map_err(|e| format!("删除前未能保存累计用量，会话已保留：{e}"))?;
    if snapshot(agent, roots, targets)? != before {
        return Err("会话日志在保存用量期间发生变化，请重试删除".into());
    }
    delete()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::{cell::Cell, fs, io::Write, path::Path};

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
            let targets =
                crate::agents::claude_family::deletion_targets(&root, "project", "aaaa").unwrap();
            preserve_at(agent, &[temp.path().join("missing"), root], &mut store, &targets, || {
                fs::remove_file(&parent).unwrap();
                fs::remove_dir_all(&children).unwrap();
                Ok(())
            })
            .unwrap();
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
        let targets =
            crate::agents::claude_family::deletion_targets(&root, "project", "aaaa").unwrap();
        preserve_at("qoder", &[root.clone()], &mut store, &targets, || {
            fs::remove_file(&parent).unwrap();
            Ok(())
        })
        .unwrap();
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
    fn target_read_or_ledger_failure_never_calls_delete() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("projects");
        let path = root.join("project/aaaa.jsonl");
        record(&path, "aaaa", "first", 11);
        let bad = temp.path().join("broken-ledger.sqlite3");
        fs::write(&bad, "not a database").unwrap();
        let called = Cell::new(false);
        let mut store = Store::open_with_ledger(None, Some(&bad));
        assert!(preserve_at(
            "qoder",
            &[root.clone()],
            &mut store,
            &[path.clone()],
            || {
                called.set(true);
                Ok(())
            }
        )
        .is_err());
        assert!(!called.get());
        assert!(path.exists());
        assert_eq!(fs::read_to_string(&bad).unwrap(), "not a database");
        let child_dir = root.join("project/aaaa");
        fs::create_dir(&child_dir).unwrap();
        std::os::unix::fs::symlink(temp.path().join("missing"), child_dir.join("child.jsonl"))
            .unwrap();
        let mut store = Store::open(None);
        assert!(preserve_at(
            "qoder",
            &[root],
            &mut store,
            &[path.clone(), child_dir],
            || {
                called.set(true);
                Ok(())
            }
        )
        .is_err());
        assert!(!called.get());
        assert!(path.exists());
    }

    #[test]
    fn writes_replacements_and_new_children_during_preservation_abort_deletion() {
        for change in ["append", "replace", "child"] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().join("projects");
            let path = root.join("project/aaaa.jsonl");
            let children = root.join("project/aaaa");
            record(&path, "aaaa", "first", 11);
            fs::create_dir(&children).unwrap();
            let called = Cell::new(false);
            let mut store = Store::open(None);
            let result = preserve_with_sync(
                "qoder",
                &[root.clone()],
                &[path.clone(), children.clone()],
                |paths| {
                    super::super::ingest::sync_paths(
                        "qoder",
                        &[root.clone()],
                        &mut store,
                        Some(paths),
                        &|| false,
                    )?;
                    match change {
                        "append" => {
                            fs::OpenOptions::new()
                                .append(true)
                                .open(&path)
                                .unwrap()
                                .write_all(b"\n")
                                .unwrap();
                        }
                        "replace" => {
                            fs::rename(&path, temp.path().join("original")).unwrap();
                            record(&path, "aaaa", "replacement", 22);
                        }
                        _ => record(&children.join("new.jsonl"), "bbbb", "child", 33),
                    }
                    Ok(())
                },
                || {
                    called.set(true);
                    Ok(())
                },
            );
            assert!(result.unwrap_err().contains("发生变化"), "{change}");
            assert!(!called.get());
            assert!(path.exists());
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
        let targets =
            crate::agents::claude_family::deletion_targets(&root, "project", "aaaa").unwrap();
        preserve_at("qoder", &[root.clone()], &mut store, &targets, || Ok(())).unwrap();
        assert_eq!(total(&mut store), 0);
        let outside = temp.path().join("outside.jsonl");
        record(&outside, "cccc", "outside", 100);
        assert!(
            preserve_at("qoder", &[root.clone()], &mut store, &[outside], || panic!(
                "Must not delete outside roots"
            ))
            .is_err()
        );
        assert!(preserve_at("qoder", &[root], &mut store, &[], || panic!(
            "Must not accept empty targets"
        ))
        .is_err());
    }

    #[test]
    #[ignore = "Requires isolated SQLite backups and an explicitly selected real Qoder session"]
    fn real_qoder_deletion_preservation_benchmark() {
        use std::time::Instant;
        let backup = PathBuf::from(
            std::env::var("ASM_DELETE_PROBE_STORE")
                .expect("Missing isolated store backup directory"),
        );
        let project = std::env::var("ASM_DELETE_PROBE_PROJECT")
            .expect("Missing source project directory key");
        let session = std::env::var("ASM_DELETE_PROBE_SESSION").expect("Missing source session ID");
        let source = crate::agents::find_source("qoder").unwrap();
        let targets = source
            .deletion_targets(&project, &session)
            .unwrap()
            .expect("Source did not supply deletion targets");
        let roots = super::super::roots("qoder").unwrap();
        let selected = snapshot("qoder", &roots, &targets).unwrap();
        assert!(
            !selected.is_empty(),
            "Requested session has no usage source files"
        );
        let full_count = super::super::discover("qoder", &roots, &|| false)
            .unwrap()
            .len();
        for round in 0..3 {
            for targeted in if round % 2 == 0 {
                [false, true]
            } else {
                [true, false]
            } {
                // 只在已关闭的备份上分出独立副本；实际日志只读，绝不调用删除动作。
                let temp = tempfile::tempdir().unwrap();
                for name in ["cache.sqlite3", "ledger.sqlite3"] {
                    fs::copy(backup.join(name), temp.path().join(name)).unwrap();
                }
                let mut store = Store::open_with_ledger(
                    Some(&temp.path().join("cache.sqlite3")),
                    Some(&temp.path().join("ledger.sqlite3")),
                );
                let started = Instant::now();
                if targeted {
                    preserve_at("qoder", &roots, &mut store, &targets, || Ok(())).unwrap();
                } else {
                    super::super::preserve_before_delete_at("qoder", &roots, &mut store, || Ok(()))
                        .unwrap();
                }
                println!(
                    "{}",
                    json!({"round":round,"targeted":targeted,"elapsed_ms":started.elapsed().as_secs_f64()*1000.,
                    "selected_sources":selected.len(),"all_sources":full_count})
                );
            }
        }
        assert_eq!(
            snapshot("qoder", &roots, &targets).unwrap(),
            selected,
            "Selected source changed during benchmark"
        );
    }
}
