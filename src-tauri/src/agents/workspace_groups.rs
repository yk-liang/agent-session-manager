//! 跨 Agent 的临时工作区展示规则；不改变会话、原 cwd 或后端操作使用的源目录。
use super::{Catalog, SessionGroupHint};
use std::collections::{HashMap, HashSet};
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};

pub(crate) fn normalized(path: &Path) -> PathBuf {
    // macOS 日志可能记录 /private 下的真实路径，也可能记录 /var 或 /tmp 别名。
    if path.starts_with("/private/var") || path.starts_with("/private/tmp") {
        return Path::new("/").join(path.strip_prefix("/private").unwrap());
    }
    path.to_path_buf()
}

fn temporary_relative(cwd: &Path, temporary_root: &Path) -> Option<PathBuf> {
    if !cwd.is_absolute() || cwd.components().any(|c| matches!(c, Component::ParentDir)) {
        return None;
    }
    let cwd = normalized(cwd);
    let configured = normalized(temporary_root);
    for root in [&configured, Path::new("/tmp"), Path::new("/var/tmp")] {
        if root.is_absolute() && root.parent().is_some() {
            if let Ok(relative) = cwd.strip_prefix(root) {
                return Some(relative.to_path_buf());
            }
        }
    }
    // 历史目录可能已经清理，或属于旧的 per-user 临时根，不能只依赖 canonicalize。
    let mut parts = cwd.strip_prefix("/var/folders").ok()?.components();
    match (parts.next(), parts.next(), parts.next()) {
        (Some(Component::Normal(_)), Some(Component::Normal(_)), Some(Component::Normal(kind)))
            if kind == "T" =>
        {
            Some(parts.as_path().to_path_buf())
        }
        _ => None,
    }
}

fn automatic_temporary(cwd: &Path, session_id: &str, temporary_root: &Path) -> bool {
    let Some(relative) = temporary_relative(cwd, temporary_root) else {
        return false;
    };
    let mut parts = relative.components();
    if let (Some(Component::Normal(namespace)), Some(Component::Normal(run))) =
        (parts.next(), parts.next())
    {
        let namespace = namespace.to_string_lossy();
        let run = run.to_string_lossy();
        if namespace
            .strip_prefix("pytest-of-")
            .is_some_and(|name| !name.is_empty())
            && run.strip_prefix("pytest-").is_some_and(|suffix| {
                suffix == "current"
                    || !suffix.is_empty() && suffix.bytes().all(|b| b.is_ascii_digit())
            })
        {
            return true;
        }
    }
    // 目录名与源日志里的会话 ID 同时吻合，才能把通用 runtime/sessions 工作区判为自动目录。
    session_id.len() >= 8
        && !session_id.contains(['/', '\\'])
        && relative.file_name().is_some_and(|name| name == session_id)
        && relative
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|name| name == "sessions")
}

fn repository_root(cwd: &Path, home: &Path) -> Result<Option<PathBuf>, ()> {
    let home = normalized(home);
    // 临时入口可能链接到仓库子目录；目录已清理时仍沿历史路径查找存活的父仓库。
    let resolved = match cwd.canonicalize() {
        Ok(path) => path,
        Err(error) if matches!(error.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) => {
            cwd.to_path_buf()
        }
        Err(_) => return Err(()),
    };
    for ancestor in resolved.ancestors() {
        if normalized(ancestor) == home || ancestor.parent().is_none() {
            break;
        }
        match std::fs::metadata(ancestor.join(".git")) {
            Ok(meta) if meta.is_dir() || meta.is_file() => {
                return Ok(Some(
                    ancestor
                        .canonicalize()
                        .unwrap_or_else(|_| ancestor.to_path_buf()),
                ));
            }
            Ok(_) => return Err(()),
            Err(error)
                if matches!(error.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) => {}
            Err(_) => return Err(()),
        }
    }
    Ok(None)
}

pub(super) fn apply(catalog: &mut Catalog) {
    let home = dirs::home_dir().unwrap_or_default();
    apply_with_environment(catalog, &home, &std::env::temp_dir());
}

fn apply_with_environment(catalog: &mut Catalog, home: &Path, temporary_root: &Path) {
    let mut classified: HashSet<_> = catalog
        .session_group_hints
        .iter()
        .map(|h| (h.project_dir.clone(), h.session_id.clone()))
        .collect();
    let mut roots = HashMap::new();
    for session in &catalog.sessions {
        let key = (session.project_dir.clone(), session.session_id.clone());
        if classified.contains(&key)
            || catalog
                .projectless_session_ids
                .contains(&session.session_id)
        {
            continue;
        }
        let Some(cwd) = session.cwd.as_deref() else {
            continue;
        };
        let path = Path::new(cwd);
        if !automatic_temporary(path, &session.session_id, temporary_root) {
            continue;
        }
        // 自动运行目录里的 AGENTS.md 常为临时生成；真实 Git 仓库（含 worktree）仍优先归组。
        // 无法检查归属时保留原路径，而非将权限错误当成“没有项目”。
        let Ok(root) = roots
            .entry(cwd.to_owned())
            .or_insert_with(|| repository_root(path, home))
        else {
            continue;
        };
        catalog.session_group_hints.push(SessionGroupHint {
            project_dir: session.project_dir.clone(),
            session_id: session.session_id.clone(),
            root: root
                .as_ref()
                .map(|root| root.to_string_lossy().into_owned()),
        });
        classified.insert(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::SessionMeta;
    use serde_json::json;
    use std::fs;

    fn session(id: &str, directory: &str, cwd: &Path) -> SessionMeta {
        let mut meta: SessionMeta = serde_json::from_value(json!({
            "sessionId":id,"projectDir":directory,"cwd":cwd,"userCount":1,"assistantCount":0,
            "realUserCount":1,"assistantTextCount":0,"sizeBytes":1,"hasTranscript":true
        }))
        .unwrap();
        meta.source_files = vec![PathBuf::from("/original/transcript.jsonl")];
        meta
    }

    fn catalog(sessions: Vec<SessionMeta>) -> Catalog {
        Catalog {
            sessions,
            projects: vec![],
            warnings: vec![],
            projectless_session_ids: vec![],
            session_group_hints: vec![],
        }
    }

    #[test]
    fn known_pytest_layouts_survive_cleanup_and_macos_path_aliases() {
        for prefix in [
            "/tmp",
            "/private/tmp",
            "/var/tmp",
            "/private/var/tmp",
            "/var/folders/zz/asm-fixture/T",
            "/private/var/folders/zz/asm-fixture/T",
        ] {
            let cwd = PathBuf::from(prefix)
                .join("pytest-of-demo/pytest-42/test_case0/runtime with spaces/sessions/id");
            let mut data = catalog(vec![session("session-one", "original-dir", &cwd)]);
            apply_with_environment(
                &mut data,
                Path::new("/Users/demo"),
                Path::new("/another-temp-root"),
            );
            assert_eq!(data.session_group_hints.len(), 1, "{}", cwd.display());
            assert!(data.session_group_hints[0].root.is_none());
        }
    }

    #[test]
    fn unknown_manual_relative_and_escaping_paths_are_not_classified() {
        for path in [
            "/tmp/random-uuid",
            "/work/pytest-of-demo/pytest-42/test_case0",
            "/tmp/pytest-of-demo/pytest-manual/test_case0",
            "/tmp/pytest-of-/pytest-42/test_case0",
            "/tmp-other/pytest-of-demo/pytest-42/test_case0",
            "/tmp/pytest-of-demo/pytest-42/../manual",
            "/var/folders/zz/asm-fixture/C/pytest-of-demo/pytest-42/test_case0",
            "tmp/pytest-of-demo/pytest-42/test_case0",
        ] {
            assert!(
                !automatic_temporary(Path::new(path), "session-one", Path::new("/system-temp")),
                "{path}"
            );
        }
        assert!(automatic_temporary(
            Path::new("/tmp/pytest-of-demo/pytest-current/test_case0"),
            "id",
            Path::new("/system-temp")
        ));
    }

    #[test]
    fn runtime_layout_requires_an_authoritative_matching_session_id() {
        let cwd =
            Path::new("/tmp/runtime with spaces/sessions/1234abcd-1111-4444-8888-000000000000");
        assert!(automatic_temporary(
            cwd,
            "1234abcd-1111-4444-8888-000000000000",
            Path::new("/system-temp")
        ));
        assert!(!automatic_temporary(
            cwd,
            "different-session",
            Path::new("/system-temp")
        ));
        assert!(!automatic_temporary(
            Path::new("/work/runtime/sessions/session-one"),
            "session-one",
            Path::new("/system-temp")
        ));
        assert!(!automatic_temporary(
            Path::new("/tmp/runtime/sessions/id"),
            "id",
            Path::new("/system-temp")
        ));
        assert!(automatic_temporary(
            Path::new("/custom/tmp/runtime/sessions/session-one"),
            "session-one",
            Path::new("/custom/tmp")
        ));
    }

    #[test]
    fn generated_instructions_do_not_create_projects_but_real_git_roots_win() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let temporary_root = home.join("custom-temp");
        let project = temporary_root.join("pytest-of-demo/pytest-1/test_case0");
        let cwd = project.join("runtime/sessions/session-one");
        fs::create_dir_all(&cwd).unwrap();
        fs::write(cwd.join("AGENTS.md"), "generated runtime instructions").unwrap();
        fs::create_dir(home.join(".git")).unwrap();
        let original = session("session-one", "source-a", &cwd);
        let mut automatic = catalog(vec![original.clone()]);
        apply_with_environment(&mut automatic, &home, &temporary_root);
        assert!(automatic.session_group_hints[0].root.is_none());
        fs::write(project.join(".git"), "gitdir: /worktree/metadata").unwrap();
        let mut repository = catalog(vec![original]);
        apply_with_environment(&mut repository, &home, &temporary_root);
        assert_eq!(
            repository.session_group_hints[0].root.as_deref(),
            project.canonicalize().unwrap().to_str()
        );
    }

    #[test]
    fn hints_are_idempotent_and_never_replace_source_identity_or_existing_ownership() {
        let cwd = Path::new("/tmp/pytest-of-demo/pytest-42/test_case0");
        let mut data = catalog(vec![
            session("same-id", "one", cwd),
            session("same-id", "two", cwd),
        ]);
        data.session_group_hints.push(SessionGroupHint {
            project_dir: "one".into(),
            session_id: "same-id".into(),
            root: Some("/explicit/project".into()),
        });
        let before = serde_json::to_value(&data.sessions).unwrap();
        let source_files: Vec<_> = data
            .sessions
            .iter()
            .map(|s| s.source_files.clone())
            .collect();
        for _ in 0..2 {
            apply_with_environment(
                &mut data,
                Path::new("/Users/demo"),
                Path::new("/system-temp"),
            );
        }
        assert_eq!(data.session_group_hints.len(), 2);
        assert_eq!(
            data.session_group_hints[0].root.as_deref(),
            Some("/explicit/project")
        );
        assert_eq!(data.session_group_hints[1].project_dir, "two");
        assert!(data.session_group_hints[1].root.is_none());
        assert_eq!(serde_json::to_value(&data.sessions).unwrap(), before);
        assert_eq!(
            data.sessions
                .iter()
                .map(|s| s.source_files.clone())
                .collect::<Vec<_>>(),
            source_files
        );
    }

    #[cfg(unix)]
    #[test]
    fn temporary_symlink_to_a_repository_subdirectory_keeps_the_real_project() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let project = home.join("real-project");
        fs::create_dir_all(project.join("src")).unwrap();
        fs::create_dir(project.join(".git")).unwrap();
        let temporary_root = tmp.path().join("temporary");
        let runtime = temporary_root.join("runtime/sessions");
        fs::create_dir_all(&runtime).unwrap();
        let cwd = runtime.join("session-one");
        std::os::unix::fs::symlink(project.join("src"), &cwd).unwrap();
        let mut data = catalog(vec![session("session-one", "source", &cwd)]);
        apply_with_environment(&mut data, &home, &temporary_root);
        assert_eq!(
            data.session_group_hints[0].root.as_deref(),
            project.canonicalize().unwrap().to_str()
        );
    }

    #[test]
    fn common_catalog_assembly_supplies_hints_for_every_adapter() {
        let sessions: Vec<_> = ["qoder", "claude", "codex", "opencode", "pi"]
            .iter()
            .map(|agent| {
                session(
                    "session-one",
                    agent,
                    Path::new("/tmp/pytest-of-demo/pytest-42/test_case0"),
                )
            })
            .collect();
        let data = super::super::assemble_catalog(
            sessions
                .into_iter()
                .map(|s| (s, "original display".into()))
                .collect(),
        );
        assert_eq!(data.sessions.len(), 5);
        assert_eq!(data.projects.len(), 5);
        assert_eq!(data.session_group_hints.len(), 5);
        assert!(data
            .session_group_hints
            .iter()
            .all(|hint| hint.root.is_none()));
    }
}
