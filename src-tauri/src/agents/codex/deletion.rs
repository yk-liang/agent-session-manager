use super::{app_server, escape_cwd, history, sqlite_home, storage, thread, valid_thread_id};
use crate::agents::{present, search::SessionRef, verify_within_root, SessionDeletion, TargetKind};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Debug, PartialEq, Eq)]
struct Target {
    session: SessionRef,
    files: Vec<PathBuf>,
    native_ids: HashSet<String>,
}

#[derive(Debug, PartialEq, Eq)]
struct Scope {
    ids: Vec<String>,
    targets: Vec<Target>,
}

type PreserveUsage<'a> = dyn Fn(
        &[PathBuf],
        &dyn Fn() -> Result<(), String>,
        &mut app_server::Mutation<'_>,
    ) -> Result<(), String>
    + Sync
    + 'a;

pub(crate) struct Plan {
    root: PathBuf,
    scope: Scope,
}

impl Plan {
    pub fn for_session(home: &Path, project: &str, id: &str) -> Result<Self, String> {
        let root = home.join("sessions");
        let scope = plan(&root, project, id)?;
        Ok(Self { root, scope })
    }

    pub fn run(
        self,
        diagnostic: &Path,
        preserve: &PreserveUsage<'_>,
    ) -> Result<SessionDeletion, String> {
        self.run_using(diagnostic, preserve, app_server::delete_verified)
    }

    pub(super) fn run_using(
        self,
        diagnostic: &Path,
        preserve: &PreserveUsage<'_>,
        mut delete: impl FnMut(&Path, &str, &str, &app_server::Preserve<'_>) -> Result<(), String>,
    ) -> Result<SessionDeletion, String> {
        let root = &self.scope.targets[0].session;
        if plan(&self.root, &root.project_dir, &root.session_id)? != self.scope {
            return Err("Codex 会话或子会话在保存用量期间发生变化，请重新确认后删除".into());
        }
        let home = self.root.parent().unwrap();
        let expected_sqlite_home = diagnostic.parent().ok_or("缺少 Codex 数据库目录")?;
        let mut removed = HashSet::new();
        let result = (|| -> Result<(), String> {
            let targets = &self.scope.targets;
            let mut covered = targets[0].native_ids.clone();
            let mut order = vec![&targets[0]];
            for target in targets.iter().skip(1) {
                if !covered.contains(&target.session.session_id) {
                    covered.extend(target.native_ids.iter().cloned());
                    order.push(target);
                }
            }
            for target in order {
                let id = &target.session.session_id;
                if removed.contains(id) {
                    continue;
                }
                let expected: HashSet<_> =
                    target.native_ids.difference(&removed).cloned().collect();
                delete(
                    home,
                    &target.session.project_dir,
                    id,
                    &|server_path, rollout_path, mutate| {
                        let server_home = match server_path {
                            Some(uri) if uri.starts_with("file:") => {
                                crate::agents::codex::app_server::file_url_path(uri)
                                    .ok_or("无法识别 Codex 返回的数据库目录")?
                            }
                            Some(path) => home.join(crate::agents::expand_tilde(path)),
                            None => sqlite_home(home),
                        };
                        if crate::agents::workspace_groups::normalized(&server_home)
                            != crate::agents::workspace_groups::normalized(expected_sqlite_home)
                        {
                            return Err("Codex 使用的数据库目录发生变化，无法确认删除前用量".into());
                        }
                        let current_scope = plan(&self.root, &target.session.project_dir, id)?;
                        if !current_scope.targets[0].native_ids.is_subset(&expected)
                            || current_scope
                                .targets
                                .iter()
                                .any(|target| !self.scope.ids.contains(&target.session.session_id))
                        {
                            return Err("Codex 子会话索引在删除前发生变化，请刷新后重试".into());
                        }
                        if let Some(path) = rollout_path {
                            let path = crate::agents::workspace_groups::normalized(Path::new(path));
                            if !current_scope.targets[0].files.iter().any(|file| {
                                crate::agents::workspace_groups::normalized(file) == path
                            }) {
                                return Err(
                                    "Codex App Server 返回的日志路径已变化，请刷新后重试".into()
                                );
                            }
                        }
                        let had_diagnostic = present(diagnostic)?;
                        let files: Vec<_> = current_scope
                            .targets
                            .iter()
                            .flat_map(|target| target.files.iter().cloned())
                            .chain(had_diagnostic.then(|| diagnostic.to_path_buf()))
                            .collect();
                        preserve(
                            &files,
                            &|| {
                                if plan(&self.root, &target.session.project_dir, id)?
                                    != current_scope
                                {
                                    return Err(
                                        "Codex 子会话索引在删除前发生变化，请刷新后重试".into()
                                    );
                                }
                                if present(diagnostic)? != had_diagnostic {
                                    return Err("Codex 用量记录已变化，请重新确认后删除".into());
                                }
                                Ok(())
                            },
                            mutate,
                        )
                    },
                )?;
                let remaining = rollout_ids(&self.root)?;
                removed.extend(expected.into_iter().filter(|id| !remaining.contains(id)));
                if !removed.contains(id) {
                    return Err(format!("Codex 会话 {id} 仍有日志，请刷新后重试"));
                }
            }
            Ok(())
        })();
        let remaining = rollout_ids(&self.root);
        let expected_count = self.scope.targets.len();
        let (removed, error) = match remaining {
            Ok(remaining) => {
                let removed: Vec<_> = self
                    .scope
                    .targets
                    .into_iter()
                    .filter(|target| !remaining.contains(&target.session.session_id))
                    .map(|target| target.session)
                    .collect();
                let error = result.err().or_else(|| {
                    (removed.len() < expected_count)
                        .then(|| "部分 Codex 会话仍有日志，请刷新后重试".into())
                });
                (removed, error)
            }
            Err(error) => (Vec::new(), Some(error)),
        };
        Ok(SessionDeletion { removed, error })
    }
}

fn rollout_ids(root: &Path) -> Result<HashSet<String>, String> {
    storage::deletion_rollouts(root)?
        .iter()
        .filter(|path| !path.metadata().is_ok_and(|meta| meta.len() == 0))
        .try_fold(HashSet::new(), |mut ids, path| {
            let (header, _) = history::read_header(path)?;
            if let Some(id) = header
                .map(|header| header.session_id)
                .or_else(|| history::filename_id(path).map(str::to_owned))
            {
                ids.insert(id);
            }
            Ok(ids)
        })
}

fn plan(root: &Path, project: &str, id: &str) -> Result<Scope, String> {
    if !valid_thread_id(id) {
        return Err("会话 ID 无效".into());
    }
    let paths = storage::deletion_rollouts(root)?;
    let mut warnings = Vec::new();
    let pages = history::scan_headers(&paths, &|| false, |path, _| {
        warnings.push(path.to_path_buf())
    })?;
    let groups = history::group_pages(pages);
    let project_of = |id: &str| {
        groups[id]
            .last()
            .and_then(|page| page.header.cwd.as_deref())
            .map(escape_cwd)
            .unwrap_or_else(|| "-unknown".into())
    };
    let edges = thread::spawn_edges(root.parent().ok_or("缺少 Codex 数据目录")?)?;
    let relations = thread::Relations::new(&groups, edges.clone());
    if !groups.contains_key(id) || project_of(id) != project {
        return Err("Codex 会话身份或所属项目已变化，请刷新列表后重试".into());
    }
    let subtree = relations.subtree(id)?;
    let ids: HashSet<_> = subtree.iter().copied().collect();
    let pages: Vec<_> = groups.values().flatten().collect();
    history::validate_delete_copies(&pages, &ids)?;
    history::validate_delete_references(&pages, &ids)?;
    let by_path: HashMap<_, _> = pages
        .iter()
        .map(|page| (&page.path, page.header.session_id.as_str()))
        .collect();
    for path in &warnings {
        if history::filename_id(path).is_some_and(|id| ids.contains(id))
            || by_path.get(path).is_some_and(|id| ids.contains(id))
        {
            return Err(format!("无法确认待删 Codex 日志的身份：{path:?}"));
        }
    }
    let archived = storage::archived_root(root);
    let targets = subtree
        .iter()
        .copied()
        .filter(|id| groups.contains_key(*id))
        .map(|id| {
            if !valid_thread_id(id) {
                return Err(format!("Codex 会话 {id} 的 ID 无效，无法确认删除范围"));
            }
            let files = groups[id]
                .iter()
                .map(|page| {
                    let directory = if storage::is_archived(root, &page.path) {
                        archived.as_path()
                    } else {
                        root
                    };
                    verify_within_root(directory, &page.path, TargetKind::File)?;
                    Ok(page.path.clone())
                })
                .collect::<Result<_, String>>()?;
            Ok(Target {
                session: SessionRef {
                    session_id: id.into(),
                    project_dir: project_of(id),
                },
                files,
                native_ids: thread::indexed_subtree(&edges, id),
            })
        })
        .collect::<Result<_, String>>()?;
    Ok(Scope {
        ids: subtree.into_iter().map(str::to_owned).collect(),
        targets,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;

    #[test]
    fn deletion_scope_includes_nested_archived_children_and_excludes_orphans() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("sessions");
        let active = root.join("2099/01/01");
        let archived = storage::archived_root(&root);
        let cwd = home.path().to_str().unwrap();
        let fixture =
            |directory: &Path, id: &str, parent: Option<&str>| {
                fs::create_dir_all(directory).unwrap();
                let path = directory.join(format!("rollout-2099-01-01T00-00-00-{id}.jsonl"));
                fs::write(&path, json!({"type":"session_meta","payload":{
                "id":id, "cwd":cwd, "source":if parent.is_some() { "subagent" } else { "cli" },
                "parent_thread_id":parent
            }}).to_string()).unwrap();
                path
            };
        let main = fixture(&active, "main", None);
        let child = fixture(&active, "child", Some("main"));
        let leaf = fixture(&archived, "leaf", Some("child"));
        let orphan = fixture(&active, "orphan", Some("missing"));
        let project = escape_cwd(cwd);
        let targets = plan(&root, &project, "main").unwrap().targets;
        assert_eq!(
            targets
                .iter()
                .map(|target| &target.files[0])
                .collect::<Vec<_>>(),
            [&main, &child, &leaf]
        );
        assert!(orphan.exists());
        let state = rusqlite::Connection::open(home.path().join(thread::STATE_DB)).unwrap();
        state.execute_batch("CREATE TABLE thread_spawn_edges (child_thread_id TEXT PRIMARY KEY, parent_thread_id TEXT);
            INSERT INTO thread_spawn_edges VALUES ('child', 'main'), ('leaf', 'child');").unwrap();
        let targets = plan(&root, &project, "main").unwrap().targets;
        assert_eq!(
            targets[0].native_ids,
            HashSet::from(["main".into(), "child".into(), "leaf".into()])
        );
        fs::write(
            active.join("rollout-unrelated.jsonl"),
            "unreadable header\n",
        )
        .unwrap();
        assert!(plan(&root, &project, "main").is_ok());
        let conflicting = main.with_extension("jsonl.zst");
        let encoded = zstd::stream::encode_all(fs::read(&orphan).unwrap().as_slice(), 1).unwrap();
        fs::write(&conflicting, encoded).unwrap();
        assert!(plan(&root, &project, "main")
            .unwrap_err()
            .contains("压缩副本"));
        fs::remove_file(conflicting).unwrap();
        let outcome = Plan::for_session(home.path(), &project, "main")
            .unwrap()
            .run_using(
                &home.path().join("thread_history_1.sqlite"),
                &|_, validate, mutate| {
                    validate()?;
                    mutate()
                },
                |_, _, id, preserve| {
                    assert_eq!(id, "main");
                    preserve(None, main.to_str(), &mut || {
                        fs::remove_file(&leaf).unwrap();
                        Err("App Server 中断".into())
                    })
                },
            )
            .unwrap();
        assert_eq!(outcome.error.as_deref(), Some("App Server 中断"));
        assert_eq!(
            outcome
                .removed
                .iter()
                .map(|session| session.session_id.as_str())
                .collect::<Vec<_>>(),
            ["leaf"]
        );
        assert!(main.exists() && child.exists() && orphan.exists());
    }
}
