//! Qoder 的展示归属：识别受控自动工作区，并把项目内运行目录归并到项目根目录。
use super::super::{Catalog, SessionGroupHint, SessionMeta};
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

fn automatic_workspace(cwd: &Path, session_id: &str, home: &Path) -> bool {
    if let Ok(relative) = cwd.strip_prefix(home.join("Documents/Qoder")) {
        let parts: Vec<_> = relative.components().collect();
        if let [Component::Normal(day), Component::Normal(leaf)] = parts.as_slice() {
            let day = day.to_string_lossy();
            let valid_day = day.len() == 10
                && day.bytes().enumerate().all(|(i, c)| {
                    if i == 4 || i == 7 {
                        c == b'-'
                    } else {
                        c.is_ascii_digit()
                    }
                });
            // 仅识别 Qoder 已观察到的“日期/会话前缀”布局，普通手选目录不能靠名字判为无项目。
            if valid_day
                && session_id.len() >= 8
                && session_id.as_bytes()[..8].iter().all(u8::is_ascii_hexdigit)
                && leaf
                    .to_string_lossy()
                    .eq_ignore_ascii_case(&session_id[..8])
            {
                return true;
            }
        }
    }
    if let Ok(relative) = cwd.strip_prefix(home.join(".qoderwork/workspace")) {
        // QoderWork 管理的直接工作区沿用现有可见记录，只归组，不移除或改写原日志。
        return matches!(
            relative.components().collect::<Vec<_>>().as_slice(),
            [Component::Normal(_)]
        );
    }
    false
}

fn workspace_root(cwd: &Path, home: &Path) -> Option<PathBuf> {
    if !cwd.is_absolute() {
        return None;
    }
    let mut start = PathBuf::new();
    for component in cwd.components() {
        if matches!(component, Component::Normal(name) if name == ".local") {
            break;
        }
        start.push(component);
    }
    // .local 内的每次诊断可生成自己的 AGENTS.md，不能把它误当成一个新项目。
    for ancestor in start.ancestors() {
        if ancestor == home || ancestor.parent().is_none() {
            break;
        }
        let git = ancestor.join(".git");
        if git.is_dir() || git.is_file() || ancestor.join("AGENTS.md").is_file() {
            return Some(ancestor.to_path_buf());
        }
    }
    None
}

pub(super) fn apply(catalog: &mut Catalog, home: &Path) {
    let mut roots: HashMap<String, Option<PathBuf>> = HashMap::new();
    for session in &catalog.sessions {
        let Some(cwd) = session.cwd.as_deref() else {
            continue;
        };
        let path = Path::new(cwd);
        let root = if automatic_workspace(path, &session.session_id, home) {
            None
        } else {
            let root = roots
                .entry(cwd.to_string())
                .or_insert_with(|| workspace_root(path, home));
            let Some(root) = root else { continue };
            Some(root.to_string_lossy().to_string())
        };
        catalog.session_group_hints.push(hint(session, root));
    }
}

fn hint(session: &SessionMeta, root: Option<String>) -> SessionGroupHint {
    SessionGroupHint {
        project_dir: session.project_dir.clone(),
        session_id: session.session_id.clone(),
        root,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn automatic_workspace_requires_managed_layout_and_matching_session_prefix() {
        let home = Path::new("/Users/demo");
        assert!(automatic_workspace(
            &home.join("Documents/Qoder/2026-10-16/1234abcd"),
            "1234abcd-1111-4444-8888-000000000000",
            home
        ));
        assert!(!automatic_workspace(
            &home.join("Documents/Qoder/2026-10-16/1234abcd"),
            "eeeeffff-1111-4444-8888-000000000000",
            home
        ));
        assert!(!automatic_workspace(
            &home.join("Documents/Qoder/manual"),
            "1234abcd",
            home
        ));
        assert!(!automatic_workspace(
            Path::new("/tmp/random-uuid"),
            "1234abcd",
            home
        ));
        assert!(automatic_workspace(
            &home.join(".qoderwork/workspace/task-id"),
            "1234abcd",
            home
        ));
        assert!(!automatic_workspace(
            &home.join(".qoderwork/workspace-other/task-id"),
            "1234abcd",
            home
        ));
    }

    #[test]
    fn diagnostic_local_roots_use_real_project_markers_and_keep_nested_repositories() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        fs::write(home.join("AGENTS.md"), "global instructions").unwrap();
        let root = home.join("work/real-project");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("AGENTS.md"), "project instructions").unwrap();
        let run = root.join("poc/.local/local-runs/diag-one");
        fs::create_dir_all(&run).unwrap();
        fs::write(run.join("AGENTS.md"), "generated diagnostic instructions").unwrap();
        assert_eq!(workspace_root(&run, home), Some(root.clone()));
        let nested = root.join("independent");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join(".git"), "gitdir: /tmp/git-worktree").unwrap();
        assert_eq!(workspace_root(&nested.join("src"), home), Some(nested));
        assert_eq!(workspace_root(&home.join("unrelated/task"), home), None);
    }

    #[test]
    fn catalog_groups_preserve_source_keys_and_authoritative_paths() {
        use crate::agents::claude_family;
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        let root = home.join("work/repo-with-dash");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("AGENTS.md"), "project").unwrap();
        let base = home.join(".qoder/projects");
        let cases = [
            (
                "run-a",
                "1234abcd-1111-4444-8888-000000000000",
                root.join(".local/diag-a"),
            ),
            (
                "run-b",
                "bbbbbbbb-1111-4444-8888-000000000000",
                root.join("poc/.local/diag-b"),
            ),
            (
                "automatic",
                "1234abcd-1111-4444-8888-000000000000",
                home.join("Documents/Qoder/2026-10-16/1234abcd"),
            ),
            (
                "manual",
                "cccccccc-1111-4444-8888-000000000000",
                home.join("manual/random-looking-directory"),
            ),
        ];
        for (source, id, cwd) in &cases {
            fs::create_dir_all(base.join(source)).unwrap();
            fs::write(base.join(source).join(format!("{id}.jsonl")),format!("{}\n",
                serde_json::json!({"type":"user","sessionId":id,"entrypoint":"cli","cwd":cwd,"message":{"content":"hello"}}))).unwrap();
        }
        let mut catalog = claude_family::catalog(&base, super::super::OPTS, false).unwrap();
        for (source, _, cwd) in &cases {
            assert_eq!(
                catalog
                    .projects
                    .iter()
                    .find(|p| &p.dir_name == source)
                    .unwrap()
                    .display_path,
                cwd.to_string_lossy()
            );
        }
        let before = serde_json::to_value(&catalog.sessions).unwrap();
        apply(&mut catalog, home);
        assert_eq!(catalog.session_group_hints.len(), 3);
        assert_eq!(serde_json::to_value(&catalog.sessions).unwrap(), before);
        let hints = &catalog.session_group_hints;
        assert_eq!(
            hints
                .iter()
                .find(|h| h.project_dir == "automatic")
                .unwrap()
                .root,
            None
        );
        assert_eq!(
            hints
                .iter()
                .filter(|h| h.root.as_deref() == root.to_str())
                .count(),
            2
        );
        for (source, id, cwd) in &cases {
            let detail =
                claude_family::read_session(&base, source, id, super::super::OPTS).unwrap();
            assert_eq!(detail.meta.project_dir, *source);
            assert_eq!(detail.meta.cwd.as_deref(), cwd.to_str());
        }
    }
}
