use crate::agents::{self, lineage::{Lineage, Node}, SessionLocation, Subagent, TargetKind};
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashSet},
    fs,
    io::{BufRead, BufReader, Lines},
    path::{Component, Path, PathBuf},
};

const SUBAGENT_ARTIFACTS: &str = "subagent-artifacts";

pub(super) fn is_valid_session_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
        && id.as_bytes()[0].is_ascii_alphanumeric()
        && id.as_bytes()[id.len() - 1].is_ascii_alphanumeric()
}

fn is_project_name(name: &str) -> bool {
    agents::claude_family::is_safe_name(name) && !name.starts_with('.') && name != SUBAGENT_ARTIFACTS
}

pub(super) fn open(path: &Path) -> Result<(Value, Lines<BufReader<fs::File>>, Option<u64>), String> {
    let file = agents::open_jsonl(path).map_err(|error| format!("无法读取 Pi 会话文件 {path:?}：{error}"))?;
    let size = file.metadata().ok().map(|meta| meta.len());
    let mut lines = BufReader::new(file).lines();
    let first = lines.next().ok_or_else(|| format!("Pi 会话文件为空：{path:?}"))?
        .map_err(|error| format!("读取 Pi 会话头部 {path:?} 失败：{error}"))?;
    let header: Value = serde_json::from_str(&first)
        .map_err(|_| format!("Pi 会话头部不是有效 JSON：{path:?}"))?;
    if header["type"] != "session" {
        return Err(format!("Pi 会话文件头类型无效：{path:?}"));
    }
    Ok((header, lines, size))
}

fn filename_id(path: &Path) -> Option<&str> {
    let stem = path.file_stem()?.to_str()?;
    let id = stem.rsplit_once('_').map_or(stem, |(_, id)| id);
    (stem != "session" && is_valid_session_id(id)).then_some(id)
}

pub(super) fn session_id<'a>(header: &'a Value, path: &'a Path) -> Option<&'a str> {
    header["id"].as_str().filter(|id| !id.is_empty())
        .or_else(|| filename_id(path)).filter(|id| is_valid_session_id(id))
}

pub(super) fn projects(root: &Path, warnings: &mut Vec<String>) -> Vec<(String, PathBuf)> {
    agents::read_dir_warned(root, warnings).into_iter()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            (is_project_name(&name) && entry.file_type().is_ok_and(|kind| kind.is_dir()))
                .then(|| (name, entry.path()))
        })
        .collect()
}

fn in_project(project: &Path, path: &Path) -> bool {
    path.strip_prefix(project).is_ok_and(|relative| {
        relative.components().all(|part| matches!(part, Component::Normal(_)))
    })
}

pub(super) fn entry_ids(path: &Path) -> Result<HashSet<String>, String> {
    let (_, lines, _) = open(path)?;
    let mut ids = HashSet::new();
    agents::for_each_jsonl_line(lines, |entry| {
        if let Some(id) = entry["id"].as_str() {
            ids.insert(id.to_owned());
        }
    });
    Ok(ids)
}

fn recorded_parent(path: &Path, header: &Value) -> Option<PathBuf> {
    let recorded = header["parentSession"].as_str().filter(|path| !path.is_empty()).map(Path::new)?;
    let absolute = if recorded.is_absolute() {
        recorded.to_path_buf()
    } else {
        path.parent()?.join(recorded)
    };
    Some(absolute.parent()?.canonicalize().ok()?.join(absolute.file_name()?))
}

fn layout_parent(project: &Path, path: &Path) -> Option<PathBuf> {
    let folder = path.parent()?;
    let run = folder.parent()?;
    let is_run = path.file_name().is_some_and(|name| name == "session.jsonl")
        && folder.file_name()?.to_str()?.strip_prefix("run-")
            .is_some_and(|step| !step.is_empty() && step.bytes().all(|byte| byte.is_ascii_digit()));
    let is_fork = run.file_name().is_some_and(|name| name == "forks") && filename_id(path).is_some();
    if !is_run && !is_fork {
        return None;
    }
    let owner = run.parent()?;
    if !in_project(project, owner) || owner == project {
        return None;
    }
    let mut filename = owner.file_name()?.to_os_string();
    filename.push(".jsonl");
    Some(owner.with_file_name(filename))
}

#[derive(Debug)]
pub(super) struct Location {
    pub(super) path: PathBuf,
    pub(super) id: String,
    cwd: Option<String>,
    /// 按子代理日志的存储结构推断的父会话文件；有值即为子会话。
    layout_parent: Option<PathBuf>,
    /// 会话头记录的父会话文件。自定义会话目录下它与存储结构不一致，以它为准。
    recorded_parent: Option<PathBuf>,
    parent_id_hint: Option<String>,
    embedded_id: bool,
}

/// 子会话的父会话以会话头记录为准，且须是本项目中已扫描的会话。
fn parent_of<'a>(files: &'a BTreeMap<PathBuf, Location>, file: &Location) -> Option<&'a Location> {
    file.layout_parent.as_ref()?;
    files.get(file.recorded_parent.as_ref()?)
}

pub(super) struct ProjectSessions {
    project: PathBuf,
    pub(super) files: BTreeMap<PathBuf, Location>,
    ids: BTreeMap<String, Vec<PathBuf>>,
    lineage: Lineage,
    unreadable: BTreeMap<PathBuf, String>,
    directory_warnings: Vec<String>,
}

impl ProjectSessions {
    pub(super) fn scan(name: &str, project: PathBuf, cancelled: &dyn Fn() -> bool) -> Result<Self, String> {
        let project = project.canonicalize().unwrap_or(project);
        let mut files = BTreeMap::new();
        let mut ids = BTreeMap::<String, Vec<PathBuf>>::new();
        let mut unreadable = BTreeMap::new();
        let mut directory_warnings = Vec::new();
        let mut pending = vec![project.clone()];
        while let Some(dir) = pending.pop() {
            for entry in agents::read_dir_warned(&dir, &mut directory_warnings) {
                agents::check_catalog_cancelled(cancelled)?;
                let path = entry.path();
                let child = dir != project;
                if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                    if entry.file_name().to_str().is_some_and(is_project_name) {
                        pending.push(path);
                    }
                    continue;
                }
                if path.extension().is_none_or(|ext| ext != "jsonl") {
                    continue;
                }
                let layout_parent = layout_parent(&project, &path);
                if child && layout_parent.is_none() {
                    continue;
                }
                match open(&path) {
                    Ok((header, _, _)) => {
                        let Some(id) = session_id(&header, &path) else {
                            unreadable.insert(path.clone(), format!("无法解析 Pi 会话 ID：{path:?}"));
                            continue;
                        };
                        let pathname = header["parentSession"].as_str().map(Path::new);
                        let location = Location {
                            id: id.to_owned(),
                            cwd: header["cwd"].as_str().filter(|cwd| !cwd.trim().is_empty()).map(str::to_owned),
                            recorded_parent: recorded_parent(&path, &header),
                            parent_id_hint: pathname.and_then(filename_id)
                                .or_else(|| layout_parent.as_deref().and_then(filename_id)).map(str::to_owned),
                            layout_parent,
                            embedded_id: header["id"].as_str() == Some(id),
                            path: path.clone(),
                        };
                        ids.entry(id.to_owned()).or_default().push(path.clone());
                        files.insert(path, location);
                    }
                    Err(error) => {
                        unreadable.insert(path, error);
                    }
                }
            }
        }
        let nodes = files.values().map(|file| {
            let unique = ids[&file.id].len() == 1;
            let parent = parent_of(&files, file);
            Node {
                id: file.id.clone(),
                location: unique.then(|| SessionLocation { session_id: file.id.clone(), project_dir: name.to_owned(), cwd: file.cwd.clone() }),
                parents: parent.map(|parent| parent.id.clone()).into_iter().collect(),
                child: file.layout_parent.is_some(),
            }
        }).collect::<Vec<_>>();
        Ok(Self { project, lineage: Lineage::new(nodes), files, ids, unreadable, directory_warnings })
    }

    pub(super) fn open(root: &Path, project_dir: &str, id: &str) -> Result<Self, String> {
        if !is_project_name(project_dir) {
            return Err("项目目录名无效".into());
        }
        if !is_valid_session_id(id) {
            return Err("会话 ID 无效".into());
        }
        let project = agents::verify_within_root(root, &root.join(project_dir), TargetKind::Dir)?;
        Self::scan(project_dir, project, &|| false)
    }

    pub(super) fn warnings(&self) -> impl Iterator<Item = &String> {
        self.directory_warnings.iter().chain(self.unreadable.values())
    }

    pub(super) fn locate(&self, id: &str) -> Result<&Location, String> {
        match self.ids.get(id).map(Vec::as_slice) {
            Some([path]) => Ok(&self.files[path]),
            Some(_) => Err(format!("项目中找到多个 ID 为 {id} 的 Pi 会话")),
            None => Err(self.unreadable.iter().find(|(path, _)| filename_id(path) == Some(id))
                .map(|(_, error)| error.clone()).unwrap_or_else(|| "未找到该 Pi 会话".into())),
        }
    }

    pub(super) fn subagent(&self, location: &Location) -> Option<Subagent> {
        let mut relation = self.lineage.subagent(&location.id)?;
        if relation.parent_id.is_none() {
            relation.parent_id.clone_from(&location.parent_id_hint);
        }
        Some(relation)
    }

    /// 父会话可用（非孤儿）时它的日志文件；fork 子会话的继承前缀要与它比对。
    pub(super) fn parent_file(&self, location: &Location) -> Option<&Path> {
        let relation = self.lineage.subagent(&location.id)?;
        relation.parent.and(parent_of(&self.files, location)).map(|parent| parent.path.as_path())
    }

    /// 会话及其全部子会话的日志（父在前）。存储在它之下却无法归属的子会话日志会中止删除。
    pub(super) fn deletion_targets(&self, id: &str) -> Result<Vec<(String, PathBuf)>, String> {
        self.locate(id)?;
        if let Some(error) = self.directory_warnings.first() {
            return Err(error.clone());
        }
        let mut targets = Vec::new();
        for id in self.lineage.subtree(id)? {
            let location = self.locate(id)?;
            if !location.embedded_id {
                return Err("Pi 会话头部缺少内嵌 id，无法确认删除目标".into());
            }
            let child_dir = location.path.with_extension("");
            if let Some((_, error)) = self.unreadable.iter().find(|(path, _)| {
                path.starts_with(&child_dir) || filename_id(path) == Some(id)
            }) {
                return Err(error.clone());
            }
            if let Some(stray) = self.files.values().find(|file| {
                file.layout_parent.as_ref() == Some(&location.path) && parent_of(&self.files, file).is_none()
            }) {
                return Err(format!("Pi 子会话 {:?} 的父会话与存储位置不符，已中止删除", stray.path));
            }
            let target = agents::verify_within_root(&self.project, &location.path, TargetKind::File)?;
            let (header, lines, _) = open(&target)?;
            if header["id"].as_str() != Some(id)
                || recorded_parent(&location.path, &header) != location.recorded_parent
            {
                return Err("Pi 会话身份或父子关系已变化，请刷新后重试".into());
            }
            for line in lines {
                let line = line.map_err(|error| format!("读取 Pi 会话 {target:?} 失败：{error}"))?;
                if !line.trim().is_empty() && serde_json::from_str::<Value>(&line).is_err() {
                    return Err(format!("{target:?} 含无法解析的行，无法完成身份复核"));
                }
            }
            targets.push((id.to_owned(), target));
        }
        Ok(targets)
    }
}
