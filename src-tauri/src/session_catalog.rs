use crate::agents::{self, check_catalog_cancelled, AgentSource, Catalog, CatalogFilter, CatalogRefinement};
use serde::Serialize;
use std::collections::{BTreeSet, HashMap};
use std::io::ErrorKind;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

type Scope = (&'static str, CatalogFilter);

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    revision: u64,
    #[serde(flatten)]
    catalog: Catalog,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    missing_project_paths: Vec<String>,
}

#[derive(Default)]
struct State {
    revision: u64,
    snapshots: HashMap<Scope, Snapshot>,
    registered: HashMap<&'static str, u64>,
    refinement: Option<(u64, Scope, CatalogRefinement)>,
}

#[derive(Default)]
pub struct SessionCatalogs {
    generation: AtomicU64,
    state: Mutex<State>,
}

impl SessionCatalogs {
    pub fn cancel(&self, generation: u64) {
        self.generation.fetch_max(generation, Ordering::SeqCst);
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state
            .refinement
            .as_ref()
            .is_some_and(|(pending, _, _)| *pending < generation)
        {
            state.refinement = None;
        }
    }

    pub fn load(
        &self,
        source: &dyn AgentSource,
        filter: CatalogFilter,
        generation: u64,
        revision: Option<u64>,
    ) -> Result<Option<Snapshot>, String> {
        self.cancel(generation);
        let cancelled = || self.generation.load(Ordering::SeqCst) != generation;
        let (catalog, refine) = source.prepare_catalog(filter, &cancelled)?;
        let missing_paths = missing_project_paths(&catalog, &cancelled)?;
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        check_catalog_cancelled(&cancelled)?;
        let scope = (source.id(), filter);
        state.refinement = refine.map(|refine| (generation, scope, refine));
        Ok(state.publish(source, scope, catalog, missing_paths, revision))
    }

    pub fn refine(
        &self,
        source: &dyn AgentSource,
        filter: CatalogFilter,
        generation: u64,
        revision: Option<u64>,
    ) -> Result<Option<Snapshot>, String> {
        let cancelled = || self.generation.load(Ordering::SeqCst) != generation;
        let scope = (source.id(), filter);
        let refine = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            check_catalog_cancelled(&cancelled)?;
            let Some((_, _, refine)) = state
                .refinement
                .take_if(|(pending, target, _)| *pending == generation && *target == scope)
            else {
                return Ok(None);
            };
            refine
        };
        let catalog = refine(&cancelled)?;
        let missing_paths = missing_project_paths(&catalog, &cancelled)?;
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        check_catalog_cancelled(&cancelled)?;
        Ok(state.publish(source, scope, catalog, missing_paths, revision))
    }
}

fn missing_project_paths(
    catalog: &Catalog,
    cancelled: &dyn Fn() -> bool,
) -> Result<Vec<String>, String> {
    // 目录可能在会话日志无变化时被移动或恢复。
    let paths: BTreeSet<_> = catalog
        .projects
        .iter()
        .map(|project| project.display_path.as_str())
        .chain(catalog.session_group_hints.iter().filter_map(|hint| hint.root.as_deref()))
        .chain(catalog.sessions.iter().filter_map(|session| session.codex.as_ref()?.project_owner.as_ref()?.cwd.as_deref()))
        .collect();
    let mut missing = Vec::new();
    for path in paths {
        check_catalog_cancelled(cancelled)?;
        let directory = Path::new(path);
        if !directory.is_absolute() {
            continue;
        }
        let is_missing = match directory.metadata() {
            Ok(metadata) => !metadata.is_dir(),
            Err(error) => matches!(error.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory),
        };
        if is_missing {
            missing.push(path.to_owned());
        }
    }
    Ok(missing)
}

impl State {
    fn publish(
        &mut self,
        source: &dyn AgentSource,
        scope: Scope,
        catalog: Catalog,
        missing_project_paths: Vec<String>,
        known_revision: Option<u64>,
    ) -> Option<Snapshot> {
        let snapshot = match self.snapshots.entry(scope) {
            std::collections::hash_map::Entry::Occupied(entry)
                if entry.get().catalog == catalog && entry.get().missing_project_paths == missing_project_paths =>
            {
                entry.into_mut()
            }
            entry => {
                self.revision += 1;
                entry
                    .insert_entry(Snapshot {
                        revision: self.revision,
                        catalog,
                        missing_project_paths,
                    })
                    .into_mut()
            }
        };
        let revision = snapshot.revision;
        if self.registered.get(&scope.0) != Some(&revision) {
            agents::search::register(source, &snapshot.catalog);
            self.registered.insert(scope.0, revision);
        }
        (known_revision != Some(revision)).then(|| snapshot.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::{Project, SessionGroupHint};
    use std::fs;

    #[test]
    fn detects_missing_groups_and_rechecks_symlink_targets() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        let root = temp.path().join("group");
        let alias = temp.path().join("alias");
        std::os::unix::fs::symlink(&project, &alias).unwrap();
        fs::create_dir(&root).unwrap();
        let mut catalog = agents::assemble_catalog(Vec::new());
        for path in [project.as_path(), alias.as_path(), Path::new("unknown-project")] {
            catalog.projects.push(Project {
                dir_name: path.display().to_string(),
                display_path: path.display().to_string(),
                session_count: 1,
                last_activity: None,
            });
        }
        catalog.session_group_hints.push(SessionGroupHint {
            project_dir: "source".into(),
            session_id: "session".into(),
            root: Some(root.display().to_string()),
        });
        assert_eq!(
            missing_project_paths(&catalog, &|| false).unwrap(),
            [alias.display().to_string(), project.display().to_string()]
        );

        fs::create_dir(&project).unwrap();
        fs::remove_dir(&root).unwrap();
        fs::write(&root, "a file is no longer a project directory").unwrap();
        assert_eq!(
            missing_project_paths(&catalog, &|| false).unwrap(),
            [root.display().to_string()]
        );
    }
}
