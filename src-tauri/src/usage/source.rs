use super::{antigravity, cache, opencode};
use std::{
    fs::FileType,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy)]
pub(super) enum Source<'a> {
    JsonLines(&'a str),
    OpenCode,
    Antigravity,
}

impl<'a> Source<'a> {
    pub(super) fn for_agent(agent: &'a str) -> Self {
        match agent {
            "opencode" => Self::OpenCode,
            "antigravity" => Self::Antigravity,
            _ => Self::JsonLines(agent),
        }
    }

    fn accepts(self, name: &str) -> bool {
        match self {
            Self::JsonLines(agent) => {
                name.ends_with(".jsonl") || agent == "codex" && name.ends_with(".jsonl.zst")
            }
            Self::OpenCode => name.starts_with("opencode") && name.ends_with(".db"),
            Self::Antigravity => crate::agents::antigravity::is_conversation(name),
        }
    }

    pub(super) fn recursive(self) -> bool {
        matches!(self, Self::JsonLines(_))
    }

    pub(super) fn discovers(self, name: &str, kind: FileType) -> bool {
        self.accepts(name) && (!matches!(self, Self::Antigravity) || kind.is_file())
    }

    pub(super) fn changed_file(self, root: &Path, path: &Path) -> Option<PathBuf> {
        if !self.recursive() && path.parent() != Some(root) {
            return None;
        }
        let name = path.file_name()?.to_string_lossy();
        if self.accepts(&name) {
            return Some(path.to_path_buf());
        }
        if self.recursive() {
            return None;
        }
        let database = name
            .strip_suffix("-wal")
            .filter(|name| self.accepts(name))?;
        Some(path.with_file_name(database))
    }

    pub(super) fn parser_version(self) -> u8 {
        match self {
            Self::JsonLines(_) => cache::JSONL_PARSER_VERSION,
            Self::OpenCode => opencode::PARSER_VERSION,
            Self::Antigravity => antigravity::PARSER_VERSION,
        }
    }

    pub(super) fn stamps(self, path: &Path) -> std::io::Result<Vec<cache::Stamp>> {
        match self {
            Self::JsonLines(_) => cache::stamp(path).map(|stamp| vec![stamp]),
            _ => cache::sqlite_stamps(path),
        }
    }

    pub(super) fn read(
        self,
        path: &Path,
        old: Option<&cache::FileCache>,
        force: bool,
        cancel: &dyn Fn() -> bool,
    ) -> Result<(cache::FileCache, u64), String> {
        match self {
            Self::JsonLines(agent) => cache::read_file(path, agent, old, force, cancel),
            Self::OpenCode => opencode::read(path, old, force, cancel),
            Self::Antigravity => antigravity::read(path, old, force, cancel),
        }
    }
}
