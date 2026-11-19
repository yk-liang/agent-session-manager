use crate::agents::{self, check_catalog_cancelled, AgentSource, Catalog, CatalogRefinement};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

type Scope = (&'static str, bool);

#[derive(Serialize)]
pub struct Snapshot {
    revision: u64,
    #[serde(flatten)]
    catalog: Catalog,
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
        include_empty: bool,
        generation: u64,
        revision: Option<u64>,
    ) -> Result<Option<Snapshot>, String> {
        self.cancel(generation);
        let cancelled = || self.generation.load(Ordering::SeqCst) != generation;
        let (catalog, refine) = source.prepare_catalog(include_empty, &cancelled)?;
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        check_catalog_cancelled(&cancelled)?;
        let scope = (source.id(), include_empty);
        state.refinement = refine.map(|refine| (generation, scope, refine));
        Ok(state.publish(source, scope, catalog, revision))
    }

    pub fn refine(
        &self,
        source: &dyn AgentSource,
        include_empty: bool,
        generation: u64,
        revision: Option<u64>,
    ) -> Result<Option<Snapshot>, String> {
        let cancelled = || self.generation.load(Ordering::SeqCst) != generation;
        let scope = (source.id(), include_empty);
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
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        check_catalog_cancelled(&cancelled)?;
        Ok(state.publish(source, scope, catalog, revision))
    }
}

impl State {
    fn publish(
        &mut self,
        source: &dyn AgentSource,
        scope: Scope,
        catalog: Catalog,
        known_revision: Option<u64>,
    ) -> Option<Snapshot> {
        let snapshot = match self.snapshots.entry(scope) {
            std::collections::hash_map::Entry::Occupied(entry)
                if entry.get().catalog == catalog =>
            {
                entry.into_mut()
            }
            entry => {
                self.revision += 1;
                entry
                    .insert_entry(Snapshot {
                        revision: self.revision,
                        catalog,
                    })
                    .into_mut()
            }
        };
        let revision = snapshot.revision;
        if self.registered.get(&scope.0) != Some(&revision) {
            agents::search::register(source, &snapshot.catalog);
            self.registered.insert(scope.0, revision);
        }
        (known_revision != Some(revision)).then(|| Snapshot {
            revision,
            catalog: snapshot.catalog.clone(),
        })
    }
}
