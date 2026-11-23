use super::{is_session_id, DeletionPlan, SessionMeta};
use crate::agents::{self, lineage::{Lineage, Node}, SessionLocation, TargetKind};
use serde::Deserialize;
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    sync::OnceLock,
};

pub(super) struct Child {
    pub id: String,
    pub log: PathBuf,
    root_id: String,
    parent: Parent,
    description: Option<String>,
    tool_use_id: Option<String>,
    missing_sidecar: bool,
}

#[derive(PartialEq, Eq)]
enum Parent {
    Root,
    Child(String),
    Unknown,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Metadata {
    description: Option<String>,
    tool_use_id: Option<String>,
    spawn_depth: Option<u64>,
}

fn valid_agent_id(id: &str) -> bool {
    (1..=128).contains(&id.len())
        && id.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn child_id(root: &str, id: &str) -> String {
    format!("subagent:{root}:{id}")
}

pub(super) fn child_identity(session_id: &str) -> Option<(&str, &str)> {
    let (root, id) = session_id.strip_prefix("subagent:")?.split_once(':')?;
    (is_session_id(root) && valid_agent_id(id)).then_some((root, id))
}

pub(super) fn child_log(project: &Path, root: &str, id: &str) -> PathBuf {
    project.join(root).join("subagents").join(format!("agent-{id}.jsonl"))
}

/// 子会话的 ID 带根会话前缀，展示时只用 Agent 记录的原始 ID。
fn display_id(id: &str) -> &str {
    child_identity(id).map_or(id, |(_, id)| id)
}

/// 根会话目录下的全部子会话摘要。嵌套子会话的父会话都在同一组内，关系按整组解析。
pub(super) fn group(
    project: &Path,
    root: Option<&SessionMeta>,
    root_id: &str,
    summary: impl Fn(&Path) -> Result<(SessionMeta, Vec<String>), String>,
    warnings: &mut Vec<String>,
) -> Vec<(Child, SessionMeta)> {
    let mut members = Vec::new();
    for child in children(project, root_id, warnings) {
        match summary(&child.log) {
            Ok((mut meta, ws)) => {
                warnings.extend(ws);
                meta.session_id = child.session_id();
                members.push((child, meta));
            }
            Err(error) => warnings.push(error),
        }
    }
    let root = root.map(|meta| Node::new(meta.location(), None));
    let lineage = Lineage::new(root.into_iter().chain(members.iter().map(|(child, meta)| child.node(Some(meta.location())))));
    for (child, meta) in &mut members {
        child.relate(meta, &lineage);
    }
    members
}

impl Child {
    fn session_id(&self) -> String {
        child_id(&self.root_id, &self.id)
    }

    fn node(&self, location: Option<SessionLocation>) -> Node {
        let parent = match &self.parent {
            Parent::Root => Some(self.root_id.clone()),
            Parent::Child(id) => Some(child_id(&self.root_id, id)),
            Parent::Unknown => None,
        };
        Node { id: self.session_id(), location, parents: parent.into_iter().collect(), child: true }
    }

    fn relate(&self, meta: &mut SessionMeta, lineage: &Lineage) {
        meta.subagent = lineage.subagent(&meta.session_id).map(|mut relation| {
            relation.id = Some(self.id.clone());
            relation.parent_id = relation.parent_id.map(|id| display_id(&id).to_owned());
            relation
        });
        if meta.title.is_none() {
            meta.title.clone_from(&self.description);
        }
    }

    fn verify(&self, project: &Path) -> Result<(PathBuf, Option<PathBuf>), String> {
        let log = agents::verify_within_root(project, &self.log, TargetKind::File)?;
        let reader = BufReader::new(agents::open_jsonl(&log).map_err(|error| error.to_string())?);
        let mut confirmed = false;
        for line in reader.lines() {
            let line = line.map_err(|error| format!("无法读取子会话 {log:?}：{error}"))?;
            if line.trim().is_empty() {
                continue;
            }
            let row: serde_json::Map<String, Value> = serde_json::from_str(&line)
                .map_err(|_| format!("{log:?} 含无法解析的行，无法完成身份复核"))?;
            let agent = row.get("agentId").and_then(Value::as_str);
            let session = row.get("sessionId").and_then(Value::as_str);
            if agent.is_some_and(|id| id != self.id)
                || session.is_some_and(|id| id != self.root_id && id != self.id)
            {
                return Err(format!("子会话 {log:?} 内记录身份不符，已中止删除"));
            }
            confirmed |= agent == Some(self.id.as_str()) || session == Some(self.root_id.as_str()) || session == Some(self.id.as_str());
        }
        if !confirmed {
            return Err(format!("子会话 {log:?} 缺少身份记录，已中止删除"));
        }
        let sidecar = sidecar(&self.log);
        let sidecar = agents::present(&sidecar)?
            .then(|| agents::verify_within_root(project, &sidecar, TargetKind::File))
            .transpose()?;
        Ok((log, sidecar))
    }
}

fn child_logs(project: &Path, root_id: &str, warnings: &mut Vec<String>) -> Vec<(String, PathBuf)> {
    let state = project.join(root_id);
    let dir = state.join("subagents");
    if !state.symlink_metadata().is_ok_and(|meta| meta.is_dir())
        || !dir.symlink_metadata().is_ok_and(|meta| meta.is_dir()) {
        return Vec::new();
    }
    let mut logs: Vec<_> = agents::read_dir_warned(&dir, warnings).into_iter()
        .filter_map(|entry| {
            if !entry.file_type().is_ok_and(|kind| kind.is_file()) {
                return None;
            }
            let log = entry.path();
            let id = log.file_name()?.to_str()?.strip_suffix(".jsonl")?.strip_prefix("agent-")?;
            valid_agent_id(id).then(|| (id.to_owned(), log.clone()))
        })
        .collect();
    logs.sort();
    logs
}

fn sidecar(log: &Path) -> PathBuf {
    log.with_extension("meta.json")
}

fn children(project: &Path, root_id: &str, warnings: &mut Vec<String>) -> Vec<Child> {
    let mut children: Vec<_> = child_logs(project, root_id, warnings).into_iter()
        .map(|(id, log)| {
            let (parent, metadata, missing_sidecar) = match read_metadata(&sidecar(&log)) {
                Ok(None) => (Parent::Root, None, true),
                Ok(Some(meta)) => {
                    let parent = if meta.spawn_depth.is_some_and(|depth| depth > 1) { Parent::Unknown } else { Parent::Root };
                    (parent, Some(meta), false)
                }
                Err(error) => {
                    warnings.push(error);
                    (Parent::Unknown, None, false)
                }
            };
            Child {
                id,
                root_id: root_id.to_owned(),
                parent,
                description: metadata.as_ref().and_then(|meta| meta.description.clone()).filter(|text| !text.trim().is_empty()),
                tool_use_id: metadata.and_then(|meta| meta.tool_use_id).filter(|text| !text.trim().is_empty()),
                log,
                missing_sidecar,
            }
        })
        .collect();
    resolve_nested(&mut children, warnings);
    children
}

fn read_metadata(path: &Path) -> Result<Option<Metadata>, String> {
    if !agents::present(path)? {
        return Ok(None);
    }
    agents::open_jsonl(path).map_err(|error| error.to_string())
        .and_then(|file| serde_json::from_reader(file).map_err(|error| error.to_string()))
        .map(Some)
        .map_err(|error| format!("无法读取子会话元数据 {path:?}：{error}"))
}

fn resolve_nested(children: &mut [Child], warnings: &mut Vec<String>) {
    if children.len() < 2 || !children.iter().any(|child| child.parent == Parent::Unknown || child.missing_sidecar) {
        return;
    }
    let mut callers = HashMap::<String, Option<String>>::new();
    let mut spawn_callers = HashSet::new();
    for child in children.iter() {
        match cached_spawn_calls(&child.log) {
            Ok((calls, warning)) => {
                warnings.extend(warning);
                if !calls.is_empty() {
                    spawn_callers.insert(child.id.clone());
                }
                for call in calls {
                    let caller = callers.entry(call).or_insert_with(|| Some(child.id.clone()));
                    if caller.as_deref() != Some(child.id.as_str()) {
                        *caller = None;
                    }
                }
            }
            Err(error) => warnings.push(error),
        }
    }
    for child in children.iter_mut() {
        if child.missing_sidecar && spawn_callers.iter().any(|id| id != &child.id) {
            child.parent = Parent::Unknown;
        }
        if child.parent != Parent::Unknown {
            continue;
        }
        if let Some(parent) = child.tool_use_id.as_ref().and_then(|id| callers.get(id)).and_then(Option::as_ref).filter(|id| **id != child.id) {
            child.parent = Parent::Child(parent.clone());
        }
    }
}

type SpawnCalls = (HashSet<String>, Option<String>);

fn cached_spawn_calls(path: &Path) -> Result<SpawnCalls, String> {
    static CACHE: OnceLock<agents::catalog_cache::FileCache<SpawnCalls>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| agents::catalog_cache::FileCache::new(1024));
    cache.load("family-spawn-calls", &[path.to_path_buf()], || {
        let file = agents::open_jsonl(path).map_err(|error| format!("无法读取子会话 {path:?}：{error}"))?;
        let mut calls = HashSet::new();
        let warning = agents::scanning::read_jsonl_checked(BufReader::new(file), path, "subagent_relationship", |event| {
            for part in event["message"]["content"].as_array().into_iter().flatten() {
                if part["type"] == "tool_use" && matches!(part["name"].as_str(), Some("Agent" | "Task" | "agent" | "subagent")) {
                    if let Some(id) = part["id"].as_str() {
                        calls.insert(id.to_owned());
                    }
                }
            }
        })?;
        Ok((calls, warning))
    }, |(_, warning)| warning.is_none())
}

/// 删除范围必须基于完整读取的子会话目录。
fn complete_children(project: &Path, root_id: &str) -> Result<Vec<Child>, String> {
    let mut warnings = Vec::new();
    let children = children(project, root_id, &mut warnings);
    if warnings.is_empty() { Ok(children) } else { Err(warnings.join("；")) }
}

/// 根会话的状态目录整体移入废纸篓前，逐个复核其中子会话日志的身份。
pub(super) fn verified_logs(project: &Path, root_id: &str) -> Result<Vec<(String, PathBuf)>, String> {
    complete_children(project, root_id)?.iter()
        .map(|child| child.verify(project).map(|(log, _)| (child.session_id(), log)))
        .collect()
}

pub(super) fn deletion_plan(project: &Path, root_id: &str, id: &str) -> Result<DeletionPlan, String> {
    let group = complete_children(project, root_id)?;
    if group.iter().any(|child| child.parent == Parent::Unknown) {
        return Err("无法确认项目内嵌套子会话的父子关系，已中止删除".into());
    }
    let members: HashMap<_, _> = group.iter().map(|child| (child.session_id(), child)).collect();
    let lineage = Lineage::new(group.iter().map(|child| child.node(None)));
    let target = child_id(root_id, id);
    let mut plan = DeletionPlan::default();
    for session_id in lineage.subtree(&target)? {
        let (log, sidecar) = members[session_id].verify(project)?;
        plan.add_session(session_id.to_owned(), log);
        plan.targets.extend(sidecar);
    }
    Ok(plan)
}
