use super::Source;
use crate::{
    agents::{antigravity, claude_family, codex, opencode, pi},
    usage::{discover, source_error},
};
use std::{
    collections::{BTreeSet, HashSet},
    fs::{self, FileType},
    io::ErrorKind,
    path::{Path, PathBuf},
};

pub(in crate::usage) struct SessionInventory {
    transcripts: BTreeSet<PathBuf>,
    ids: HashSet<String>,
    claude_family: bool,
}

impl SessionInventory {
    pub fn read(agent: &str, roots: &[PathBuf]) -> Result<Self, String> {
        let source = Source::for_agent(agent);
        let mut transcripts = discover(agent, roots, &|| false)?;
        transcripts.retain(|path| !source.is_file_root(path));
        let claude_family = matches!(source, Source::JsonLines("claude" | "qoder"));
        let mut ids = HashSet::new();
        for path in &transcripts {
            match source {
                Source::JsonLines("codex") => ids.extend(codex::transcript_session_id(path)?),
                Source::JsonLines("pi") => {
                    ids.insert(pi::transcript_session_id(path)?);
                }
                Source::Antigravity => {
                    let id = antigravity::storage::session_id(path)
                        .map_err(|error| source_error(path, error))?;
                    ids.insert(id.to_owned());
                }
                Source::OpenCode if !source.legacy_opencode(path) => {
                    ids.extend(opencode::session_ids(path)?);
                }
                _ => {}
            }
        }
        for root in roots {
            if claude_family {
                ids.extend(grouped_ids(root, |name, kind| {
                    let id = if kind.is_dir() { Some(name) } else { name.strip_suffix(".jsonl") };
                    id.filter(|id| claude_family::is_session_id(id))
                })?);
            } else if matches!(source, Source::OpenCode) {
                ids.extend(grouped_ids(&root.join("storage/session"), |name, kind| {
                    name.strip_suffix(".json").filter(|id| kind.is_file() && id.starts_with("ses_"))
                })?);
            }
        }
        Ok(Self {
            transcripts,
            ids,
            claude_family,
        })
    }

    pub fn contains(&self, session: &str, parent: Option<&str>, source: Option<&str>) -> bool {
        // Claude 和 Qoder 的子代理可能只记录在父会话或嵌套日志中。
        self.ids.contains(session)
            || self.transcripts.contains(Path::new(session))
            || self.claude_family
                && (parent.is_some_and(|id| self.ids.contains(id))
                    || source.is_some_and(|path| self.transcripts.contains(Path::new(path))))
    }
}

fn grouped_ids(
    root: &Path,
    session_id: impl Fn(&str, FileType) -> Option<&str>,
) -> Result<Vec<String>, String> {
    let groups = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(source_error(root, error)),
    };
    let mut ids = Vec::new();
    for group in groups {
        let group = group.map_err(|error| source_error(root, error))?;
        let path = group.path();
        if !group
            .file_type()
            .map_err(|error| source_error(&path, error))?
            .is_dir()
        {
            continue;
        }
        for entry in fs::read_dir(&path).map_err(|error| source_error(&path, error))? {
            let entry = entry.map_err(|error| source_error(&path, error))?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            let kind = entry
                .file_type()
                .map_err(|error| source_error(&entry.path(), error))?;
            ids.extend(session_id(name, kind).map(str::to_owned));
        }
    }
    Ok(ids)
}
