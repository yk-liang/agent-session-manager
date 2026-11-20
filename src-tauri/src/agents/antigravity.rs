pub(crate) mod models;
pub(crate) mod proto;
pub(crate) mod storage;
mod steps;
mod summaries;
mod transcript;

use super::{
    assemble_catalog, expand_tilde, read_dir_warned, AgentSource, AgentWatchPaths, Catalog,
    DeletePolicy, SessionDetail, SessionMeta,
};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

const DEFAULT_ROOTS: [&str; 6] = [
    ".gemini/antigravity-cli",
    ".gemini/antigravity",
    ".gemini/antigravity-ide",
    ".gemini/antigravity-backup",
    ".gemini/antigravity-acp",
    ".config/antigravity",
];

fn data_roots() -> (Vec<PathBuf>, bool) {
    if let Ok(value) = std::env::var("ANTIGRAVITY_DATA_DIR") {
        let paths: Vec<_> = value
            .split(',')
            .map(str::trim)
            .filter(|path| !path.is_empty())
            .map(expand_tilde)
            .filter_map(|path| std::path::absolute(path).ok())
            .collect();
        if !paths.is_empty() {
            return (paths, true);
        }
    }
    let home = dirs::home_dir().unwrap_or_default();
    (
        DEFAULT_ROOTS.iter().map(|path| home.join(path)).collect(),
        false,
    )
}

pub(crate) fn conversation_dirs() -> Vec<PathBuf> {
    let (roots, custom) = data_roots();
    let mut dirs = Vec::new();
    for root in roots {
        let nested = root.join("conversations");
        let dir = if custom && !nested.is_dir() {
            root
        } else {
            nested
        };
        let dir = dir.canonicalize().unwrap_or(dir);
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    dirs
}

pub(crate) fn is_conversation(name: &str) -> bool {
    name != summaries::DATABASE && name.ends_with(".db")
}

fn is_cli(path: &Path) -> bool {
    path.components()
        .any(|part| part.as_os_str() == "antigravity-cli")
}

fn candidate_files(roots: &[PathBuf], warnings: &mut Vec<String>) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for root in roots {
        match fs::metadata(root) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            _ => {}
        }
        paths.extend(
            read_dir_warned(root, warnings)
                .into_iter()
                .filter_map(|entry| {
                    let name = entry.file_name();
                    if !name.to_str().is_some_and(is_conversation) {
                        return None;
                    }
                    match entry.file_type() {
                        Ok(kind) if kind.is_file() => Some(entry.path()),
                        Ok(_) => None,
                        Err(error) => {
                            warnings.push(super::diagnostics::io(
                                "catalog",
                                "file_type",
                                &entry.path(),
                                &error,
                            ));
                            None
                        }
                    }
                }),
        );
    }
    paths.sort();
    paths
}

fn catalog_in(roots: &[PathBuf], include_empty: bool, cancelled: &dyn Fn() -> bool) -> Result<Catalog, String> {
    let mut warnings = Vec::new();
    let mut sessions = BTreeMap::<_, SessionMeta>::new();
    for root in roots {
        super::check_catalog_cancelled(cancelled)?;
        let index = load_index(root, &mut warnings);
        for path in candidate_files(std::slice::from_ref(root), &mut warnings) {
            super::check_catalog_cancelled(cancelled)?;
            let summary = path.file_stem().and_then(|name| name.to_str()).and_then(|id| index.get(id));
            if summary.is_some_and(|entry| entry.child) {
                continue;
            }
            let sources = [path.clone(), super::scanning::sqlite_wal_path(&path)];
            match super::catalog_cache::summary("antigravity-v3", &sources, || {
                let detail = transcript::read(&path, false)?;
                Ok((detail.meta, detail.warnings))
            }) {
                Ok((mut meta, ws)) => {
                    warnings.extend(ws);
                    if include_empty || meta.real_user_count > 0 {
                        summaries::apply(&mut meta, &path, summary);
                        let key = (meta.project_dir.clone(), meta.session_id.clone());
                        if sessions.get(&key).is_none_or(|old| prefer(&meta, old)) {
                            sessions.insert(key, meta);
                        }
                    }
                }
                Err(error) => warnings.push(error),
            }
        }
    }
    let mut catalog = assemble_catalog(
        sessions
            .into_values()
            .map(|meta| {
                let display = meta.cwd.clone().unwrap_or_else(|| "(未知目录)".into());
                (meta, display)
            })
            .collect(),
    );
    catalog.warnings = warnings;
    Ok(catalog)
}

fn load_index(root: &Path, warnings: &mut Vec<String>) -> std::collections::HashMap<String, summaries::Summary> {
    summaries::read(&summaries::path(root)).unwrap_or_else(|error| {
        warnings.push(error);
        Default::default()
    })
}

fn prefer(next: &SessionMeta, old: &SessionMeta) -> bool {
    (
        super::timestamp(next.last_ts.as_deref()),
        next.user_count,
        next.assistant_count,
        next.entrypoint.as_deref() == Some("cli"),
    ) > (
        super::timestamp(old.last_ts.as_deref()),
        old.user_count,
        old.assistant_count,
        old.entrypoint.as_deref() == Some("cli"),
    )
}

pub struct AntigravitySource;

impl AgentSource for AntigravitySource {
    fn id(&self) -> &'static str {
        "antigravity"
    }
    fn display_name(&self) -> &'static str {
        "Antigravity"
    }
    fn is_installed(&self) -> bool {
        conversation_dirs().iter().any(|dir| dir.is_dir())
    }
    fn data_dir(&self) -> Option<PathBuf> {
        let (roots, _) = data_roots();
        roots
            .iter()
            .find(|root| root.join("conversations").is_dir())
            .or_else(|| roots.iter().find(|root| root.is_dir()))
            .or_else(|| roots.first())
            .cloned()
    }
    fn watch_locations(&self) -> Vec<(PathBuf, AgentWatchPaths)> {
        conversation_dirs().into_iter().flat_map(|root| {
            let index_root = summaries::path(&root).parent().unwrap_or(&root).to_path_buf();
            [
                (root, self.watch_paths()),
                (index_root, AgentWatchPaths {
                    root_files: vec![summaries::DATABASE, summaries::DATABASE_WAL],
                    ..Default::default()
                }),
            ]
        }).collect()
    }
    fn watch_paths(&self) -> AgentWatchPaths {
        AgentWatchPaths {
            root_file_patterns: vec![("", ".db"), ("", ".db-wal")],
            ..Default::default()
        }
    }
    fn scan_catalog(
        &self,
        filter: crate::agents::CatalogFilter,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Catalog, String> {
        catalog_in(&conversation_dirs(), filter.include_empty, cancelled)
    }
    fn read_session(&self, project_dir: &str, session_id: &str) -> Result<SessionDetail, String> {
        let mut warnings = Vec::new();
        let mut detail: Option<SessionDetail> = None;
        for path in candidate_files(&conversation_dirs(), &mut warnings) {
            if path.file_stem().and_then(|stem| stem.to_str()) != Some(session_id) {
                continue;
            }
            let index = path.parent().map(|root| load_index(root, &mut warnings)).unwrap_or_default();
            let summary = index.get(session_id);
            if summary.is_some_and(|entry| entry.child) {
                continue;
            }
            match transcript::read(&path, true) {
                Ok(mut next) => {
                    summaries::apply(&mut next.meta, &path, summary);
                    if next.meta.project_dir == project_dir
                        && detail.as_ref().is_none_or(|old| prefer(&next.meta, &old.meta))
                    {
                        detail = Some(next);
                    }
                }
                Err(error) => warnings.push(error),
            }
        }
        if let Some(mut detail) = detail {
            detail.warnings.extend(warnings);
            return Ok(detail);
        }
        Err(warnings.into_iter().next()
            .unwrap_or_else(|| "Antigravity 会话不存在或项目已变化".into()))
    }
    fn search_events(&self, meta: &SessionMeta) -> Result<Vec<Value>, String> {
        let path = meta.source_files.first().ok_or("缺少会话文件定位信息")?;
        let root = path.parent().ok_or("缺少会话目录")?;
        let index = summaries::read(&summaries::path(root)).unwrap_or_default();
        let summary = index.get(&meta.session_id);
        let mut detail = transcript::read(path, true)?;
        summaries::apply(&mut detail.meta, path, summary);
        if summary.is_some_and(|entry| entry.child)
            || detail.meta.session_id != meta.session_id || detail.meta.project_dir != meta.project_dir
        {
            return Err("会话来源已变化，请刷新后重试".into());
        }
        Ok(detail.events)
    }
    fn delete_policy(&self) -> DeletePolicy {
        DeletePolicy::ReadOnly
    }
    fn resume_entrypoint(&self) -> Option<&'static str> {
        Some("cli")
    }
    fn resume_command(&self, session_id: &str, fork: bool) -> Option<String> {
        if fork || !super::claude_family::is_session_id(session_id) {
            return None;
        }
        conversation_dirs()
            .iter()
            .any(|root| is_cli(root) && root.join(format!("{session_id}.db")).is_file())
            .then(|| {
                format!(
                    "{} --conversation {}",
                    super::shell_quote("agy"),
                    super::shell_quote(session_id)
                )
            })
    }
}
