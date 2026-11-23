use super::{history::Page, sqlite_home, ParsedRollout};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::Path;

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadMeta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    pub subagent: bool,
    pub can_resume: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub empty: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub orphan: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_owner: Option<ProjectOwner>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectOwner {
    pub session_id: String,
    pub project_dir: String,
    pub cwd: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct ThreadHeader {
    pub meta: ThreadMeta,
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
                parent_id: parent_id.map(str::to_owned),
                subagent,
                can_resume,
                ..Default::default()
            },
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
            link.subagent |= header.meta.subagent;
            link.parents.extend(
                header
                    .meta
                    .parent_id
                    .iter()
                    .chain(&header.conflicting_parent)
                    .cloned(),
            );
        }
        link
    }

    fn parent(&self) -> Option<&str> {
        (self.parents.len() == 1).then(|| self.parents.first().unwrap().as_str())
    }
}

pub(super) fn metadata(latest: &ParsedRollout, pages: &[Page]) -> ThreadMeta {
    let mut meta = latest.thread.meta.clone();
    let link = Link::from_pages(pages);
    link.apply(&mut meta);
    meta
}

impl Link {
    fn apply(&self, meta: &mut ThreadMeta) {
        meta.subagent = self.subagent;
        meta.parent_id = self.parent().map(str::to_owned);
        meta.can_resume &= !meta.subagent;
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

pub(super) struct Relations {
    links: BTreeMap<String, Link>,
    local_ids: HashSet<String>,
    cycles: HashSet<String>,
    owners: BTreeMap<String, ProjectOwner>,
}

impl Relations {
    pub fn new(groups: &BTreeMap<String, Vec<Page>>, edges: SpawnEdges) -> Self {
        let mut links: BTreeMap<_, _> = groups
            .iter()
            .map(|(id, pages)| (id.clone(), Link::from_pages(pages)))
            .collect();
        for (child, parent) in edges {
            let link = links.entry(child).or_default();
            link.parents.insert(parent);
            link.subagent = true;
        }
        let mut cycles = HashSet::new();
        let mut visited = HashSet::new();
        for id in links.keys() {
            let mut chain = Vec::new();
            let mut path = HashSet::new();
            let mut current = id.as_str();
            while !visited.contains(current) {
                if !path.insert(current) {
                    cycles.extend(
                        chain
                            .iter()
                            .skip_while(|&&id| id != current)
                            .map(|&id: &&str| id.to_owned()),
                    );
                    break;
                }
                chain.push(current);
                let Some(parent) = links.get(current).and_then(Link::parent) else {
                    break;
                };
                current = parent;
            }
            visited.extend(chain);
        }
        let mut owners = BTreeMap::<String, ProjectOwner>::new();
        let mut roots = HashMap::<&str, ProjectOwner>::new();
        for id in groups.keys() {
            let mut current = id.as_str();
            let mut path = Vec::new();
            let owner = loop {
                if let Some(owner) = owners.get(current) {
                    break owner.clone();
                }
                let parent = links[current]
                    .parent()
                    .filter(|parent| !cycles.contains(current) && groups.contains_key(*parent));
                let Some(parent) = parent else {
                    break roots
                        .entry(current)
                        .or_insert_with(|| {
                            let cwd = groups[current]
                                .last()
                                .and_then(|page| page.header.cwd.clone());
                            let project_dir = cwd
                                .as_deref()
                                .map(super::escape_cwd)
                                .unwrap_or_else(|| "-unknown".into());
                            ProjectOwner {
                                session_id: current.into(),
                                project_dir,
                                cwd,
                            }
                        })
                        .clone();
                };
                path.push(current);
                current = parent;
            };
            for id in path {
                owners.insert(id.into(), owner.clone());
            }
        }
        Self {
            links,
            local_ids: groups.keys().cloned().collect(),
            cycles,
            owners,
        }
    }

    pub fn apply(&self, id: &str, meta: &mut ThreadMeta) {
        let link = &self.links[id];
        link.apply(meta);
        if link.subagent {
            meta.orphan = Some(
                self.cycles.contains(id)
                    || link
                        .parent()
                        .is_none_or(|parent| !self.local_ids.contains(parent)),
            );
        }
        meta.project_owner = self.owners.get(id).cloned();
    }

    pub fn subtree<'a>(&'a self, root: &'a str) -> Result<Vec<&'a str>, String> {
        if !self.links.contains_key(root) {
            return Err("未找到该 Codex 会话".into());
        }
        let mut children = BTreeMap::<&str, Vec<&str>>::new();
        for (id, link) in &self.links {
            for parent in &link.parents {
                children.entry(parent).or_default().push(id);
            }
        }
        let mut ordered = Vec::new();
        let mut pending = vec![root];
        while let Some(id) = pending.pop() {
            if self.cycles.contains(id) || self.links[id].parents.len() > 1 {
                return Err(format!(
                    "Codex 会话 {id} 的父子关系存在冲突，无法确认删除范围"
                ));
            }
            ordered.push(id);
            pending.extend(children.get(id).into_iter().flatten().rev().copied());
        }
        Ok(ordered)
    }
}
