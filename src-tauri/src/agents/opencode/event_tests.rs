use super::*;
use rusqlite::OptionalExtension;

fn fixture() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("opencode.db");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch("CREATE TABLE session (id TEXT PRIMARY KEY,title TEXT,directory TEXT,time_created INTEGER,time_updated INTEGER,parent_id TEXT,time_archived INTEGER);
        CREATE TABLE message (id TEXT PRIMARY KEY,session_id TEXT,time_created INTEGER,data TEXT);
        CREATE TABLE part (id TEXT PRIMARY KEY,message_id TEXT,time_created INTEGER,data TEXT);
        INSERT INTO session VALUES ('ses_fixture','Ordered conversation','/tmp/oc',1000000,2000000,NULL,NULL);
        INSERT INTO message VALUES ('m_user','ses_fixture',1000000,'{\"role\":\"user\"}');
        INSERT INTO message VALUES ('m_assistant','ses_fixture',1000001,'{\"role\":\"assistant\",\"providerID\":\"fixture\",\"modelID\":\"model-a\"}');").unwrap();
    // 插入顺序故意打乱；同毫秒的 part 按其单调 ID 还原顺序。
    for (id, msg, value) in [
        (
            "p06",
            "m_assistant",
            json!({"type":"text","text":"Final answer with searchable needle"}),
        ),
        (
            "p01",
            "m_user",
            json!({"type":"text","text":"First request"}),
        ),
        (
            "p02",
            "m_user",
            json!({"type":"text","text":"continued request"}),
        ),
        (
            "p04",
            "m_assistant",
            json!({"type":"text","text":"Before tool"}),
        ),
        (
            "p03",
            "m_assistant",
            json!({"type":"reasoning","text":"Stored reasoning"}),
        ),
        (
            "p05",
            "m_assistant",
            json!({"type":"tool","tool":"read","callID":"call_1","state":{"status":"completed","input":{"path":"/tmp/example"},"output":"Tool output"}}),
        ),
        (
            "p07",
            "m_assistant",
            json!({"type":"text","text":"Hidden synthetic text","synthetic":true}),
        ),
    ] {
        conn.execute(
            "INSERT INTO part VALUES (?1,?2,1000001,?3)",
            rusqlite::params![id, msg, value.to_string()],
        )
        .unwrap();
    }
    (dir, path)
}

#[test]
fn context_visibility_opencode_legacy_synthetic_and_summaries() {
    let (_dir, path) = fixture();
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch("UPDATE message SET data='{\"role\":\"user\",\"system\":\"context-secret custom system prompt\"}' WHERE id='m_user';
        INSERT INTO part VALUES ('p00','m_user',1000000,'{\"type\":\"text\",\"text\":\"context-secret injected prompt\",\"synthetic\":true}');
        INSERT INTO part VALUES ('p08','m_user',1000002,'{\"type\":\"text\",\"text\":\"context-secret ignored prompt\",\"ignored\":true}');
        INSERT INTO message VALUES ('summary','ses_fixture',1000004,'{\"role\":\"assistant\",\"summary\":true}');
        INSERT INTO part VALUES ('summary','summary',1000004,'{\"type\":\"text\",\"text\":\"context-secret compressed history\"}');
        INSERT INTO session VALUES ('context_only','Context only','/tmp/oc',1000000,2000000,NULL,NULL);
        INSERT INTO message VALUES ('context_user','context_only',1000000,'{\"role\":\"user\"}');
        INSERT INTO part VALUES ('context_user','context_user',1000000,'{\"type\":\"text\",\"text\":\"context-secret\",\"synthetic\":true}');
        INSERT INTO message VALUES ('context_assistant','context_only',1000001,'{\"role\":\"assistant\",\"summary\":true}');
        INSERT INTO part VALUES ('context_assistant','context_assistant',1000001,'{\"type\":\"text\",\"text\":\"context-secret\"}');").unwrap();
    let before = std::fs::read(&path).unwrap();
    let catalog = catalog_at(&path, false).unwrap();
    assert_eq!(catalog.sessions.len(), 1);
    let detail = read_session_at(&path, "ses_fixture").unwrap();
    assert_eq!(
        (
            detail.meta.real_user_count,
            detail.meta.assistant_text_count
        ),
        (1, 1)
    );
    assert_eq!(
        detail
            .events
            .iter()
            .filter(|e| e["type"] == "context")
            .count(),
        5
    );
    assert_eq!(
        search_events_at(&path, &detail.meta).unwrap(),
        detail.events
    );
    assert_eq!(catalog_at(&path, true).unwrap().sessions.len(), 2);
    assert_eq!(std::fs::read(&path).unwrap(), before);
    super::super::context_test_support::check_and_export(
        "opencode",
        detail,
        "context-secret",
        "searchable needle",
    );
}

#[test]
fn detail_filters_session_id_in_sql_before_decoding_unrelated_rows() {
    let (_dir, path) = fixture();
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch("INSERT INTO session VALUES ('ses_other','Other','/tmp/other','bad timestamp',2,NULL,NULL);").unwrap();
    let detail = read_session_at(&path, "ses_fixture").unwrap();
    assert!(
        detail.warnings.is_empty(),
        "无关坏行不得污染详情：{:?}",
        detail.warnings
    );

    // 在 SQLite 内部对无关行求值即报错：Rust 侧全表解码后 find 无法通过。
    conn.execute_batch("ALTER TABLE session RENAME TO stored_session;
        CREATE VIEW session AS SELECT id,
            CASE WHEN id='ses_fixture' THEN title ELSE json_extract('invalid JSON','$') END AS title,
            directory,time_created,time_updated,parent_id,time_archived FROM stored_session;").unwrap();
    assert!(conn
        .prepare("SELECT * FROM session")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .any(|row| row.is_err()));
    let filtered = read_session_at(&path, "ses_fixture").unwrap();
    assert!(
        filtered.warnings.is_empty(),
        "SQL 必须先按 ID 过滤：{:?}",
        filtered.warnings
    );
    assert_eq!(filtered.events, detail.events);
    assert_eq!(
        search_events_at(&path, &detail.meta).unwrap(),
        detail.events
    );
    assert!(read_session_at(&path, "ses_missing").is_err());
}

#[test]
fn targeted_session_decoding_keeps_legacy_columns_and_failure_warnings() {
    let (_dir, path) = fixture();
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "ALTER TABLE session RENAME TO stored_session;
        CREATE VIEW session AS SELECT id,directory FROM stored_session;",
    )
    .unwrap();
    let legacy = read_session_at(&path, "ses_fixture").unwrap();
    assert!(legacy.warnings.is_empty());
    assert_eq!(legacy.meta.title.as_deref(), Some("First request"));
    assert_eq!(
        search_events_at(&path, &legacy.meta).unwrap(),
        legacy.events
    );
    conn.execute_batch("DROP VIEW session;
        CREATE VIEW session AS SELECT id,directory,'bad timestamp' AS time_created FROM stored_session;").unwrap();
    let degraded = read_session_at(&path, "ses_fixture").unwrap();
    assert!(degraded.warnings.iter().any(|w| w.contains("读取失败")));
    conn.execute_batch("DROP VIEW session;
        CREATE VIEW session AS SELECT id,json_extract('invalid JSON','$') AS directory FROM stored_session;").unwrap();
    assert!(read_session_at(&path, "ses_fixture").is_err());
}

#[test]
fn stored_reasoning_and_tools_preserve_part_order_and_user_message_grouping() {
    let (_dir, path) = fixture();
    let detail = read_session_at(&path, "ses_fixture").unwrap();
    assert_eq!(
        detail.events.len(),
        7,
        "正文、工具及 synthetic 上下文均保留"
    );
    assert_eq!(detail.events[6]["type"], "context");
    assert_eq!(
        detail.events[0]["message"]["content"],
        "First request\ncontinued request"
    );
    assert_eq!(
        detail.events[1]["message"]["content"][0]["thinking"],
        "Stored reasoning"
    );
    assert_eq!(detail.events[2]["message"]["content"], "Before tool");
    let tool = &detail.events[3]["message"]["content"][0];
    assert_eq!(tool["type"], "tool_use");
    assert_eq!(tool["name"], "read");
    assert_eq!(tool["input"]["path"], "/tmp/example");
    let output = &detail.events[4]["message"]["content"][0];
    assert_eq!(output["type"], "tool_result");
    assert_eq!(output["content"], "Tool output");
    assert_eq!(tool["id"], output["tool_use_id"]);
    assert_eq!(
        detail.events[5]["message"]["content"],
        "Final answer with searchable needle"
    );
    assert_eq!(detail.meta.real_user_count, 1);
    assert_eq!(detail.meta.assistant_text_count, 1);
    assert!(detail.warnings.is_empty());
}

#[test]
fn search_validates_target_and_preserves_events_without_hiding_read_failures() {
    let (_dir, path) = fixture();
    let detail = read_session_at(&path, "ses_fixture").unwrap();
    assert_eq!(
        search_events_at(&path, &detail.meta).unwrap(),
        detail.events
    );
    let mut wrong = detail.meta.clone();
    wrong.project_dir = "-different-project".into();
    assert!(
        search_events_at(&path, &wrong).is_err(),
        "搜索不得读取已变更项目归属的会话"
    );
    wrong.session_id = "ses_fixture' OR 1=1 --".into();
    assert!(search_events_at(&path, &wrong).is_err());
    let conn = Connection::open(&path).unwrap();
    conn.execute("UPDATE part SET data='invalid JSON' WHERE id='p06'", [])
        .unwrap();
    assert!(!read_session_at(&path, "ses_fixture")
        .unwrap()
        .warnings
        .is_empty());
    assert!(
        search_events_at(&path, &detail.meta).is_err(),
        "不完整事件不可被当作完整搜索结果"
    );
}

#[test]
fn view_payloads_match_full_events_and_only_the_changed_tool_expires() {
    let (_dir, path) = fixture();
    let full = read_session_at(&path, "ses_fixture").unwrap();
    let view = read_session_mode_at(&path, "ses_fixture", true).unwrap();
    let mut tokens = vec![];
    for (a, b) in full.events.iter().zip(&view.events) {
        assert_eq!(a["uuid"], b["uuid"]);
        assert_eq!(a["timestamp"], b["timestamp"]);
        if let Some(token) = b
            .pointer("/message/content/0/deferred")
            .and_then(Value::as_str)
        {
            assert_eq!(
                crate::agents::read_tool_payload(token).unwrap(),
                a["message"]["content"][0]
            );
            tokens.push(token.to_string());
        } else {
            assert_eq!(a, b);
        }
    }
    assert_eq!(tokens.len(), 2);
    let conn = Connection::open(&path).unwrap();
    conn.execute(
        "UPDATE part SET data=?1 WHERE id='p06'",
        [json!({"type":"text","text":"CLI continued talking"}).to_string()],
    )
    .unwrap();
    assert!(
        crate::agents::read_tool_payload(&tokens[0]).is_ok(),
        "其他消息追加不应使工具内容过期"
    );
    conn.execute(
        "UPDATE part SET data=?1 WHERE id='p05'",
        [
            json!({"type":"tool","tool":"read","state":{"status":"error","error":"new error"}})
                .to_string(),
        ],
    )
    .unwrap();
    assert!(crate::agents::read_tool_payload(&tokens[1])
        .unwrap_err()
        .contains("已更新"));
    let fresh = read_session_mode_at(&path, "ses_fixture", true).unwrap();
    let token = fresh.events[4]["message"]["content"][0]["deferred"]
        .as_str()
        .unwrap();
    conn.execute("DELETE FROM part WHERE id='p05'", []).unwrap();
    assert!(crate::agents::read_tool_payload(token)
        .unwrap_err()
        .contains("已删除"));
    assert!(crate::agents::read_tool_payload("opencode:unregistered:input").is_err());
}

fn add_status_parts(path: &Path) {
    let conn = Connection::open(path).unwrap();
    conn.execute(
        "INSERT INTO message VALUES ('m_status','ses_fixture',1000002,'{\"role\":\"assistant\",\"providerID\":\"fixture\",\"modelID\":\"model-b\"}')",
        [],
    )
    .unwrap();
    for (id, part) in [
        (
            "s01",
            json!({"type":"reasoning","text":"Second stored reasoning"}),
        ),
        (
            "s02",
            json!({"type":"tool","tool":"bash","callID":"call_1","state":{"status":"error","input":{"cmd":"false"},"error":"Exit code 1"}}),
        ),
        (
            "s03",
            json!({"type":"tool","tool":"queued","state":{"status":"pending","input":{"query":"waiting"}}}),
        ),
        (
            "s04",
            json!({"type":"tool","tool":"streaming","state":{"status":"running","input":{},"output":"partial output"}}),
        ),
        ("s05", json!({"type":"tool","tool":"no-state"})),
        ("s06", json!({"type":"reasoning","text":"  "})),
        ("s07", json!({"type":"step-finish","reason":"tool-calls"})),
        (
            "s08",
            json!({"type":"text","text":"Last visible answer, final needle"}),
        ),
    ] {
        conn.execute(
            "INSERT INTO part VALUES (?1,'m_status',1000002,?2)",
            rusqlite::params![id, part.to_string()],
        )
        .unwrap();
    }
}

#[test]
fn status_errors_and_missing_call_ids_do_not_fake_completion_or_cross_pair_results() {
    let (_dir, path) = fixture();
    add_status_parts(&path);
    let detail = read_session_at(&path, "ses_fixture").unwrap();
    let blocks: Vec<_> = detail
        .events
        .iter()
        .filter_map(|e| e.pointer("/message/content/0"))
        .collect();
    let tools: Vec<_> = blocks.iter().filter(|b| b["type"] == "tool_use").collect();
    assert_eq!(tools.len(), 5);
    assert_ne!(
        tools[0]["id"], tools[1]["id"],
        "跨消息复用 callID 仍应独立配对"
    );
    assert_eq!(tools[1]["status"], "error");
    assert_eq!(tools[2]["status"], "pending");
    assert_eq!(tools[3]["status"], "running");
    assert!(tools[4].get("status").is_none());
    let results: Vec<_> = blocks
        .iter()
        .filter(|b| b["type"] == "tool_result")
        .collect();
    assert_eq!(
        results.len(),
        3,
        "等待中的工具和状态缺失的工具不应产生成功结果"
    );
    assert_eq!(results[1]["is_error"], true);
    assert_eq!(results[1]["content"], "Exit code 1");
    assert_eq!(results[2]["content"], "partial output");
    assert!(!detail.warnings.is_empty(), "状态缺失应提示数据不完整");
}

#[test]
fn malformed_parts_keep_readable_messages_and_report_warnings() {
    let (_dir, path) = fixture();
    let conn = Connection::open(&path).unwrap();
    conn.execute(
        "INSERT INTO part VALUES ('p_bad','m_assistant',1000003,'not JSON')",
        [],
    )
    .unwrap();
    let detail = read_session_at(&path, "ses_fixture").unwrap();
    assert_eq!(detail.events.len(), 7);
    assert!(!detail.warnings.is_empty());
}

#[test]
fn export_ordered_fixture_for_browser_if_requested() {
    let Ok(destination) = std::env::var("ASM_OPENCODE_EXPORT") else {
        return;
    };
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let (_dir, path) = fixture();
    add_status_parts(&path);
    if std::env::var("ASM_OPENCODE_SCHEMA").as_deref() == Ok("v2") {
        super::v2_tests::convert_browser_fixture(&path);
    }
    let mut view = read_session_mode_at(&path, "ses_fixture", true).unwrap();
    let names = super::models::Names::from([
        ("fixture/model-a".to_string(), "Model Alpha".to_string()),
        ("fixture/model-b".to_string(), "Model Beta".to_string()),
    ]);
    super::models::decorate_detail(&mut view, &names);
    let mut payloads = std::collections::HashMap::new();
    for event in &view.events {
        if let Some(token) = event
            .pointer("/message/content/0/deferred")
            .and_then(Value::as_str)
        {
            payloads.insert(
                token.to_string(),
                crate::agents::read_tool_payload(token).unwrap(),
            );
        }
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(destination)
        .unwrap();
    file.write_all(&serde_json::to_vec(&json!({"detail":view,"payloads":payloads})).unwrap())
        .unwrap();
}

#[test]
fn real_stored_parts_are_complete_ordered_and_fast_if_present() {
    let path = db_path();
    if !path.is_file() {
        return;
    }
    let conn = open_db(&path).unwrap();
    let session: Option<String> = conn.query_row("SELECT s.id FROM session s JOIN message m ON m.session_id=s.id JOIN part p ON p.message_id=m.id
        WHERE s.parent_id IS NULL AND s.time_archived IS NULL GROUP BY s.id ORDER BY SUM(length(p.data)) DESC LIMIT 1",[],|r|r.get(0)).optional().unwrap();
    let Some(session) = session else { return };
    let started = std::time::Instant::now();
    let view = read_session_mode_at(&path, &session, true).unwrap();
    let elapsed = started.elapsed();
    let counts:Vec<_>=conn.prepare("SELECT json_extract(p.data,'$.type'),COUNT(*) FROM part p JOIN message m ON m.id=p.message_id
        WHERE m.session_id=?1 AND CASE WHEN json_valid(p.data) AND json_valid(m.data) THEN json_extract(m.data,'$.role')='assistant'
        AND COALESCE(json_extract(p.data,'$.synthetic'),0)=0
        AND (json_extract(p.data,'$.type')='tool' OR (json_extract(p.data,'$.type')='reasoning' AND LENGTH(TRIM(COALESCE(json_extract(p.data,'$.text'),'')))>0)) ELSE 0 END GROUP BY 1").unwrap()
        .query_map([&session],|r|Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)? as usize))).unwrap().map(Result::unwrap).collect();
    for (kind, count) in counts {
        let unified = if kind == "tool" {
            "tool_use"
        } else {
            "thinking"
        };
        let actual = view
            .events
            .iter()
            .filter(|e| {
                e.pointer("/message/content/0/type").and_then(Value::as_str) == Some(unified)
            })
            .count();
        assert_eq!(actual, count, "真实 SQLite 分段不应被遗漏");
    }
    let bytes = serde_json::to_vec(&view).unwrap().len();
    println!(
        "OpenCode view: {} events, {:.1} KiB IPC, {:.1} ms native read",
        view.events.len(),
        bytes as f64 / 1024.,
        elapsed.as_secs_f64() * 1000.
    );
    assert!(bytes < 2 * 1024 * 1024, "工具正文不得进入首屏 IPC");
}

#[test]
fn search_positions_stay_aligned_after_reasoning_and_tool_parts_are_inserted() {
    struct Source(PathBuf);
    impl AgentSource for Source {
        fn id(&self) -> &'static str {
            "opencode"
        }
        fn display_name(&self) -> &'static str {
            "OpenCode"
        }
        fn is_installed(&self) -> bool {
            true
        }
        fn data_dir(&self) -> Option<PathBuf> {
            Some(self.0.parent().unwrap().into())
        }
        fn catalog(&self, empty: bool) -> Result<super::super::Catalog, String> {
            catalog_at(&self.0, empty)
        }
        fn read_session(&self, _: &str, id: &str) -> Result<SessionDetail, String> {
            read_session_at(&self.0, id)
        }
        fn read_session_view(&self, _: &str, id: &str) -> Result<SessionDetail, String> {
            read_session_mode_at(&self.0, id, true)
        }
        fn delete_session(&self, _: &str, _: &str) -> Result<(), String> {
            Err("Read-only fixture".into())
        }
        fn resume_command(&self, _: &str, _: bool) -> Option<String> {
            None
        }
    }
    let (_dir, path) = fixture();
    let source = Source(path);
    let catalog = source.catalog(false).unwrap();
    super::super::search::register(&source, &catalog);
    let target = super::super::search::SessionRef::from(&catalog.sessions[0]);
    let result = super::super::search::search(
        &source,
        &[target.clone()],
        "searchable needle",
        super::super::search::start(),
    )
    .unwrap();
    assert_eq!(result.hits.len(), 1);
    let hit = &result.hits[0];
    let view = source
        .read_session_view(&target.project_dir, &target.session_id)
        .unwrap();
    assert_eq!(hit.event_index, 5);
    assert_eq!(hit.event_id.as_deref(), Some("m_assistant:p06"));
    assert_eq!(view.events[hit.event_index]["uuid"], "m_assistant:p06");
    for query in ["Stored reasoning", "Tool output"] {
        assert!(super::super::search::search(
            &source,
            &[target.clone()],
            query,
            super::super::search::start()
        )
        .unwrap()
        .hits
        .is_empty());
    }
}

#[test]
fn replacing_the_database_invalidates_old_tokens_but_a_fresh_view_still_opens_tools() {
    let (_dir, path) = fixture();
    let old = read_session_mode_at(&path, "ses_fixture", true).unwrap();
    let old_token = old.events[3]["message"]["content"][0]["deferred"]
        .as_str()
        .unwrap();
    let replacement = path.with_extension("replacement");
    std::fs::copy(&path, &replacement).unwrap();
    std::fs::rename(&replacement, &path).unwrap();
    assert!(crate::agents::read_tool_payload(old_token).is_err());
    let fresh = read_session_mode_at(&path, "ses_fixture", true).unwrap();
    let new_token = fresh.events[3]["message"]["content"][0]["deferred"]
        .as_str()
        .unwrap();
    assert_ne!(old_token, new_token);
    assert!(crate::agents::read_tool_payload(new_token).is_ok());
}
