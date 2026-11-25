//! Pi 会话读取。v1 日志按行排列，从 v2 起沿最后一个节点的 parentId 读取当前分支。
//! 数据目录由 PI_CODING_AGENT_DIR 指定，默认使用 ~/.pi/agent/sessions 下的会话。

mod events;
mod storage;
mod tools;
mod transcript;
#[cfg(test)]
mod context_tests;

use serde_json::Value;
#[cfg(test)]
use std::fs;
use std::path::{Path, PathBuf};

use super::deletion::{Outcome, PreserveUsage, TrashPlan};
use super::{AgentSource, ConfigFile, SessionDetail, SessionMeta};
use storage::ProjectSessions;
use transcript::parse;
#[cfg(test)]
use transcript::ParsedPi;

const SUMMARY_CACHE_KIND: &str = "pi-transcript-v4";

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
    let project = ProjectSessions::open(root, project_dir, session_id)?;
    let location = project.locate(session_id)?;
    let parent_ids = project.parent_file(location).map(storage::entry_ids).transpose()?.unwrap_or_default();
    let (parsed, warnings) = parse(&location.path, project_dir, true, &parent_ids).split();
    let mut parsed = parsed.ok_or_else(|| warnings.join("；"))?;
    if parsed.meta.session_id != session_id || parsed.meta.project_dir != project_dir {
        return Err("Pi 会话来源已变化，请刷新后重试".into());
    }
    parsed.meta.subagent = project.subagent(location);
    Ok(SessionDetail { warnings, meta: parsed.meta, events: parsed.events, inherited: parsed.inherited })
}

fn trash_plan_in(root: &Path, project_dir: &str, session_id: &str) -> Result<TrashPlan, String> {
    ProjectSessions::open(root, project_dir, session_id)?.trash_plan(session_id)
}

pub(crate) fn transcript_session_id(path: &Path) -> Result<String, String> {
    let (header, _, _) = storage::open(path)?;
    storage::session_id(&header, path)
        .map(str::to_owned)
        .ok_or_else(|| format!("Pi 会话 ID 无效：{}", path.display()))
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
        super::AgentWatchPaths::transcripts("sessions", None)
    }
    fn scan_catalog(
        &self,
        filter: crate::agents::CatalogFilter,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<super::Catalog, String> {
        let mut warnings = Vec::new();
        let mut items = Vec::new();
        for (project_dir, project_path) in storage::projects(&sessions_dir(), &mut warnings) {
            let project = ProjectSessions::scan(&project_dir, project_path, cancelled)?;
            warnings.extend(project.warnings().cloned());
            for location in project.files.values() {
                super::check_catalog_cancelled(cancelled)?;
                let path = &location.path;
                let result = super::catalog_cache::summary(SUMMARY_CACHE_KIND, std::slice::from_ref(path), || {
                    let (parsed, warnings) = parse(path, &project_dir, false, &Default::default()).split();
                    let parsed = parsed.ok_or_else(|| warnings.join("；"))?;
                    Ok((parsed.meta, warnings))
                });
                let (mut meta, ws) = match result {
                    Ok(result) => result,
                    Err(error) => {
                        warnings.push(error);
                        continue;
                    }
                };
                warnings.extend(ws);
                if meta.session_id != location.id {
                    continue;
                }
                if let Err(error) = project.locate(&location.id) {
                    warnings.push(error);
                    continue;
                }
                meta.project_dir.clone_from(&project_dir);
                meta.subagent = project.subagent(location);
                // fork 子会话的继承前缀取决于父会话日志，搜索缓存随它一同失效。
                meta.source_files.extend(project.parent_file(location).map(Path::to_path_buf));
                if !filter.include_empty && meta.is_empty() {
                    continue;
                }
                let display = meta
                    .cwd
                    .clone()
                    .filter(|d| !d.is_empty())
                    .unwrap_or_else(|| "(未知目录)".to_string());
                items.push((meta, display));
            }
        }
        let mut catalog = super::assemble_catalog(items);
        catalog.warnings = warnings;
        Ok(catalog)
    }
    fn read_session(&self, project_dir: &str, session_id: &str) -> Result<SessionDetail, String> {
        read_session_in(&sessions_dir(), project_dir, session_id)
    }
    fn search_events(&self, meta: &SessionMeta) -> Result<Vec<Value>, String> {
        let [path, parent @ ..] = meta.source_files.as_slice() else {
            return Err("缺少会话文件定位信息".into());
        };
        let parent_ids = parent.first().map(|parent| storage::entry_ids(parent)).transpose()?.unwrap_or_default();
        let (parsed, _) = parse(path, &meta.project_dir, true, &parent_ids).split();
        let mut p = parsed.ok_or("无法解析该 Pi 会话")?;
        if p.meta.session_id != meta.session_id || p.meta.project_dir != meta.project_dir {
            return Err("会话来源已变化，请刷新后重试".into());
        }
        Ok(p.events.split_off(p.inherited))
    }
    fn delete_sessions(
        &self,
        sessions: &[super::search::SessionRef],
        preserve: &mut dyn PreserveUsage,
        outcome: &mut Outcome<'_>,
    ) -> Result<(), String> {
        let root = sessions_dir();
        super::deletion::trash(sessions, |session| {
            trash_plan_in(&root, &session.project_dir, &session.session_id)
        }, preserve, outcome)
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
        let (parsed, _) = parse(path, "test", true, &Default::default()).split();
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

        let err = trash_plan_in(tmp.path(), "proj", "fallback123").unwrap_err();
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

        let targets = trash_plan_in(tmp.path(), "proj", "good123").unwrap().sessions;
        assert_eq!(targets, [("good123".to_owned(), file.canonicalize().unwrap())], "内嵌 id 的会话应可正常删除");
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
        mk("projB", "项目B的问题");
        let proj_a = "projA";
        let proj_b = "projB";

        // 读取按项目定位：各自拿到自己项目的内容
        let da = read_session_in(tmp.path(), &proj_a, "dup").unwrap();
        assert_eq!(da.events[0]["message"]["content"], "项目A的问题");
        let db = read_session_in(tmp.path(), &proj_b, "dup").unwrap();
        assert_eq!(db.events[0]["message"]["content"], "项目B的问题");

        // 删除按项目定位：删 A 只动 A，不得误删项目 B 的同名会话
        let targets = trash_plan_in(tmp.path(), &proj_a, "dup").unwrap().sessions;
        assert_eq!(targets, [("dup".to_owned(), file_a.canonicalize().unwrap())]);
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
        let targets = trash_plan_in(tmp.path(), "proj", "safe").unwrap().sessions;
        assert_eq!(targets, [("safe".to_owned(), project.join("one_safe.jsonl").canonicalize().unwrap())]);
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
        let delete_error = trash_plan_in(tmp.path(), "proj", "dup").err();

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

        let detail = read_session_in(tmp.path(), "proj", "safe").unwrap();
        assert_eq!(detail.events[0]["message"]["content"], "hello");
        let err = trash_plan_in(tmp.path(), "proj", "safe").unwrap_err();
        assert!(err.contains("无法读取"), "扫描不完整时不得删除任何文件：{err}");
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

        let err = trash_plan_in(tmp.path(), "proj", "corrupt1").unwrap_err();
        assert!(err.contains("无法解析"), "{err}");
        assert!(file.exists(), "部分损坏的会话不得删除");
    }

    #[test]
    fn nested_extension_sessions_resolve_by_header_and_cascade_without_user_forks() {
        use std::io::Write;
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("project");
        let parent = project.join("2026-11-23_parent.jsonl");
        let child = project.join("2026-11-23_parent/run-id/run-0/session.jsonl");
        let leaf = child.with_extension("").join("forks/next/2026-11-23_leaf.jsonl");
        let user_fork = project.join("2026-11-23_user-fork.jsonl");
        let custom = project.join("custom/run-id/run-0/session.jsonl");
        let (parent, child, leaf, user_fork, custom) = (&parent, &child, &leaf, &user_fork, &custom);

        let write = |path: &Path, id, parent: Option<&Path>| {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, format!("{}\n{}\n",
                serde_json::json!({"type":"session","version":3,"id":id,"parentSession":parent}),
                serde_json::json!({"type":"message","id":"task","parentId":null,"message":{"role":"user","content":"Task"}}),
            )).unwrap();
        };
        write(parent, "parent", None);
        write(child, "child", Some(parent));
        write(leaf, "leaf", Some(child));
        write(user_fork, "user-fork", Some(parent));
        write(custom, "custom", Some(parent));
        let mut own = fs::OpenOptions::new().append(true).open(leaf).unwrap();
        writeln!(own, "{}", serde_json::json!({"type":"message","id":"own","parentId":"task","message":{"role":"user","content":"Own"}})).unwrap();

        let detail = read_session_in(tmp.path(), "project", "leaf").unwrap();
        assert_eq!((detail.events.len(), detail.inherited), (2, 1), "forked children reuse the parent's entry IDs");
        let relation = detail.meta.subagent.unwrap();
        assert_eq!(relation.parent_id.as_deref(), Some("child"));
        assert_eq!(relation.project_owner.unwrap().session_id, "parent");
        assert!(!relation.orphan);
        let user_fork = read_session_in(tmp.path(), "project", "user-fork").unwrap();
        assert!(user_fork.meta.subagent.is_none() && user_fork.inherited == 0);
        let custom_relation = read_session_in(tmp.path(), "project", "custom").unwrap().meta.subagent.unwrap();
        assert_eq!((custom_relation.parent_id.as_deref(), custom_relation.orphan), (Some("parent"), false), "custom session directories keep the recorded parent");
        let targets = trash_plan_in(tmp.path(), "project", "parent").unwrap().sessions;
        assert_eq!(targets, [("parent", parent), ("child", child), ("leaf", leaf), ("custom", custom)].map(|(id, path)| (id.to_owned(), path.canonicalize().unwrap())));
        fs::remove_file(parent).unwrap();
        let orphan = read_session_in(tmp.path(), "project", "child").unwrap().meta.subagent.unwrap();
        assert!(orphan.orphan);
        assert_eq!(orphan.parent_id.as_deref(), Some("parent"));
        assert_eq!(read_session_in(tmp.path(), "project", "leaf").unwrap().events.len(), 2);
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
            r#"{"type":"session_info","id":"rename","parentId":"tool","timestamp":"2026-09-10T09:00:00Z","name":"改名后"}"#,
            r#"{"type":"label","id":"tag","parentId":"rename","timestamp":"2026-09-10T09:01:00Z","label":"important"}"#,
            r#"{"type":"usage","id":"u1","parentId":"tag","timestamp":"2026-09-10T09:02:00Z"}"#,
            r#"{"type":"custom","id":"ext","parentId":"u1","timestamp":"2026-09-10T09:03:00Z","customType":"plugin","data":{}}"#,
        ];
        for line in lines {
            writeln!(f, "{line}").unwrap();
        }

        let summary = parse(f.path(), "test", false, &Default::default()).split().0.unwrap();
        let p = session_of(f.path());
        assert_eq!(p.meta.session_id, "pi-001");
        assert_eq!(p.meta.cwd.as_deref(), Some("/tmp/pi-proj"));
        assert_eq!(p.meta.title.as_deref(), Some("改名后"));
        assert_eq!(p.meta.real_user_count, 2);
        assert_eq!(p.meta.assistant_text_count, 1);
        assert_eq!(p.meta.model.as_deref(), Some("actual"));
        assert_eq!(p.meta.first_ts.as_deref(), Some("2026-09-03T10:00:00Z"));
        assert_eq!(p.meta.last_ts.as_deref(), Some("2026-09-03T10:00:01.654Z"));
        assert_eq!(summary.meta.last_ts, p.meta.last_ts);
        assert_eq!(summary.meta.title, p.meta.title);
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
