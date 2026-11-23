use super::{history, load_thread_names_in, projects, storage, thread, to_meta, ReadMode};
use crate::agents::{
    self, catalog_cache, check_catalog_cancelled, Catalog, CatalogFilter, SessionMeta,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

const SUMMARY_CACHE_KIND: &str = "codex-history-v7";

pub(super) struct Scan {
    root: PathBuf,
    filter: CatalogFilter,
    paths: Vec<PathBuf>,
    titles: HashMap<String, String>,
    warnings: Vec<String>,
}

impl Scan {
    pub(super) fn new(
        root: &Path,
        filter: CatalogFilter,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Self, String> {
        check_catalog_cancelled(cancelled)?;
        let mut warnings = Vec::new();
        let paths = storage::rollouts(root, true, &mut warnings, cancelled)?;
        check_catalog_cancelled(cancelled)?;
        let titles = root
            .parent()
            .map(|home| load_thread_names_in(home, &mut warnings))
            .unwrap_or_default();
        Ok(Self {
            root: root.to_path_buf(),
            filter,
            paths,
            titles,
            warnings,
        })
    }

    pub(super) fn read(
        &self,
        quick: bool,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Catalog, String> {
        let mut warnings = self.warnings.clone();
        let pages = history::read_headers(&self.paths, &mut warnings, cancelled)?;
        let groups = history::group_pages(pages);
        let relations = thread::Relations::new(&groups, thread::spawn_edges_warned(&self.root, &mut warnings));
        let mut items = Vec::with_capacity(groups.len());
        for (id, pages) in &groups {
            let archived = storage::is_archived(&self.root, &pages.last().unwrap().path);
            if archived && !self.filter.include_archived {
                continue;
            }
            let paths: Vec<_> = pages.iter().map(|page| page.path.clone()).collect();
            check_catalog_cancelled(cancelled)?;
            let result = if quick {
                let meta = catalog_cache::peek_summary(SUMMARY_CACHE_KIND, &paths)
                    .unwrap_or_else(|| preview(pages, paths));
                Ok((meta, Vec::new()))
            } else {
                catalog_cache::summary(SUMMARY_CACHE_KIND, &paths, || {
                    history::read(pages, ReadMode::Summary)
                        .map(|(detail, warnings)| (detail.meta, warnings))
                })
            };
            match result {
                Ok((mut meta, ws)) => {
                    warnings.extend(ws);
                    relations.apply(id, meta.codex.as_mut().unwrap());
                    if !self.filter.include_empty && !meta.stats_pending && meta.is_empty() {
                        continue;
                    }
                    if let Some(name) = self.titles.get(&meta.session_id) {
                        meta.title = Some(name.clone());
                    }
                    meta.archived = archived;
                    let display = meta.cwd.clone().unwrap_or_else(|| "(未知目录)".into());
                    items.push((meta, display));
                }
                Err(error) => warnings.push(error),
            }
        }
        let mut catalog = agents::assemble_catalog(items);
        if let Some(home) = self.root.parent() {
            projects::apply(&mut catalog, home, &mut warnings);
        }
        catalog.warnings = warnings;
        check_catalog_cancelled(cancelled)?;
        Ok(catalog)
    }
}

fn preview(pages: &[history::Page], paths: Vec<PathBuf>) -> SessionMeta {
    let latest = &pages.last().unwrap().header;
    let mut meta = to_meta(latest, None);
    meta.codex = Some(super::thread::metadata(latest, pages));
    meta.size_bytes = Some(pages.iter().map(|page| page.header.size_bytes).sum());
    meta.last_ts = paths
        .iter()
        .filter_map(|path| std::fs::metadata(path).ok()?.modified().ok())
        .max()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .and_then(|elapsed| agents::ms_to_iso(elapsed.as_millis() as i64))
        .or(meta.last_ts);
    meta.source_files = paths;
    meta.stats_pending = true;
    meta
}
