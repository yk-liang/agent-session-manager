//! QoderCLI 数据源：~/.qoder/projects 下的 Claude 系 JSONL 会话。
//! 配置方案快照事务见 `profiles.rs`。

use std::path::PathBuf;

use super::claude_family::{self, FamilyOptions};
use super::{shell_quote, AgentSource, ConfigFile, SessionDetail};

mod desktop;
mod grouping;
mod models;

/// 用量查询只读已有名称快照，不启动 CLI 或等待联网补全。
pub(crate) fn usage_model_names(ids: &[String]) -> std::collections::HashMap<String, String> {
    models::usage_names(&qoder_dir(), ids)
}

const OPTS: FamilyOptions = FamilyOptions {
    title_events: true,
    filter_non_cli_entrypoint: true,
    filter_qoderwork_cwd: true,
};

pub fn qoder_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_default().join(".qoder")
}

fn projects_dir() -> PathBuf {
    qoder_dir().join("projects")
}

fn load_catalog(
    root: &std::path::Path,
    desktop_db: Option<&std::path::Path>,
    include_empty: bool,
    cancelled: &dyn Fn() -> bool,
) -> Result<super::Catalog, String> {
    let mut catalog = claude_family::catalog(&root.join("projects"), OPTS, include_empty, cancelled)?;
    super::check_catalog_cancelled(cancelled)?;
    desktop::apply(desktop_db, &mut catalog.sessions, &mut catalog.warnings);
    models::apply_cached(root, &mut catalog.sessions);
    if let Some(home) = root.parent() {
        grouping::apply(&mut catalog, home);
    }
    Ok(catalog)
}

fn load_session(
    root: &std::path::Path,
    desktop_db: Option<&std::path::Path>,
    project_dir: &str,
    session_id: &str,
) -> Result<SessionDetail, String> {
    let mut detail =
        claude_family::read_session(&root.join("projects"), project_dir, session_id, OPTS)?;
    desktop::apply(
        desktop_db,
        std::slice::from_mut(&mut detail.meta),
        &mut detail.warnings,
    );
    models::apply_cached(root, std::slice::from_mut(&mut detail.meta));
    Ok(detail)
}

pub fn resolve_qoder_bin() -> String {
    let entry = qoder_dir().join("entry").join("qoder");
    if entry.exists() {
        entry.to_string_lossy().to_string()
    } else {
        "qoder".to_string()
    }
}

pub struct QoderSource;

impl AgentSource for QoderSource {
    fn id(&self) -> &'static str {
        "qoder"
    }
    fn display_name(&self) -> &'static str {
        "Qoder"
    }
    fn is_installed(&self) -> bool {
        projects_dir().exists()
    }
    fn data_dir(&self) -> Option<PathBuf> {
        Some(qoder_dir())
    }
    fn watch_paths(&self, usage: bool) -> super::AgentWatchPaths {
        super::AgentWatchPaths::transcripts("projects", if usage { None } else { Some(2) })
    }
    fn supports_fork(&self) -> bool {
        true
    }
    fn supports_model_names(&self) -> bool {
        true
    }
    fn scan_catalog(
        &self,
        include_empty: bool,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<super::Catalog, String> {
        load_catalog(
            &qoder_dir(),
            desktop::database_path().as_deref(),
            include_empty,
            cancelled,
        )
    }
    fn resolve_model_names(&self, ids: &[String]) -> std::collections::HashMap<String, String> {
        models::resolve(&qoder_dir(), ids)
    }
    fn read_session(&self, project_dir: &str, session_id: &str) -> Result<SessionDetail, String> {
        load_session(
            &qoder_dir(),
            desktop::database_path().as_deref(),
            project_dir,
            session_id,
        )
    }
    fn search_events(&self, meta: &super::SessionMeta) -> Result<Vec<serde_json::Value>, String> {
        // 搜索不需要再次解析模型目录，直接复用统一的会话事件顺序。
        claude_family::read_session(&projects_dir(), &meta.project_dir, &meta.session_id, OPTS)
            .map(|detail| detail.events)
    }
    fn delete_session(&self, project_dir: &str, session_id: &str) -> Result<(), String> {
        claude_family::delete_session(&projects_dir(), project_dir, session_id)
    }
    fn deletion_targets(
        &self,
        project_dir: &str,
        session_id: &str,
    ) -> Result<Option<Vec<PathBuf>>, String> {
        claude_family::deletion_targets(&projects_dir(), project_dir, session_id).map(Some)
    }
    fn resume_command(&self, session_id: &str, fork: bool) -> Option<String> {
        Some(format!(
            "{} --resume {}{}",
            shell_quote(&resolve_qoder_bin()),
            shell_quote(session_id),
            if fork { " --fork-session" } else { "" }
        ))
    }
    fn config_spec(&self) -> Vec<ConfigFile> {
        let dir = qoder_dir();
        vec![
            ConfigFile {
                key: "settings.json".into(),
                label: "settings.json".into(),
                path: dir.join("settings.json"),
            },
            ConfigFile {
                key: "mcp.json".into(),
                label: "mcp.json".into(),
                path: dir.join("mcp.json"),
            },
            ConfigFile {
                key: ".models/default".into(),
                label: ".models/default（默认模型）".into(),
                path: dir.join(".models").join("default"),
            },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_spec_lists_three_files_under_qoder_dir() {
        // settings.json / mcp.json / .models/default 均落在 ~/.qoder 下
        let spec = QoderSource.config_spec();
        let keys: Vec<&str> = spec.iter().map(|c| c.key.as_str()).collect();
        assert_eq!(keys, ["settings.json", "mcp.json", ".models/default"]);
        let dir = qoder_dir();
        assert_eq!(spec[0].path, dir.join("settings.json"));
        assert_eq!(spec[1].path, dir.join("mcp.json"));
        assert_eq!(spec[2].path, dir.join(".models").join("default"));
    }

    #[test]
    fn desktop_title_replaces_last_prompt_in_catalog_and_detail_without_changing_source_identity() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("projects/source-project");
        std::fs::create_dir_all(&project).unwrap();
        let path = project.join("aaaa.jsonl");
        let transcript = concat!(
            "{\"type\":\"user\",\"sessionId\":\"aaaa\",\"cwd\":\"/work/project\",\"message\":{\"content\":\"Initial task\"}}\n",
            "{\"type\":\"runtime-config\",\"model\":\"mode-original\"}\n",
            "{\"type\":\"last-prompt\",\"lastPrompt\":\"Continue\"}\n");
        std::fs::write(&path, transcript).unwrap();
        let db = dir.path().join("desktop.sqlite");
        let writer = rusqlite::Connection::open(&db).unwrap();
        writer.execute_batch("CREATE TABLE chat_sessions(session_id TEXT PRIMARY KEY,title TEXT,deleted_at INTEGER); INSERT INTO chat_sessions VALUES('aaaa','Named desktop session',NULL)").unwrap();
        let before = std::fs::read(&db).unwrap();
        let catalog = load_catalog(dir.path(), Some(&db), false, &|| false).unwrap();
        let meta = &catalog.sessions[0];
        assert_eq!(meta.title.as_deref(), Some("Named desktop session"));
        assert_eq!(meta.session_id, "aaaa");
        assert_eq!(meta.project_dir, "source-project");
        assert_eq!(meta.cwd.as_deref(), Some("/work/project"));
        assert_eq!(meta.model.as_deref(), Some("mode-original"));
        assert_eq!(meta.source_files, vec![path.clone()]);
        let detail = load_session(dir.path(), Some(&db), "source-project", "aaaa").unwrap();
        assert_eq!(detail.meta.title, meta.title);
        assert_eq!(std::fs::read(&db).unwrap(), before);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), transcript);
    }

    #[test]
    fn catalog_and_detail_use_background_names_without_changing_original_model_ids() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("projects/-tmp-demo");
        std::fs::create_dir_all(&project).unwrap();
        let logs = dir.path().join("logs/runs/test");
        std::fs::create_dir_all(&logs).unwrap();
        let cases = [
            ("aaaa", "mode-first", "Qwen3.8-Max-DogFooding"),
            ("bbbb", "mode-second", "Another model"),
        ];
        let mut records = String::new();
        for (session, model, name) in cases {
            std::fs::write(project.join(format!("{session}.jsonl")), format!(
                "{{\"type\":\"user\",\"entrypoint\":\"cli\",\"message\":{{\"content\":\"hello\"}}}}\n{{\"type\":\"assistant\",\"message\":{{\"model\":\"{model}\",\"content\":\"answer\"}}}}\n"
            )).unwrap();
            records.push_str(&format!("2026-10-15T16:01:14.710+08:00 INFO  debug.message [QoderInferRequest details] model_config={{\"key\":\"{model}\",\"display_name\":\"{name}\"}}, custom_model=null\n"));
        }
        std::fs::write(logs.join("qodercli.log"), records).unwrap();
        let catalog = load_catalog(dir.path(), None, false, &|| false).unwrap();
        assert_eq!(catalog.sessions.len(), cases.len());
        assert!(catalog
            .sessions
            .iter()
            .all(|session| session.model_name.is_none()));
        let ids = cases
            .iter()
            .map(|(_, id, _)| id.to_string())
            .collect::<Vec<_>>();
        let names = models::resolve_with(dir.path(), &ids, std::collections::HashMap::new);
        assert_eq!(names.len(), cases.len());
        let catalog = load_catalog(dir.path(), None, false, &|| false).unwrap();
        for (id, model, name) in cases {
            let meta = catalog
                .sessions
                .iter()
                .find(|s| s.session_id == id)
                .unwrap();
            let detail = load_session(dir.path(), None, "-tmp-demo", id).unwrap();
            assert_eq!(meta.model.as_deref(), Some(model));
            assert_eq!(meta.model_name.as_deref(), Some(name));
            assert_eq!(detail.meta.model, meta.model);
            assert_eq!(detail.meta.model_name, meta.model_name);
        }
    }
}
