use super::{history::Page, sqlite_home};
use crate::agents::{lineage::{Lineage, Node}, SessionLocation, SessionMeta};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::Path;

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadMeta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_source: Option<String>,
    pub can_resume: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub empty: Option<bool>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct ThreadHeader {
    pub meta: ThreadMeta,
    pub parent_id: Option<String>,
    pub subagent: bool,
    pub conflicting_parent: Option<String>,
    pub nickname: Option<String>,
}

impl ThreadHeader {
    pub fn read(payload: &Value) -> Self {
        let source = payload.get("source");
        let spawn = payload.pointer("/source/subagent/thread_spawn");
        let recorded_parent = text(payload.get("parent_thread_id"));
        let spawn_parent = text(spawn.and_then(|value| value.get("parent_thread_id")));
        let parent_id = recorded_parent.or(spawn_parent);
        let thread_source = text(payload.get("thread_source"));
        let subagent = recorded_parent.is_some()
            || payload.pointer("/source/subagent").is_some()
            || matches!(source.and_then(Value::as_str), Some("subagent"))
            || thread_source == Some("subagent");
        let label = match source {
            Some(Value::String(value)) => Some(value.as_str()),
            Some(Value::Object(fields)) => fields
                .get("custom")
                .and_then(Value::as_str)
                .or_else(|| fields.keys().next().map(String::as_str)),
            _ => None,
        };
        let client = text(payload.get("originator"));
        let origin = if client == Some("codex_exec")
            && matches!(label, None | Some("cli" | "vscode" | "exec"))
        {
            Some("exec")
        } else {
            label
        };
        let can_resume = !subagent
            && matches!(thread_source, None | Some("user"))
            && (source.is_none()
                || matches!(
                    origin,
                    Some("cli" | "vscode" | "exec" | "mcp" | "appServer")
                ));
        Self {
            meta: ThreadMeta {
                source: origin.map(str::to_owned),
                thread_source: thread_source.map(str::to_owned),
                can_resume,
                ..Default::default()
            },
            parent_id: parent_id.map(str::to_owned),
            subagent,
            conflicting_parent: spawn_parent
                .filter(|parent| Some(*parent) != parent_id)
                .map(str::to_owned),
            nickname: text(payload.get("agent_nickname"))
                .or_else(|| text(spawn.and_then(|value| value.get("agent_nickname"))))
                .map(str::to_owned),
        }
    }
}

fn text(value: Option<&Value>) -> Option<&str> {
    value
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
}

#[derive(Default)]
struct Link {
    parents: BTreeSet<String>,
    subagent: bool,
}

impl Link {
    fn from_pages(pages: &[Page]) -> Self {
        let mut link = Self::default();
        for page in pages {
            let header = &page.header.thread;
            link.subagent |= header.subagent;
            link.parents.extend(
                header
                    .parent_id
                    .iter()
                    .chain(&header.conflicting_parent)
                    .cloned(),
            );
        }
        link
    }
}

pub(super) type SpawnEdges = Vec<(String, String)>;
pub(super) const STATE_DB: &str = "state_5.sqlite";
pub(super) const STATE_WAL: &str = "state_5.sqlite-wal";

pub(super) fn spawn_edges(home: &Path) -> Result<SpawnEdges, String> {
    let db = sqlite_home(home).join(STATE_DB);
    if !super::super::present(&db)? {
        return Ok(Vec::new());
    }
    super::super::scanning::sqlite_snapshot(&db, |conn| {
        let exists: bool = conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = 'thread_spawn_edges')",
            [],
            |row| row.get(0),
        )?;
        if !exists {
            return Ok(Vec::new());
        }
        let mut stmt = conn.prepare("SELECT child_thread_id, parent_thread_id FROM thread_spawn_edges")?;
        let edges = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        Ok(edges.collect::<Result<_, _>>()?)
    }).map_err(|error| format!("无法读取 Codex 子会话索引：{error}"))
}

pub(super) fn indexed_subtree(edges: &SpawnEdges, root: &str) -> HashSet<String> {
    let mut children = BTreeMap::<&str, Vec<&str>>::new();
    for (child, parent) in edges {
        children.entry(parent).or_default().push(child);
    }
    let mut ids = HashSet::new();
    let mut pending = vec![root];
    while let Some(id) = pending.pop() {
        if ids.insert(id.to_owned()) {
            pending.extend(children.get(id).into_iter().flatten().copied());
        }
    }
    ids
}

pub(super) fn spawn_edges_warned(root: &Path, warnings: &mut Vec<String>) -> SpawnEdges {
    match root.parent().map(spawn_edges).transpose() {
        Ok(edges) => edges.unwrap_or_default(),
        Err(error) => {
            warnings.push(error);
            Vec::new()
        }
    }
}

pub(super) struct Relations(Lineage);

impl Relations {
    pub fn new(groups: &BTreeMap<String, Vec<Page>>, edges: SpawnEdges) -> Self {
        let threads = groups.iter().map(|(id, pages)| {
            let link = Link::from_pages(pages);
            let cwd = pages.last().and_then(|page| page.header.cwd.clone());
            let project_dir = cwd.as_deref().map(super::escape_cwd).unwrap_or_else(|| "-unknown".into());
            Node {
                id: id.clone(),
                location: Some(SessionLocation { session_id: id.clone(), project_dir, cwd }),
                parents: link.parents.into_iter().collect(),
                child: link.subagent,
            }
        });
        let spawned = edges.into_iter().map(|(child, parent)| Node { id: child, location: None, parents: vec![parent], child: true });
        Self(Lineage::new(threads.chain(spawned)))
    }

    pub fn is_subagent(&self, id: &str) -> bool {
        self.0.is_child(id)
    }

    pub fn apply(&self, id: &str, meta: &mut SessionMeta) {
        meta.subagent = self.0.subagent(id);
        if meta.subagent.is_some() {
            meta.codex.as_mut().unwrap().can_resume = false;
        }
    }

    pub fn subtree<'a>(&'a self, root: &'a str) -> Result<Vec<&'a str>, String> {
        self.0.subtree(root)
    }
}
