//! 通过 App Server 永久删除 Codex 会话：一次扫描确认整批删除范围，在同一连接上逐个删除。
use super::app_server::{AppServer, Connection, ThreadAction, UpdateError};
use super::{escape_cwd, history, storage, thread, valid_thread_id};
use crate::agents::deletion::{Outcome, PreserveUsage};
use crate::agents::workspace_groups::normalized;
use crate::agents::{present, search::SessionRef, verify_within_root, TargetKind};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Debug, PartialEq, Eq)]
struct Target {
    session: SessionRef,
    files: Vec<PathBuf>,
    native_ids: HashSet<String>,
}

/// 会话树，根会话在前。
#[derive(Debug, PartialEq, Eq)]
struct Scope {
    ids: Vec<String>,
    targets: Vec<Target>,
}

impl Scope {
    /// 删除根会话会连带删除索引中的子会话，索引未覆盖的子会话再单独删除。
    fn order(&self) -> Vec<&Target> {
        let mut covered = self.targets[0].native_ids.clone();
        let mut order = vec![&self.targets[0]];
        for target in &self.targets[1..] {
            if !covered.contains(&target.session.session_id) {
                covered.extend(target.native_ids.iter().cloned());
                order.push(target);
            }
        }
        order
    }
}

pub(super) fn delete(
    home: &Path,
    sessions: &[SessionRef],
    preserve: &mut dyn PreserveUsage,
    outcome: &mut Outcome<'_>,
) -> Result<(), String> {
    let index = Index::scan(&home.join("sessions"))?;
    let mut scopes = Vec::new();
    for session in sessions {
        match index.scope(session) {
            Ok(scope) => scopes.push((session, scope)),
            Err(error) => outcome.fail(session, error),
        }
    }
    if scopes.is_empty() {
        return Ok(());
    }
    let diagnostic = super::history_database(home);
    let sqlite_home = diagnostic.parent().ok_or("缺少 Codex 数据库目录")?;
    let check = |server: &mut AppServer| match normalized(&server.sqlite_home(home)?) == normalized(sqlite_home) {
        true => Ok(()),
        false => Err(UpdateError::Operation("Codex 使用的数据库目录发生变化，无法确认删除前用量".into())),
    };
    let mut connection = Connection::open(home, &check, None)?;
    run(home, &scopes, preserve, outcome, |target, ready| connection.run(|server| {
        let path = server.read_thread(&target.session.project_dir, &target.session.session_id)?;
        if path.is_some_and(|path| !target.files.iter().any(|file| normalized(file) == normalized(Path::new(&path)))) {
            return Err(UpdateError::Operation("Codex App Server 返回的日志路径已变化，请刷新后重试".into()));
        }
        ready().map_err(UpdateError::Operation)?;
        server.mutate(ThreadAction::Delete, &target.session.session_id)
    }));
    Ok(())
}

/// delete 在 App Server 确认线程后、发出删除前调用 ready 做最后一次复核。
fn run(
    home: &Path,
    scopes: &[(&SessionRef, Scope)],
    preserve: &mut dyn PreserveUsage,
    outcome: &mut Outcome<'_>,
    mut delete: impl FnMut(&Target, &dyn Fn() -> Result<(), String>) -> Result<(), String>,
) {
    let root = home.join("sessions");
    for (session, scope) in scopes {
        let error = scope.order().into_iter()
            .try_for_each(|target| delete_target(home, scope, target, preserve, &mut delete))
            .err();
        match rollout_ids(&root) {
            Ok(remaining) => {
                let removed: Vec<_> = scope.targets.iter()
                    .filter(|target| !remaining.contains(&target.session.session_id))
                    .map(|target| target.session.clone())
                    .collect();
                let error = error.or_else(|| {
                    (removed.len() < scope.targets.len()).then(|| "部分 Codex 会话仍有日志，请刷新后重试".into())
                });
                outcome.settle(session, removed, error);
            }
            Err(scan) => outcome.fail(session, error.unwrap_or(scan)),
        }
    }
}

/// 删除会按当时的索引级联：删除前子树、用量记录和日志都必须与保存用量时一致。
fn delete_target(
    home: &Path,
    scope: &Scope,
    target: &Target,
    preserve: &mut dyn PreserveUsage,
    delete: &mut impl FnMut(&Target, &dyn Fn() -> Result<(), String>) -> Result<(), String>,
) -> Result<(), String> {
    let root = home.join("sessions");
    let Some(subtree) = current(&root, scope, target)? else {
        return Ok(());
    };
    let diagnostic = super::history_database(home);
    let had_diagnostic = present(&diagnostic)?;
    let files: Vec<_> = subtree.targets.iter().flat_map(|target| target.files.iter().cloned())
        .chain(had_diagnostic.then(|| diagnostic.clone()))
        .collect();
    let unchanged = preserve.save(&files)?;
    delete(&subtree.targets[0], &|| {
        if current(&root, scope, target)?.as_ref() != Some(&subtree) {
            return Err("Codex 子会话索引在删除前发生变化，请刷新后重试".into());
        }
        if super::history_database(home) != diagnostic || present(&diagnostic)? != had_diagnostic {
            return Err("Codex 用量记录已变化，请重新确认后删除".into());
        }
        unchanged()
    })
}

/// 目标会话现在的子树；已随先删除的父会话一起删除时为 None。
fn current(root: &Path, scope: &Scope, target: &Target) -> Result<Option<Scope>, String> {
    let index = Index::scan(root)?;
    if !index.groups.contains_key(&target.session.session_id) {
        return Ok(None);
    }
    let current = index.scope(&target.session)?;
    if current.ids.iter().any(|id| !scope.ids.contains(id))
        || !current.targets[0].native_ids.is_subset(&target.native_ids)
    {
        return Err("Codex 子会话索引在删除前发生变化，请刷新后重试".into());
    }
    Ok(Some(current))
}

/// 活跃与归档日志中仍存在的会话；清空的日志不算。
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

/// 一次扫描得到的日志头和子会话关系，可为同批多个会话确定删除范围。
struct Index {
    root: PathBuf,
    groups: BTreeMap<String, Vec<history::Page>>,
    edges: thread::SpawnEdges,
    relations: thread::Relations,
    unreadable: Vec<PathBuf>,
}

impl Index {
    fn scan(root: &Path) -> Result<Self, String> {
        let paths = storage::deletion_rollouts(root)?;
        let mut unreadable = Vec::new();
        let pages = history::scan_headers(&paths, &|| false, |path, _| unreadable.push(path.to_path_buf()))?;
        let groups = history::group_pages(pages);
        let edges = thread::spawn_edges(root.parent().ok_or("缺少 Codex 数据目录")?)?;
        let relations = thread::Relations::new(&groups, edges.clone());
        Ok(Self { root: root.to_owned(), groups, edges, relations, unreadable })
    }

    fn project_of(&self, id: &str) -> String {
        self.groups[id].last()
            .and_then(|page| page.header.cwd.as_deref())
            .map(escape_cwd)
            .unwrap_or_else(|| "-unknown".into())
    }

    fn scope(&self, session: &SessionRef) -> Result<Scope, String> {
        let id = session.session_id.as_str();
        if !valid_thread_id(id) {
            return Err("会话 ID 无效".into());
        }
        if !self.groups.contains_key(id) || self.project_of(id) != session.project_dir {
            return Err("Codex 会话身份或所属项目已变化，请刷新列表后重试".into());
        }
        let subtree = self.relations.subtree(id)?;
        let ids: HashSet<_> = subtree.iter().copied().collect();
        let pages: Vec<_> = self.groups.values().flatten().collect();
        history::validate_delete_copies(&pages, &ids)?;
        history::validate_delete_references(&pages, &ids)?;
        let by_path: HashMap<_, _> = pages.iter()
            .map(|page| (&page.path, page.header.session_id.as_str()))
            .collect();
        for path in &self.unreadable {
            if history::filename_id(path).is_some_and(|id| ids.contains(id))
                || by_path.get(path).is_some_and(|id| ids.contains(id))
            {
                return Err(format!("无法确认待删 Codex 日志的身份：{path:?}"));
            }
        }
        let archived = storage::archived_root(&self.root);
        let targets = subtree.iter().copied()
            .filter(|id| self.groups.contains_key(*id))
            .map(|id| {
                if !valid_thread_id(id) {
                    return Err(format!("Codex 会话 {id} 的 ID 无效，无法确认删除范围"));
                }
                let files = self.groups[id].iter()
                    .map(|page| {
                        let directory = if storage::is_archived(&self.root, &page.path) { &archived } else { &self.root };
                        verify_within_root(directory, &page.path, TargetKind::File)?;
                        Ok(page.path.clone())
                    })
                    .collect::<Result<_, String>>()?;
                Ok(Target {
                    session: SessionRef { session_id: id.into(), project_dir: self.project_of(id) },
                    files,
                    native_ids: thread::indexed_subtree(&self.edges, id),
                })
            })
            .collect::<Result<_, String>>()?;
        Ok(Scope { ids: subtree.into_iter().map(str::to_owned).collect(), targets })
    }
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
        let request = SessionRef { project_dir: escape_cwd(cwd), session_id: "main".into() };
        let scope = |root: &Path| Index::scan(root).and_then(|index| index.scope(&request));
        let targets = scope(&root).unwrap().targets;
        assert_eq!(
            targets
                .iter()
                .map(|target| &target.files[0])
                .collect::<Vec<_>>(),
            [&main, &child, &leaf]
        );
        assert!(orphan.exists());

        // 未登记在索引中的子会话逐个删除；已随父会话删除的不再请求。
        let orphan_request = SessionRef { session_id: "orphan".into(), ..request.clone() };
        let requests = [request.clone(), orphan_request.clone()];
        let settled = std::cell::RefCell::new(Vec::new());
        let progress = |count: usize| settled.borrow_mut().push(count);
        let mut outcome = Outcome::new(&requests, &progress);
        let mut saved = Saved::default();
        let mut deleted = Vec::new();
        let scopes = [(&request, scope(&root).unwrap()), (&orphan_request, Index::scan(&root).unwrap().scope(&orphan_request).unwrap())];
        run(home.path(), &scopes, &mut saved, &mut outcome, |target, ready| {
            ready()?;
            deleted.push(target.session.session_id.clone());
            match target.session.session_id.as_str() {
                "main" => [&main, &child].into_iter().for_each(|path| fs::remove_file(path).unwrap()),
                "orphan" => fs::remove_file(&orphan).unwrap(),
                _ => {}
            }
            Ok(())
        });
        assert_eq!(deleted, ["main", "leaf", "orphan"]);
        assert_eq!(saved.0[0], [main.clone(), child.clone(), leaf.clone()]);
        assert_eq!(*settled.borrow(), [1, 2]);
        let result = outcome.finish();
        assert_eq!(result.removed.iter().map(|session| session.session_id.as_str()).collect::<Vec<_>>(), ["main", "child", "orphan"]);
        assert_eq!(result.failures.len(), 1);
        assert_eq!(result.failures[0].error, "部分 Codex 会话仍有日志，请刷新后重试");
        fixture(&active, "main", None);
        fixture(&active, "child", Some("main"));
        fixture(&active, "orphan", Some("missing"));

        let state = rusqlite::Connection::open(home.path().join(thread::STATE_DB)).unwrap();
        state.execute_batch("CREATE TABLE thread_spawn_edges (child_thread_id TEXT PRIMARY KEY, parent_thread_id TEXT);
            INSERT INTO thread_spawn_edges VALUES ('child', 'main'), ('leaf', 'child');").unwrap();
        let indexed = scope(&root).unwrap();
        assert_eq!(
            indexed.targets[0].native_ids,
            HashSet::from(["main".into(), "child".into(), "leaf".into()])
        );
        let requests = [request.clone()];
        let mut outcome = Outcome::new(&requests, &progress);
        run(home.path(), &[(&request, indexed)], &mut Saved::default(), &mut outcome, |target, _| {
            assert_eq!(target.session.session_id, "main", "Indexed children are deleted with their root");
            fs::remove_file(&leaf).unwrap();
            Err("App Server 中断".into())
        });
        let result = outcome.finish();
        assert_eq!(result.failures[0].error, "App Server 中断");
        assert_eq!(result.removed.iter().map(|session| session.session_id.as_str()).collect::<Vec<_>>(), ["leaf"]);
        assert!(main.exists() && child.exists() && orphan.exists());

        fs::write(
            active.join("rollout-unrelated.jsonl"),
            "unreadable header\n",
        )
        .unwrap();
        assert!(scope(&root).is_ok());
        let conflicting = main.with_extension("jsonl.zst");
        let encoded = zstd::stream::encode_all(fs::read(&orphan).unwrap().as_slice(), 1).unwrap();
        fs::write(&conflicting, encoded).unwrap();
        assert!(scope(&root).unwrap_err().contains("压缩副本"));
    }

    #[derive(Default)]
    struct Saved(Vec<Vec<PathBuf>>);

    impl PreserveUsage for Saved {
        fn save(&mut self, targets: &[PathBuf]) -> Result<crate::agents::deletion::Unchanged, String> {
            self.0.push(targets.to_vec());
            Ok(Box::new(|| Ok(())))
        }
    }
}
