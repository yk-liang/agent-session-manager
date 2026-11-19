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

    pub(super) fn discovers(self, root: &Path, path: &Path, kind: FileType) -> bool {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let readable = kind.is_file() || kind.is_symlink() && !matches!(self, Self::Antigravity);
        readable && self.accepts_path(root, path, &name)
    }

    fn legacy_opencode(self, path: &Path) -> bool {
        matches!(self, Self::OpenCode)
            && path.extension().is_some_and(|ext| ext == "json")
            && path.ancestors().any(|dir| dir.ends_with("storage/message"))
    }

    pub(super) fn descends(self, path: &Path) -> bool {
        !matches!(self, Self::OpenCode)
            || path.file_name().is_some_and(|name| name == "storage")
            || path.ancestors().any(|dir| dir.ends_with("storage/message"))
    }

    fn accepts_path(self, root: &Path, path: &Path, name: &str) -> bool {
        self.legacy_opencode(path)
            || (self.accepts(name)
                && (!matches!(self, Self::OpenCode) || path.parent() == Some(root)))
    }

    pub(super) fn changed_file(self, root: &Path, path: &Path) -> Option<PathBuf> {
        let name = path.file_name()?.to_string_lossy();
        if self.accepts_path(root, path, &name) {
            return Some(path.to_path_buf());
        }
        if matches!(self, Self::JsonLines(_)) {
            return None;
        }
        let database = name.strip_suffix("-wal")?;
        let database_path = path.with_file_name(database);
        self.accepts_path(root, &database_path, database)
            .then_some(database_path)
    }

    pub(super) fn parser_version(self) -> u8 {
        match self {
            Self::JsonLines(agent) => cache::jsonl_parser_version(agent),
            Self::OpenCode => opencode::PARSER_VERSION,
            Self::Antigravity => antigravity::PARSER_VERSION,
        }
    }

    pub(super) fn stamps(self, path: &Path) -> std::io::Result<Vec<cache::Stamp>> {
        match self {
            Self::JsonLines(_) => cache::stamp(path).map(|stamp| vec![stamp]),
            Self::OpenCode if self.legacy_opencode(path) => {
                cache::stamp(path).map(|stamp| vec![stamp])
            }
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
            Self::OpenCode if self.legacy_opencode(path) => {
                let stamps = self.stamps(path).map_err(|error| error.to_string())?;
                if !force
                    && old.is_some_and(|old| {
                        old.parser_version == self.parser_version() && old.stamps == stamps
                    })
                {
                    return Ok((old.unwrap().clone(), 0));
                }
                opencode::read_legacy(path, stamps)
            }
            Self::OpenCode => opencode::read(path, old, force, cancel),
            Self::Antigravity => antigravity::read(path, old, force, cancel),
        }
    }
}
