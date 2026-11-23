use crate::agents::{self, search::SessionRef, Subagent, TargetKind};
use serde_json::Value;
use std::{
    collections::BTreeMap,
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
    layout_parent: Option<PathBuf>,
    parent_confirmed: bool,
    parent_id_hint: Option<String>,
    embedded_id: bool,
}

pub(super) struct ProjectSessions {
    project: PathBuf,
    pub(super) files: BTreeMap<PathBuf, Location>,
    ids: BTreeMap<String, Vec<PathBuf>>,
    unreadable: BTreeMap<PathBuf, String>,
    directory_warnings: Vec<String>,
}

impl ProjectSessions {
    pub(super) fn scan(project: PathBuf, cancelled: &dyn Fn() -> bool) -> Result<Self, String> {
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
                            parent_confirmed: layout_parent.as_ref().is_some_and(|parent| recorded_parent(&path, &header).as_ref() == Some(parent)),
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
        Ok(Self { project, files, ids, unreadable, directory_warnings })
    }

    pub(super) fn open(root: &Path, project_dir: &str, id: &str) -> Result<Self, String> {
        if !is_project_name(project_dir) {
            return Err("项目目录名无效".into());
        }
        if !is_valid_session_id(id) {
            return Err("会话 ID 无效".into());
        }
        let project = agents::verify_within_root(root, &root.join(project_dir), TargetKind::Dir)?;
        Self::scan(project, &|| false)
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

    pub(super) fn subagent(&self, location: &Location, project_dir: &str) -> Option<Subagent> {
        let parent_path = location.layout_parent.as_ref()?;
        let parent = self.files.get(parent_path)
            .filter(|parent| location.parent_confirmed && self.locate(&parent.id).is_ok());
        Some(Subagent {
            parent_id: parent.map(|file| file.id.clone()).or_else(|| location.parent_id_hint.clone()),
            parent: parent.map(|file| SessionRef {
                session_id: file.id.clone(),
                project_dir: project_dir.to_owned(),
            }),
            orphan: parent.is_none(),
            ..Subagent::default()
        })
    }

    pub(super) fn deletion_targets(&self, id: &str) -> Result<Vec<(String, PathBuf)>, String> {
        let root = self.locate(id)?;
        if let Some(error) = self.directory_warnings.first() {
            return Err(error.clone());
        }
        let mut children = BTreeMap::<_, Vec<_>>::new();
        for file in self.files.values() {
            if let Some(parent) = &file.layout_parent {
                children.entry(parent).or_default().push(file);
            }
        }
        let mut targets = Vec::new();
        let mut pending = vec![root];
        while let Some(location) = pending.pop() {
            if !location.embedded_id {
                return Err("Pi 会话头部缺少内嵌 id，无法确认删除目标".into());
            }
            self.locate(&location.id)?;
            let child_dir = location.path.with_extension("");
            if let Some((_, error)) = self.unreadable.iter().find(|(path, _)| {
                path.starts_with(&child_dir) || filename_id(path) == Some(&location.id)
            }) {
                return Err(error.clone());
            }
            let target = agents::verify_within_root(&self.project, &location.path, TargetKind::File)?;
            let (header, lines, _) = open(&target)?;
            if header["id"].as_str() != Some(&location.id)
                || location.layout_parent.as_ref().is_some_and(|parent| {
                    (recorded_parent(&location.path, &header).as_ref() == Some(parent)) != location.parent_confirmed
                })
            {
                return Err("Pi 会话身份或父子关系已变化，请刷新后重试".into());
            }
            for line in lines {
                let line = line.map_err(|error| format!("读取 Pi 会话 {target:?} 失败：{error}"))?;
                if !line.trim().is_empty() && serde_json::from_str::<Value>(&line).is_err() {
                    return Err(format!("{target:?} 含无法解析的行，无法完成身份复核"));
                }
            }
            targets.push((location.id.clone(), target));
            for child in children.get(&location.path).into_iter().flatten().rev() {
                if !child.parent_confirmed {
                    return Err(format!("Pi 子会话 {:?} 的父会话与存储位置不符，已中止删除", child.path));
                }
                pending.push(child);
            }
        }
        Ok(targets)
    }
}
