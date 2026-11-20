use super::{antigravity, cache::FileCache, codex::failures, opencode};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, FileType},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

pub(super) mod jsonl;
pub(super) mod sessions;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(super) struct Stamp {
    pub len: u64,
    modified: i64,
    modified_ns: i64,
    inode: u64,
    device: u64,
    changed: i64,
    changed_ns: i64,
}
pub(super) fn stamp(path: &Path) -> std::io::Result<Stamp> {
    let m = fs::metadata(path)?;
    if !m.is_file() {
        return Err(std::io::Error::other("Source is not a regular file"));
    }
    Ok(Stamp {
        len: m.len(),
        modified: m.mtime(),
        modified_ns: m.mtime_nsec(),
        inode: m.ino(),
        device: m.dev(),
        changed: m.ctime(),
        changed_ns: m.ctime_nsec(),
    })
}
pub(super) fn sqlite_stamps(path: &Path) -> std::io::Result<Vec<Stamp>> {
    let mut stamps = vec![stamp(path)?];
    match stamp(&crate::agents::scanning::sqlite_wal_path(path)) {
        Ok(value) => stamps.push(value),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    Ok(stamps)
}

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

    pub(super) fn is_file_root(self, path: &Path) -> bool {
        matches!(self, Self::JsonLines("codex"))
            && path.file_name().is_some_and(|name| name == failures::DATABASE)
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
        if self.is_file_root(root) {
            return (path == root || path == crate::agents::scanning::sqlite_wal_path(root))
                .then(|| root.to_path_buf());
        }
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

    pub(super) fn parser_version(self, path: &Path) -> u8 {
        if self.is_file_root(path) {
            return failures::PARSER_VERSION;
        }
        match self {
            Self::JsonLines(_) => jsonl::PARSER_VERSION,
            Self::OpenCode => opencode::PARSER_VERSION,
            Self::Antigravity => antigravity::PARSER_VERSION,
        }
    }

    pub(super) fn stamps(self, path: &Path) -> std::io::Result<Vec<Stamp>> {
        if self.is_file_root(path) {
            return sqlite_stamps(path);
        }
        match self {
            Self::JsonLines(_) => stamp(path).map(|stamp| vec![stamp]),
            Self::OpenCode if self.legacy_opencode(path) => {
                stamp(path).map(|stamp| vec![stamp])
            }
            _ => sqlite_stamps(path),
        }
    }

    pub(super) fn read(
        self,
        path: &Path,
        old: Option<&FileCache>,
        force: bool,
        cancel: &dyn Fn() -> bool,
    ) -> Result<(FileCache, u64), String> {
        if self.is_file_root(path) {
            return failures::read(path, old, force, cancel);
        }
        match self {
            Self::JsonLines(agent) => jsonl::read(path, agent, old, force, cancel),
            Self::OpenCode if self.legacy_opencode(path) => {
                let stamps = self.stamps(path).map_err(|error| error.to_string())?;
                if !force
                    && old.is_some_and(|old| {
                        old.parser_version == self.parser_version(path) && old.stamps == stamps
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
