//! Claude Code 项目目录与 JSONL 会话。

use super::claude_family::{self, FamilyOptions};
use super::{shell_quote, AgentSource, Catalog, ConfigFile, SessionDetail};
use std::path::{Path, PathBuf};

const OPTS: FamilyOptions = FamilyOptions {
    use_runtime_model: false,
    filter_non_cli_entrypoint: false,
};

fn configured_dirs() -> Vec<PathBuf> {
    let overrides = std::env::var("CLAUDE_CONFIG_DIR").unwrap_or_default();
    let configured: Vec<_> = overrides.split(',').map(str::trim)
        .filter(|path| !path.is_empty())
        .map(super::expand_tilde)
        .collect();
    if !configured.is_empty() {
        return configured;
    }
    let home = dirs::home_dir().unwrap_or_default();
    vec![home.join(".claude"), home.join(".config/claude")]
}

fn claude_dir() -> PathBuf {
    configured_dirs().remove(0)
}

pub(crate) fn project_roots() -> Vec<PathBuf> {
    configured_dirs().into_iter().map(|dir| dir.join("projects")).collect()
}

fn distinct_project_roots() -> impl Iterator<Item = (usize, PathBuf)> {
    let mut seen = std::collections::HashSet::new();
    project_roots().into_iter().enumerate()
        .filter(move |(_, root)| seen.insert(root.canonicalize().unwrap_or_else(|_| root.clone())))
}

fn project_root_exists(root: &Path) -> bool {
    !root.metadata().is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
}

fn project_location(project_dir: &str) -> Result<(PathBuf, &str), String> {
    if claude_family::is_safe_name(project_dir) {
        return Ok((claude_dir().join("projects"), project_dir));
    }
    let (root, name) = project_dir.rsplit_once('/').ok_or("项目目录名无效")?;
    if !claude_family::is_safe_name(name) {
        return Err("项目目录名无效".into());
    }
    distinct_project_roots()
        .find(|(index, path)| *index > 0 && path == Path::new(root))
        .map(|(_, path)| (path, name))
        .ok_or_else(|| "项目目录无效".into())
}

fn resolve_claude_bin() -> String {
    let local = claude_dir().join("local").join("claude");
    if local.exists() {
        local.to_string_lossy().to_string()
    } else {
        "claude".to_string()
    }
}

pub struct ClaudeSource;

impl AgentSource for ClaudeSource {
    fn id(&self) -> &'static str {
        "claude"
    }
    fn display_name(&self) -> &'static str {
        "Claude Code"
    }
    fn is_installed(&self) -> bool {
        project_roots().iter().any(|root| root.is_dir())
    }
    fn data_dir(&self) -> Option<PathBuf> {
        Some(claude_dir())
    }
    fn watch_paths(&self) -> super::AgentWatchPaths {
        super::AgentWatchPaths::transcripts("projects", Some(2))
    }
    fn watch_locations(&self) -> Vec<(PathBuf, super::AgentWatchPaths)> {
        distinct_project_roots().filter_map(|(_, root)| {
            Some((root.parent()?.to_path_buf(), self.watch_paths()))
        }).collect()
    }
    fn scan_catalog(
        &self,
        filter: crate::agents::CatalogFilter,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Catalog, String> {
        let mut items = Vec::new();
        let mut warnings = Vec::new();
        for (index, root) in distinct_project_roots().filter(|(_, root)| project_root_exists(root)) {
            let mut sessions = claude_family::catalog_items(&root, OPTS, filter.include_empty, &mut warnings, cancelled)?;
            if index > 0 {
                for (meta, _) in &mut sessions {
                    meta.project_dir = format!("{}/{}", root.display(), meta.project_dir);
                }
            }
            items.extend(sessions);
        }
        let mut catalog = super::assemble_catalog(items);
        catalog.warnings = warnings;
        Ok(catalog)
    }
    fn read_session(&self, project_dir: &str, session_id: &str) -> Result<SessionDetail, String> {
        let (root, name) = project_location(project_dir)?;
        let mut detail = claude_family::read_session(&root, name, session_id, OPTS)?;
        detail.meta.project_dir = project_dir.to_owned();
        Ok(detail)
    }
    fn delete_session(&self, project_dir: &str, session_id: &str) -> Result<(), String> {
        let (root, name) = project_location(project_dir)?;
        claude_family::delete_session(&root, name, session_id)
    }
    fn deletion_targets(
        &self,
        project_dir: &str,
        session_id: &str,
    ) -> Result<Option<Vec<PathBuf>>, String> {
        let (root, name) = project_location(project_dir)?;
        claude_family::deletion_targets(&root, name, session_id).map(Some)
    }
    fn resume_command(&self, session_id: &str, _fork: bool) -> Option<String> {
        Some(format!(
            "{} --resume {}",
            shell_quote(&resolve_claude_bin()),
            shell_quote(session_id)
        ))
    }
    fn config_spec(&self) -> Vec<ConfigFile> {
        vec![ConfigFile {
            key: "settings.json".into(),
            label: "settings.json".into(),
            path: claude_dir().join("settings.json"),
        }]
    }
}
