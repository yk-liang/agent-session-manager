//! Pi 会话读取。v1 日志按行排列，从 v2 起沿最后一个节点的 parentId 读取当前分支。
//! 数据目录由 PI_CODING_AGENT_DIR 指定，默认使用 ~/.pi/agent/sessions 下的会话。

mod events;
mod storage;
mod transcript;
#[cfg(test)]
mod context_tests;

use serde_json::Value;
#[cfg(test)]
use std::fs;
use std::path::{Path, PathBuf};

use super::{AgentSource, ConfigFile, SessionDetail, SessionMeta};
use transcript::parse;
#[cfg(test)]
use transcript::ParsedPi;

fn pi_home() -> PathBuf {
    if let Ok(v) = std::env::var("PI_CODING_AGENT_DIR") {
        if !v.is_empty() {
            return super::expand_tilde(&v);
        }
    }
    dirs::home_dir()
        .unwrap_or_default()
        .join(".pi")
        .join("agent")
}

fn sessions_dir() -> PathBuf {
    pi_home().join("sessions")
}

#[cfg(test)]
use storage::is_valid_session_id;

fn read_session_in(root: &Path, project_dir: &str, session_id: &str) -> Result<SessionDetail, String> {
    let location = storage::locate(root, project_dir, session_id)?;
    let (parsed, warnings) = parse(&location.path, true).split();
    let parsed = parsed.ok_or_else(|| warnings.join("；"))?;
    if parsed.meta.session_id != session_id || parsed.meta.project_dir != project_dir {
        return Err("Pi 会话来源已变化，请刷新后重试".into());
    }
    Ok(SessionDetail { warnings, meta: parsed.meta, events: parsed.events })
}

fn delete_session_with(
    root: &Path,
    project_dir: &str,
    session_id: &str,
    remove: impl FnOnce(PathBuf) -> Result<(), String>,
) -> Result<(), String> {
    remove(storage::deletion_target(root, project_dir, session_id)?)
}

fn delete_session_in(root: &Path, project_dir: &str, session_id: &str) -> Result<(), String> {
    delete_session_with(root, project_dir, session_id, |target| {
        trash::delete_all([target]).map_err(|error| format!("移入废纸篓失败：{error}"))
    })
}

pub struct PiSource;

impl AgentSource for PiSource {
    fn id(&self) -> &'static str {
        "pi"
    }
    fn display_name(&self) -> &'static str {
        "Pi"
    }
    fn is_installed(&self) -> bool {
        sessions_dir().exists()
    }
    fn data_dir(&self) -> Option<PathBuf> {
        Some(pi_home())
    }
    fn watch_paths(&self) -> super::AgentWatchPaths {
        super::AgentWatchPaths::transcripts("sessions", Some(2))
    }
    fn scan_catalog(
        &self,
        filter: crate::agents::CatalogFilter,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<super::Catalog, String> {
        let mut warnings = Vec::new();
        let mut items = Vec::new();
        for path in storage::session_files(&sessions_dir(), &mut warnings) {
            super::check_catalog_cancelled(cancelled)?;
            let result = super::catalog_cache::summary("pi-transcript-v3", std::slice::from_ref(&path), || {
                let (parsed, warnings) = parse(&path, false).split();
                let parsed = parsed.ok_or_else(|| warnings.join("；"))?;
                Ok((parsed.meta, warnings))
            });
            let (meta, ws) = match result {
                Ok(result) => result,
                Err(error) => {
                    warnings.push(error);
                    continue;
                }
            };
            warnings.extend(ws);
            if !filter.include_empty && meta.real_user_count == 0 {
                continue;
            }
            let display = meta
                .cwd
                .clone()
                .filter(|d| !d.is_empty())
                .unwrap_or_else(|| "(未知目录)".to_string());
            items.push((meta, display));
        }
        let mut catalog = super::assemble_catalog(items);
        catalog.warnings = warnings;
        Ok(catalog)
    }
    fn read_session(&self, project_dir: &str, session_id: &str) -> Result<SessionDetail, String> {
        read_session_in(&sessions_dir(), project_dir, session_id)
    }
    fn search_events(&self, meta: &SessionMeta) -> Result<Vec<Value>, String> {
        let path = meta.source_files.first().ok_or("缺少会话文件定位信息")?;
        let (parsed, _) = parse(path, true).split();
        let p = parsed.ok_or("无法解析该 Pi 会话")?;
        if p.meta.session_id != meta.session_id || p.meta.project_dir != meta.project_dir {
            return Err("会话来源已变化，请刷新后重试".into());
        }
        Ok(p.events)
    }
    fn delete_session(&self, project_dir: &str, session_id: &str) -> Result<(), String> {
        delete_session_in(&sessions_dir(), project_dir, session_id)
    }
    fn deletion_targets(&self, project_dir: &str, session_id: &str) -> Result<Option<Vec<PathBuf>>, String> {
        let path = storage::deletion_target(&sessions_dir(), project_dir, session_id)?;
        Ok(Some(vec![path]))
    }
    fn resume_command(&self, session_id: &str, _fork: bool) -> Option<String> {
        Some(format!(
            "{} --session-id {}",
            super::shell_quote("pi"),
            super::shell_quote(session_id)
        ))
    }
    fn config_spec(&self) -> Vec<ConfigFile> {
        vec![ConfigFile {
            key: "settings.json".into(),
            label: "settings.json".into(),
            path: pi_home().join("settings.json"),
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session_of(path: &Path) -> ParsedPi {
        let (parsed, _) = parse(path, true).split();
        match parsed {
            Some(p) => p,
            None => panic!("解析失败"),
        }
    }

    #[test]
    fn session_id_validation() {
        assert!(is_valid_session_id("abc-123_def.456"));
        assert!(!is_valid_session_id(""));
        assert!(!is_valid_session_id("a/b"));
        assert!(!is_valid_session_id("-abc"));
        assert!(!is_valid_session_id("abc-"));
    }

    #[test]
    fn config_spec_uses_official_settings_file() {
        let spec = PiSource.config_spec();
        assert_eq!(spec.len(), 1);
        assert_eq!(spec[0].key, "settings.json");
        assert_eq!(spec[0].path, pi_home().join("settings.json"));
    }

    #[test]
    fn search_uses_the_same_active_branch_and_event_order_as_detail() {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("proj");
        fs::create_dir_all(&project).unwrap();
        let path = project.join("session-good123.jsonl");
        fs::write(&path, [
            r#"{"type":"session","id":"good123","version":2,"cwd":"/tmp/demo"}"#,
            r#"{"type":"message","id":"u","message":{"role":"user","content":"question"}}"#,
            r#"{"type":"message","id":"discarded","parentId":"u","message":{"role":"assistant","content":"discarded branch"}}"#,
            r#"{"type":"message","id":"active","parentId":"u","message":{"role":"assistant","content":"active branch"}}"#,
        ].join("\n")).unwrap();
        let detail = read_session_in(root.path(), "proj", "good123").unwrap();
        let meta = detail.meta;
        assert_eq!(meta.source_files, vec![path.canonicalize().unwrap()]);
        let events = PiSource.search_events(&meta).unwrap();
        assert_eq!(events, detail.events);
        assert!(!events
            .iter()
            .any(|event| event.to_string().contains("discarded branch")));
    }

    #[test]
    fn delete_refuses_filename_derived_id() {
        let tmp = tempfile::tempdir().unwrap();
        let proj = tmp.path().join("proj");
        fs::create_dir_all(&proj).unwrap();
        // header 无 id：解析会按文件名兜底出 "fallback123"，
        // 但删除绝不能信任这个推导结果
        let file = proj.join("1700000000_fallback123.jsonl");
        fs::write(
            &file,
            [
                r#"{"type":"session","timestamp":"2026-01-01T00:00:00Z","version":1}"#,
                r#"{"type":"message","message":{"role":"user","content":"hello"}}"#,
            ]
            .join("\n"),
        )
        .unwrap();

        let err = delete_session_in(tmp.path(), "proj", "fallback123").unwrap_err();
        assert!(err.contains("内嵌 id"), "{err}");
        assert!(file.exists(), "不得基于文件名推导执行删除");
    }

    #[test]
    fn delete_allows_header_embedded_id() {
        let tmp = tempfile::tempdir().unwrap();
        let proj = tmp.path().join("proj");
        fs::create_dir_all(&proj).unwrap();
        let file = proj.join("1700000000_good123.jsonl");
        fs::write(
            &file,
            [
                r#"{"type":"session","id":"good123","timestamp":"2026-01-01T00:00:00Z","version":1}"#,
                r#"{"type":"message","message":{"role":"user","content":"hello"}}"#,
            ]
            .join("\n"),
        )
        .unwrap();

        delete_session_with(tmp.path(), "proj", "good123", |p| {
            fs::remove_file(p).map_err(|e| e.to_string())
        })
        .unwrap();
        assert!(!file.exists(), "内嵌 id 的会话应可正常删除");
    }

    #[test]
    fn read_and_delete_are_scoped_by_project() {
        // 两个项目下各有一个 id 相同（same-id）的会话：读取/删除必须按项目定位，
        // 绝不能命中另一个项目里的同名会话
        let tmp = tempfile::tempdir().unwrap();
        let mk = |proj: &str, body: &str| {
            let dir = tmp.path().join(proj);
            fs::create_dir_all(&dir).unwrap();
            let file = dir.join("1700000000_dup.jsonl");
            fs::write(
                &file,
                [
                    r#"{"type":"session","id":"dup","version":2}"#.to_string(),
                    format!(
                        r#"{{"type":"message","id":"m1","parentId":"","message":{{"role":"user","content":"{body}"}}}}"#
                    ),
                ]
                .join("\n"),
            )
            .unwrap();
            file
        };
        let file_a = mk("projA", "项目A的问题");
        let file_b = mk("projB", "项目B的问题");
        let proj_a = "projA";
        let proj_b = "projB";

        // 读取按项目定位：各自拿到自己项目的内容
        let da = read_session_in(tmp.path(), &proj_a, "dup").unwrap();
        assert_eq!(da.events[0]["message"]["content"], "项目A的问题");
        let db = read_session_in(tmp.path(), &proj_b, "dup").unwrap();
        assert_eq!(db.events[0]["message"]["content"], "项目B的问题");

        // 删除按项目定位：删 A 只动 A，B 完好
        delete_session_with(tmp.path(), &proj_a, "dup", |p| {
            fs::remove_file(p).map_err(|e| e.to_string())
        })
        .unwrap();
        assert!(!file_a.exists(), "应删除项目 A 的会话");
        assert!(file_b.exists(), "不得误删项目 B 的同名会话");
    }

    #[test]
    fn read_healthy_session_with_unrelated_corrupt_file() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("proj");
        fs::create_dir(&project).unwrap();
        fs::write(
            project.join("one_safe.jsonl"),
            [
                r#"{"type":"session","id":"safe","version":1}"#,
                r#"{"type":"message","message":{"role":"user","content":"完整会话"}}"#,
            ]
            .join("\n"),
        )
        .unwrap();
        fs::write(
            project.join("other_corrupt.jsonl"),
            [
                r#"{"type":"session","id":"other","version":1}"#,
                "{truncated",
            ]
            .join("\n"),
        )
        .unwrap();

        let detail = read_session_in(tmp.path(), "proj", "safe").unwrap();
        assert_eq!(detail.events[0]["message"]["content"], "完整会话");
        delete_session_with(tmp.path(), "proj", "safe", |path| {
            fs::remove_file(path).map_err(|error| error.to_string())
        }).unwrap();
        assert!(!project.join("one_safe.jsonl").exists());
        assert!(project.join("other_corrupt.jsonl").is_file());
    }

    #[test]
    fn read_degraded_session_keeps_valid_messages() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("proj");
        fs::create_dir(&project).unwrap();
        fs::write(
            project.join("one_partial.jsonl"),
            [
                r#"{"type":"session","id":"partial","version":1}"#,
                r#"{"type":"message","message":{"role":"user","content":"保留的问题"}}"#,
                r#"{"type":"message","message":{"role":"assistant","content":"保留的回答"}}"#,
                "{truncated",
            ]
            .join("\n"),
        )
        .unwrap();

        let detail = read_session_in(tmp.path(), "proj", "partial").unwrap();
        assert_eq!(detail.events.len(), 2);
        assert_eq!(detail.warnings.len(), 1);
        assert_eq!(detail.events[0]["message"]["content"], "保留的问题");
        assert_eq!(detail.events[1]["message"]["content"], "保留的回答");
    }

    #[test]
    fn read_refuses_duplicate_ids_even_when_one_is_degraded() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("proj");
        fs::create_dir(&project).unwrap();
        let content = [
            r#"{"type":"session","id":"dup","version":1}"#,
            r#"{"type":"message","message":{"role":"user","content":"同 ID 会话"}}"#,
        ]
        .join("\n");
        fs::write(project.join("one_dup.jsonl"), &content).unwrap();
        fs::write(
            project.join("two_dup.jsonl"),
            format!("{content}\n{{truncated"),
        )
        .unwrap();

        let error = read_session_in(tmp.path(), "proj", "dup").err().unwrap();
        assert!(
            error.contains("多个"),
            "损坏副本也必须参与歧义检查：{error}"
        );
    }

    #[test]
    fn duplicate_ids_within_project_are_rejected_for_read_and_delete() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("proj");
        fs::create_dir_all(&project).unwrap();
        for (name, body) in [("one_dup.jsonl", "第一份"), ("two_dup.jsonl", "第二份")] {
            fs::write(
                project.join(name),
                [
                    r#"{"type":"session","id":"dup","version":1}"#.to_string(),
                    format!(
                        r#"{{"type":"message","message":{{"role":"user","content":"{body}"}}}}"#
                    ),
                ]
                .join("\n"),
            )
            .unwrap();
        }

        let read_error = read_session_in(tmp.path(), "proj", "dup").err();
        let removed = std::cell::Cell::new(false);
        let delete_error = delete_session_with(tmp.path(), "proj", "dup", |_| {
            removed.set(true);
            Ok(())
        })
        .err();

        assert!(
            read_error
                .as_deref()
                .is_some_and(|error| error.contains("多个")),
            "读取必须拒绝歧义：{read_error:?}"
        );
        assert!(
            delete_error
                .as_deref()
                .is_some_and(|error| error.contains("多个")),
            "删除必须拒绝歧义：{delete_error:?}"
        );
        assert!(!removed.get(), "歧义时不得执行删除");
    }

    #[test]
    fn delete_refuses_unreadable_candidate_with_matching_filename() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("proj");
        fs::create_dir_all(&project).unwrap();
        fs::write(
            project.join("one_safe.jsonl"),
            [
                r#"{"type":"session","id":"safe","version":1}"#,
                r#"{"type":"message","message":{"role":"user","content":"hello"}}"#,
            ]
            .join("\n"),
        )
        .unwrap();
        let fifo = project.join("two_safe.jsonl");
        let fifo_c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo_c.as_ptr(), 0o600) }, 0);
        let removed = std::cell::Cell::new(false);

        let detail = read_session_in(tmp.path(), "proj", "safe").unwrap();
        assert_eq!(detail.events[0]["message"]["content"], "hello");
        let err = delete_session_with(tmp.path(), "proj", "safe", |_| {
            removed.set(true);
            Ok(())
        })
        .unwrap_err();
        assert!(err.contains("无法读取"), "{err}");
        assert!(!removed.get(), "扫描不完整时不得删除任何文件");
    }

    #[test]
    fn delete_refuses_partially_corrupt_session() {
        let tmp = tempfile::tempdir().unwrap();
        let proj = tmp.path().join("proj");
        fs::create_dir_all(&proj).unwrap();
        // header 身份正确，但后续含损坏行：宽松解析会判 Degraded 并匹配，
        // 严格验证器必须逐行扫描后拒绝，不能只凭 header 放行
        let file = proj.join("1700000000_corrupt1.jsonl");
        fs::write(
            &file,
            [
                r#"{"type":"session","id":"corrupt1","timestamp":"2026-01-01T00:00:00Z","version":1}"#,
                r#"{"type":"message","message":{"role":"user","content":"hello"}}"#,
                "这一行不是合法 JSON",
            ]
            .join("\n"),
        )
        .unwrap();

        let err = delete_session_in(tmp.path(), "proj", "corrupt1").unwrap_err();
        assert!(err.contains("无法解析"), "{err}");
        assert!(file.exists(), "部分损坏的会话不得删除");
    }

    #[test]
    fn parses_v2_tree_and_follows_leaf_branch() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        use std::io::Write;
        let lines = [
            r#"{"type":"session","version":2,"id":"pi-001","timestamp":"2026-09-03T10:00:00Z","cwd":"/tmp/pi-proj"}"#,
            // 旧分支（被 /fork 废弃）
            r#"{"type":"message","id":"e1","parentId":"","message":{"role":"user","content":"旧问题"}}"#,
            r#"{"type":"message","id":"e2","parentId":"e1","message":{"role":"assistant","model":"old","content":[{"type":"text","text":"旧回答"}]}}"#,
            // 活跃分支
            r#"{"type":"message","id":"e3","parentId":"e1","message":{"role":"user","content":"新问题"}}"#,
            r#"{"type":"session_info","id":"e4","parentId":"e3","name":"我的命名"}"#,
            r#"{"type":"session_info","id":"old_name","parentId":"e2","name":"废弃名称"}"#,
            r#"{"type":"model_change","id":"model","parentId":"e4","modelId":"selected"}"#,
            r#"{"type":"message","id":"e5","parentId":"model","timestamp":"2026-09-03T10:00:01.321Z","message":{"role":"assistant","model":"actual","content":[{"type":"thinking","thinking":"思考"},{"type":"text","text":"新回答"},{"type":"toolCall","id":"call","name":"check","arguments":{"path":"."}}]}}"#,
            r#"{"type":"message","id":"tool","parentId":"e5","timestamp":"2026-09-03T10:00:01.654Z","message":{"role":"toolResult","toolCallId":"call","content":[{"type":"image","mimeType":"image/png","data":"AAAA"}]}}"#,
        ];
        for line in lines {
            writeln!(f, "{line}").unwrap();
        }

        let p = session_of(f.path());
        assert_eq!(p.meta.session_id, "pi-001");
        assert_eq!(p.meta.cwd.as_deref(), Some("/tmp/pi-proj"));
        assert_eq!(p.meta.title.as_deref(), Some("我的命名"));
        assert_eq!(p.meta.real_user_count, 2);
        assert_eq!(p.meta.assistant_text_count, 1);
        assert_eq!(p.meta.model.as_deref(), Some("actual"));
        assert_eq!(p.meta.last_ts.as_deref(), Some("2026-09-03T10:00:01.654Z"));
        assert_eq!(p.events.len(), 4);
        let answer = &p.events[2]["message"];
        assert_eq!(answer["model"], "actual");
        assert_eq!(answer["content"][0]["type"], "thinking");
        assert_eq!(answer["content"][1]["text"], "新回答");
        assert_eq!(answer["content"][2]["type"], "tool_use");
        let result = &p.events[3]["message"]["content"][0];
        assert_eq!(result["tool_use_id"], "call");
        assert_eq!(result["content"][0]["source"]["media_type"], "image/png");
    }

    #[test]
    fn parses_v1_linear() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        use std::io::Write;
        let lines = [
            r#"{"type":"session","version":1,"id":"pi-old","timestamp":"2026-09-01T08:00:00Z","cwd":"/tmp/old"}"#,
            r#"{"type":"message","message":{"role":"user","content":"线性消息"}}"#,
            r#"{"type":"message","message":{"role":"assistant","content":"线性回复"}}"#,
        ];
        for line in lines {
            writeln!(f, "{line}").unwrap();
        }
        let p = session_of(f.path());
        assert_eq!(p.events.len(), 2);
        assert_eq!(p.meta.real_user_count, 1);
        assert_eq!(p.meta.title.as_deref(), Some("线性消息"));
    }
}
