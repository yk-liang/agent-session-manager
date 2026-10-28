//! 只读桌面端归属状态：展示分组与日志的真实身份、工作目录保持独立。
use serde_json::Value;
use std::collections::HashSet;
use std::io::Read;
use std::path::Path;

pub(super) fn projectless_ids(home: &Path, warnings: &mut Vec<String>) -> HashSet<String> {
    let path = home.join(".codex-global-state.json");
    let read = || -> Result<HashSet<String>, String> {
        let file = match super::super::open_jsonl(&path) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(HashSet::new()),
            Err(_) => return Err("无法读取 Codex 项目归属，暂按工作目录分组".into()),
        };
        // 状态可能由桌面端同时重写；损坏或超限时保留目录分组，不猜测归属，也不回写状态。
        const MAX_BYTES: u64 = 16 * 1024 * 1024;
        if file
            .metadata()
            .map_err(|_| "无法读取 Codex 项目归属")?
            .len()
            > MAX_BYTES
        {
            return Err("Codex 项目归属数据过大，暂按工作目录分组".into());
        }
        let mut bytes = Vec::new();
        file.take(MAX_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "无法读取 Codex 项目归属")?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err("Codex 项目归属数据过大，暂按工作目录分组".into());
        }
        let state: Value = serde_json::from_slice(&bytes)
            .map_err(|_| "Codex 项目归属数据无法解析，暂按工作目录分组")?;
        let mut ids: HashSet<String> = match state.get("projectless-thread-ids") {
            Some(Value::Array(ids)) => ids
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect(),
            None => state
                .get("thread-projectless-output-directories")
                .and_then(Value::as_object)
                .map(|dirs| dirs.keys().cloned().collect())
                .unwrap_or_default(),
            _ => return Err("Codex 无项目会话标记格式异常，暂按工作目录分组".into()),
        };
        if let Some(assignments) = state
            .get("thread-project-assignments")
            .and_then(Value::as_object)
        {
            for (id, assignment) in assignments {
                let project = assignment
                    .get("projectId")
                    .and_then(Value::as_str)
                    .or_else(|| assignment.as_str());
                // 会话被移入项目后，显式项目归属优先于遗留的无项目标记与产出目录。
                if project.is_some_and(|p| !p.trim().is_empty()) {
                    ids.remove(id);
                }
            }
        }
        Ok(ids)
    };
    match read() {
        Ok(ids) => ids,
        Err(error) => {
            warnings.push(error);
            HashSet::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;

    #[test]
    fn projectless_markers_respect_explicit_assignments_and_do_not_rewrite_state() {
        let home = tempfile::tempdir().unwrap();
        let file = home.path().join(".codex-global-state.json");
        let text = json!({"projectless-thread-ids":["unassigned","moved",null],
            "thread-projectless-output-directories":{"stale":"/tmp/stale/outputs"},
            "thread-project-assignments":{"moved":{"projectKind":"local","projectId":"p1"}}})
        .to_string();
        fs::write(&file, &text).unwrap();
        let mut warnings = Vec::new();
        assert_eq!(
            projectless_ids(home.path(), &mut warnings),
            HashSet::from(["unassigned".to_string()])
        );
        assert!(warnings.is_empty());
        assert_eq!(fs::read_to_string(file).unwrap(), text);
    }

    #[test]
    fn projectless_output_marker_is_only_a_legacy_fallback() {
        let home = tempfile::tempdir().unwrap();
        let file = home.path().join(".codex-global-state.json");
        fs::write(
            &file,
            json!({"thread-projectless-output-directories":{"legacy":"/tmp/task/outputs"}})
                .to_string(),
        )
        .unwrap();
        let mut warnings = Vec::new();
        assert!(projectless_ids(home.path(), &mut warnings).contains("legacy"));
        fs::write(&file,json!({"projectless-thread-ids":[],"thread-projectless-output-directories":{"legacy":"/tmp/task/outputs"}}).to_string()).unwrap();
        assert!(projectless_ids(home.path(), &mut warnings).is_empty());
    }

    #[test]
    fn projectless_state_missing_or_corrupt_falls_back_without_using_other_homes() {
        let home = tempfile::tempdir().unwrap();
        let mut warnings = Vec::new();
        assert!(projectless_ids(home.path(), &mut warnings).is_empty());
        assert!(warnings.is_empty());
        fs::write(home.path().join(".codex-global-state.json"), "{broken").unwrap();
        assert!(projectless_ids(home.path(), &mut warnings).is_empty());
        assert_eq!(warnings.len(), 1);
    }

    #[test]
    fn projectless_catalog_exposes_only_existing_marked_ids_and_keeps_operation_identity() {
        use super::super::{catalog_in, delete_session_with, escape_cwd, read_session_in};
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("sessions");
        let day = root.join("2099/12/31");
        fs::create_dir_all(&day).unwrap();
        for (id, cwd) in [
            ("unassigned", "/tmp/task-a"),
            ("moved", "/tmp/task-b"),
            ("manual", "/Users/demo/Documents/Codex/manual"),
        ] {
            fs::write(day.join(format!("rollout-{id}.jsonl")),format!("{}\n{}\n",
                json!({"type":"session_meta","payload":{"id":id,"cwd":cwd}}),
                json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"hello"}]}})
            )).unwrap();
        }
        let text = json!({"projectless-thread-ids":["unassigned","moved","absent"],
            "thread-project-assignments":{"moved":{"projectId":"project-b","projectKind":"local"}}})
        .to_string();
        let state = home.path().join(".codex-global-state.json");
        fs::write(&state, &text).unwrap();
        let catalog = catalog_in(&root, false).unwrap();
        assert_eq!(catalog.projectless_session_ids, vec!["unassigned"]);
        assert_eq!(
            catalog.projects.len(),
            3,
            "后端仍保留真实项目身份，虚拟分组由侧栏派生"
        );
        let session = catalog
            .sessions
            .iter()
            .find(|s| s.session_id == "unassigned")
            .unwrap();
        assert_eq!(session.project_dir, escape_cwd("/tmp/task-a"));
        assert_eq!(session.cwd.as_deref(), Some("/tmp/task-a"));
        assert_eq!(
            read_session_in(&root, "unassigned")
                .unwrap()
                .meta
                .project_dir,
            session.project_dir
        );
        delete_session_with(&root, &session.project_dir, "unassigned", |paths| {
            assert_eq!(paths.len(), 1);
            Ok(())
        })
        .unwrap();
        assert_eq!(fs::read_to_string(&state).unwrap(), text);
    }
}
