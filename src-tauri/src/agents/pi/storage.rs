use crate::agents::{self, TargetKind};
use serde_json::Value;
use std::{fs, io::{BufRead, BufReader, Lines}, path::{Path, PathBuf}};

const SUBAGENT_ARTIFACTS: &str = "subagent-artifacts";

pub(super) fn is_valid_session_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
        && id.as_bytes()[0].is_ascii_alphanumeric()
        && id.as_bytes()[id.len() - 1].is_ascii_alphanumeric()
}

fn is_project_name(name: &str) -> bool {
    agents::claude_family::is_safe_name(name) && name != SUBAGENT_ARTIFACTS
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
    Some(stem.rsplit_once('_').map_or(stem, |(_, id)| id))
}

pub(super) fn session_id<'a>(header: &'a Value, path: &'a Path) -> Option<&'a str> {
    header["id"].as_str().filter(|id| !id.is_empty())
        .or_else(|| filename_id(path)).filter(|id| is_valid_session_id(id))
}

fn is_transcript(path: &Path) -> bool {
    path.extension().is_some_and(|extension| extension == "jsonl")
}

pub(super) fn session_files(root: &Path, warnings: &mut Vec<String>) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for entry in agents::read_dir_warned(root, warnings) {
        if entry.file_type().is_ok_and(|kind| kind.is_dir())
            && entry.file_name().to_str().is_some_and(is_project_name)
        {
            files.extend(agents::read_dir_warned(&entry.path(), warnings).into_iter()
                .map(|entry| entry.path()).filter(|path| is_transcript(path)));
        }
    }
    files
}

pub(super) struct Location {
    pub(super) path: PathBuf,
    project: PathBuf,
    embedded_id: bool,
    unreadable_candidate: Option<String>,
}

pub(super) fn locate(root: &Path, project_dir: &str, id: &str) -> Result<Location, String> {
    if !is_project_name(project_dir) {
        return Err("项目目录名无效".into());
    }
    if !is_valid_session_id(id) {
        return Err("会话 ID 无效".into());
    }
    let project = agents::verify_within_root(root, &root.join(project_dir), TargetKind::Dir)?;
    let entries = fs::read_dir(&project).map_err(|error| format!("无法读取 Pi 项目 {project_dir}：{error}"))?;
    let mut found = None;
    let mut unreadable_candidate = None;
    for entry in entries {
        let path = entry.map_err(|error| format!("无法读取 Pi 项目目录项：{error}"))?.path();
        if !is_transcript(&path) {
            continue;
        }
        match open(&path) {
            Ok((header, _, _)) if session_id(&header, &path) == Some(id) => {
                if found.is_some() {
                    return Err(format!("项目 {project_dir} 中找到多个 ID 为 {id} 的 Pi 会话"));
                }
                found = Some((path, header["id"].as_str() == Some(id)));
            }
            Err(error) if filename_id(&path) == Some(id) => unreadable_candidate = Some(error),
            _ => {}
        }
    }
    let Some((path, embedded_id)) = found else {
        return Err(unreadable_candidate.unwrap_or_else(|| "未找到该 Pi 会话".into()));
    };
    Ok(Location { path, project, embedded_id, unreadable_candidate })
}

pub(super) fn deletion_target(root: &Path, project_dir: &str, id: &str) -> Result<PathBuf, String> {
    let location = locate(root, project_dir, id)?;
    if let Some(error) = location.unreadable_candidate {
        return Err(error);
    }
    if !location.embedded_id {
        return Err("Pi 会话头部缺少内嵌 id，无法确认删除目标".into());
    }
    let target = agents::verify_within_root(&location.project, &location.path, TargetKind::File)?;
    let (header, lines, _) = open(&target)?;
    if header["id"].as_str() != Some(id) {
        return Err("Pi 会话身份已变化，请刷新后重试".into());
    }
    for line in lines {
        let line = line.map_err(|error| format!("读取 Pi 会话 {target:?} 失败：{error}"))?;
        if !line.trim().is_empty() && serde_json::from_str::<Value>(&line).is_err() {
            return Err(format!("{target:?} 含无法解析的行，无法完成身份复核"));
        }
    }
    Ok(target)
}
