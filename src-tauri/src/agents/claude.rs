//! Claude Code 数据源：~/.claude/projects 下的 JSONL 会话。
//! 与 QoderCLI 同源格式，但没有 ai-title/custom-title/last-prompt 事件，
//! 标题回退到首条真实用户消息。

use super::claude_family::{self, FamilyOptions};
use super::{shell_quote, AgentSource, Catalog, ConfigFile, SessionDetail};
use std::path::PathBuf;

const OPTS: FamilyOptions = FamilyOptions {
    title_events: false,
    filter_non_cli_entrypoint: false,
    filter_qoderwork_cwd: false,
};

fn claude_dir() -> PathBuf {
    if let Ok(v) = std::env::var("CLAUDE_CONFIG_DIR") {
        if !v.is_empty() {
            return super::expand_tilde(&v);
        }
    }
    dirs::home_dir().unwrap_or_default().join(".claude")
}

fn projects_dir() -> PathBuf {
    claude_dir().join("projects")
}

fn resolve_claude_bin() -> String {
    let local = claude_dir().join("local").join("claude");
    if local.exists() {
        local.to_string_lossy().to_string()
    } else {
        "claude".to_string()
    }
}

pub struct ClaudeSource;

impl AgentSource for ClaudeSource {
    fn id(&self) -> &'static str {
        "claude"
    }
    fn display_name(&self) -> &'static str {
        "Claude Code"
    }
    fn is_installed(&self) -> bool {
        projects_dir().exists()
    }
    fn data_dir(&self) -> Option<PathBuf> {
        Some(claude_dir())
    }
    fn watch_paths(&self, usage: bool) -> super::AgentWatchPaths {
        super::AgentWatchPaths::transcripts("projects", if usage { None } else { Some(2) })
    }
    fn catalog(&self, include_empty: bool) -> Result<Catalog, String> {
        claude_family::catalog(&projects_dir(), OPTS, include_empty)
    }
    fn read_session(&self, project_dir: &str, session_id: &str) -> Result<SessionDetail, String> {
        claude_family::read_session(&projects_dir(), project_dir, session_id, OPTS)
    }
    fn delete_session(&self, project_dir: &str, session_id: &str) -> Result<(), String> {
        claude_family::delete_session(&projects_dir(), project_dir, session_id)
    }
    fn resume_command(&self, session_id: &str, _fork: bool) -> Option<String> {
        Some(format!(
            "{} --resume {}",
            shell_quote(&resolve_claude_bin()),
            shell_quote(session_id)
        ))
    }
    fn config_spec(&self) -> Vec<ConfigFile> {
        vec![ConfigFile {
            key: "settings.json".into(),
            label: "settings.json".into(),
            path: claude_dir().join("settings.json"),
        }]
    }
}
