use super::*;
use rusqlite::params;

const SCHEMA: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../tests/fixtures/opencode-v2.sql"
));

#[test]
fn installed_v2_runtime_database_if_requested() {
    let Ok(path) = std::env::var("ASM_OPENCODE_LIVE_DB") else {
        return;
    };
    let path = PathBuf::from(path);
    let before = std::fs::read(&path).unwrap();
    let mut catalog = catalog_at(&path, false).unwrap();
    let names = models::names();
    for meta in &mut catalog.sessions {
        models::decorate_meta(meta, &names);
    }
    assert!(catalog.warnings.is_empty(), "{:?}", catalog.warnings);
    assert_eq!(
        catalog.sessions.len(),
        2,
        "original session and actual CLI fork"
    );
    let mut details = Vec::new();
    let mut payloads = std::collections::HashMap::new();
    for meta in &catalog.sessions {
        assert!(meta.version.as_deref().unwrap().starts_with("2."));
        assert_eq!(meta.model.as_deref(), Some("asm-live/live"));
        assert_eq!(meta.model_name.as_deref(), Some("ASM Live Model"));
        assert!(meta.real_user_count >= 2);
        let full = read_session_at(&path, &meta.session_id).unwrap();
        assert!(full.warnings.is_empty(), "{:?}", full.warnings);
        assert!(full
            .events
            .iter()
            .any(|e| e["message"]["content"] == "ASM OpenCode v2 live answer"));
        assert_eq!(search_events_at(&path, meta).unwrap(), full.events);
        let mut view = read_session_mode_at(&path, &meta.session_id, true).unwrap();
        models::decorate_detail(&mut view, &names);
        for event in &view.events {
            if let Some(token) = event
                .pointer("/message/content/0/deferred")
                .and_then(Value::as_str)
            {
                payloads.insert(token.to_string(), deferred::read(token).unwrap());
            }
        }
        details.push(view);
    }
    assert!(payloads.values().any(|p| p["type"] == "tool_result"
        && p.to_string()
            .contains("Actual OpenCode v2 integration fixture")));
    assert_eq!(std::fs::read(&path).unwrap(), before);
    if let Ok(destination) = std::env::var("ASM_OPENCODE_LIVE_EXPORT") {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(destination)
            .unwrap();
        file.write_all(
            &serde_json::to_vec(&json!({"catalog":catalog,"details":details,"payloads":payloads}))
                .unwrap(),
        )
        .unwrap();
    }
}

fn fixture() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("opencode.db");
    let db = Connection::open(&path).unwrap();
    db.execute_batch(SCHEMA).unwrap();
    db.execute_batch("INSERT INTO session_v2 VALUES ('ses_v2',NULL,'/tmp/v2','2.0.0',NULL,NULL,NULL,1000000,2000000,NULL);
        INSERT INTO session_v2 VALUES ('ses_empty','Empty','/tmp/v2','2.0.0',NULL,NULL,NULL,1000000,2000000,NULL);").unwrap();
    // IDs and wall-clock times deliberately disagree with seq. Also cover control
    // records: they are neither user prompts nor malformed assistant messages.
    for (id, kind, seq, time, value) in [
        (
            "msg_z",
            "user",
            0,
            1000010,
            json!({"text":"First v2 prompt","files":[],"agents":[],"skills":[],"time":{"created":1000010}}),
        ),
        (
            "msg_a",
            "assistant",
            1,
            1000001,
            json!({"agent":"build","model":{"id":"old","providerID":"fixture"},"time":{"created":1000001},"content":[
            {"type":"reasoning","text":"V2 reasoning"},
            {"type":"text","text":"Before tool"},
            {"type":"tool","id":"call_shared","name":"read","time":{"created":1000001},"state":{"status":"completed","input":{"path":"/tmp/example"},"content":[{"type":"text","text":"V2 result needle"}]}},
            {"type":"text","text":"After tool"}]}),
        ),
        (
            "msg_control",
            "model-switched",
            2,
            1000011,
            json!({"model":{"id":"unused","providerID":"fixture"}}),
        ),
        (
            "msg_synthetic",
            "synthetic",
            3,
            1000012,
            json!({"text":"hidden system prompt"}),
        ),
        (
            "msg_b",
            "assistant",
            4,
            1000000,
            json!({"model":{"id":"latest","providerID":"fixture"},"time":{"created":1000000},"content":[
            {"type":"tool","id":"call_shared","name":"bash","state":{"status":"error","input":{},"error":{"message":"V2 failure"}}},
            {"type":"tool","id":"pending","name":"queued","state":{"status":"streaming","input":"{\"path\":"}},
            {"type":"tool","id":"running","name":"active","state":{"status":"running","input":{},"metadata":{}}},
            {"type":"text","text":"Latest answer"}]}),
        ),
        (
            "msg_compact",
            "compaction",
            5,
            1000013,
            json!({"status":"completed","model":{"id":"summary","providerID":"fixture"},"summary":"Internal summary"}),
        ),
    ] {
        db.execute(
            "INSERT INTO session_message VALUES (?1,'ses_v2',?2,?3,?4,?4,?5)",
            params![id, kind, seq, time, value.to_string()],
        )
        .unwrap();
    }
    (dir, path)
}

#[test]
fn v2_only_database_supports_catalog_order_models_search_and_read_only_access() {
    let (_dir, path) = fixture();
    let before = std::fs::read(&path).unwrap();
    let catalog = catalog_at(&path, false).unwrap();
    assert!(catalog.warnings.is_empty(), "{:?}", catalog.warnings);
    assert_eq!(catalog.sessions.len(), 1);
    let meta = &catalog.sessions[0];
    assert_eq!(meta.title.as_deref(), Some("First v2 prompt"));
    assert_eq!((meta.real_user_count, meta.assistant_text_count), (1, 2));
    assert_eq!(meta.model.as_deref(), Some("fixture/latest"));
    let detail = read_session_at(&path, "ses_v2").unwrap();
    assert!(detail.warnings.is_empty(), "{:?}", detail.warnings);
    assert_eq!(detail.events[0]["message"]["content"], "First v2 prompt");
    assert_eq!(
        detail.events[1]["message"]["content"][0]["thinking"],
        "V2 reasoning"
    );
    assert_eq!(
        detail.events[4]["message"]["content"][0]["content"][0]["text"],
        "V2 result needle"
    );
    assert_eq!(
        detail
            .events
            .iter()
            .rev()
            .find(|e| e["type"] == "assistant")
            .unwrap()["message"]["content"],
        "Latest answer"
    );
    assert_eq!(search_events_at(&path, meta).unwrap(), detail.events);
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert!(open_db(&path)
        .unwrap()
        .execute("DELETE FROM main.session_v2", [])
        .is_err());
    assert_eq!(catalog_at(&path, true).unwrap().sessions.len(), 2);
}

#[test]
fn context_visibility_opencode_v2_synthetic_and_compaction() {
    let (_dir, path) = fixture();
    let before = std::fs::read(&path).unwrap();
    let detail = read_session_at(&path, "ses_v2").unwrap();
    assert_eq!(
        (
            detail.meta.real_user_count,
            detail.meta.assistant_text_count
        ),
        (1, 2)
    );
    assert_eq!(
        detail
            .events
            .iter()
            .filter(|e| e["type"] == "context")
            .count(),
        2
    );
    assert!(detail
        .events
        .iter()
        .any(|e| e["context"]["kind"] == "compaction"));
    assert!(detail.warnings.is_empty());
    assert_eq!(std::fs::read(&path).unwrap(), before);
    super::super::context_test_support::check_and_export(
        "opencode-v2",
        detail,
        "hidden system prompt",
        "Latest answer",
    );
}

#[test]
fn v2_deferred_tools_track_individual_content_and_wal_updates() {
    let (_dir, path) = fixture();
    let db = Connection::open(&path).unwrap();
    db.execute_batch("PRAGMA journal_mode=WAL;").unwrap();
    let detail = read_session_mode_at(&path, "ses_v2", true).unwrap();
    let blocks: Vec<_> = detail
        .events
        .iter()
        .filter_map(|e| e.pointer("/message/content/0"))
        .collect();
    let tools: Vec<_> = blocks.iter().filter(|b| b["type"] == "tool_use").collect();
    assert_eq!(tools.len(), 4);
    assert_ne!(tools[0]["id"], tools[1]["id"]);
    assert_eq!(tools[2]["status"], "pending");
    assert_eq!(tools[3]["status"], "running");
    assert_eq!(
        blocks.iter().filter(|b| b["type"] == "tool_result").count(),
        2
    );
    let token = detail.events[4]["message"]["content"][0]["deferred"]
        .as_str()
        .unwrap();
    assert_eq!(
        deferred::read(token).unwrap()["content"][0]["text"],
        "V2 result needle"
    );
    db.execute("UPDATE session_message SET data=json_set(data,'$.content[3].text','Changed other part') WHERE id='msg_a'",[]).unwrap();
    assert!(
        deferred::read(token).is_ok(),
        "unrelated content must not invalidate this tool"
    );
    db.execute("UPDATE session_message SET data=json_set(data,'$.content[2].state.content[0].text','New result') WHERE id='msg_a'",[]).unwrap();
    assert!(deferred::read(token).unwrap_err().contains("已更新"));
    let fresh = read_session_mode_at(&path, "ses_v2", true).unwrap();
    let token = fresh.events[4]["message"]["content"][0]["deferred"]
        .as_str()
        .unwrap();
    assert_eq!(
        deferred::read(token).unwrap()["content"][0]["text"],
        "New result"
    );
    db.execute("DELETE FROM session_message WHERE id='msg_a'", [])
        .unwrap();
    assert!(deferred::read(token).is_err());
}

#[test]
fn v2_and_legacy_can_coexist_without_duplicate_sessions_or_stale_messages() {
    let (_dir, path) = fixture();
    let db = Connection::open(&path).unwrap();
    db.execute_batch("CREATE TABLE session(id TEXT,title TEXT,directory TEXT);
        CREATE TABLE message(id TEXT,session_id TEXT,time_created INTEGER,data TEXT);
        CREATE TABLE part(id TEXT,message_id TEXT,time_created INTEGER,data TEXT);
        INSERT INTO session VALUES ('ses_v2','Obsolete','/tmp/old'),('ses_legacy','Legacy','/tmp/old');
        INSERT INTO message VALUES ('legacy','ses_legacy',1,'{\"role\":\"user\"}'),('duplicate','ses_v2',1,'{\"role\":\"user\"}');
        INSERT INTO part VALUES ('p','legacy',1,'{\"type\":\"text\",\"text\":\"legacy prompt\"}'),('q','duplicate',1,'{\"type\":\"text\",\"text\":\"obsolete prompt\"}');").unwrap();
    let catalog = catalog_at(&path, false).unwrap();
    assert!(catalog.warnings.is_empty(), "{:?}", catalog.warnings);
    assert_eq!(catalog.sessions.len(), 2);
    assert_eq!(
        read_session_at(&path, "ses_legacy").unwrap().events[0]["message"]["content"],
        "legacy prompt"
    );
    assert_eq!(
        read_session_at(&path, "ses_v2")
            .unwrap()
            .meta
            .real_user_count,
        1
    );
}

#[test]
fn v2_corruption_stays_visible_and_unrelated_damage_does_not_poison_details() {
    let (_dir, path) = fixture();
    let db = Connection::open(&path).unwrap();
    db.execute_batch("INSERT INTO session_message VALUES ('bad','ses_empty','assistant',0,1,1,'broken');
        INSERT INTO session_message VALUES ('badcontent','ses_empty','assistant',1,2,2,'{\"content\":false}');").unwrap();
    assert!(read_session_at(&path, "ses_v2")
        .unwrap()
        .warnings
        .is_empty());
    let catalog = catalog_at(&path, false).unwrap();
    assert_eq!(catalog.sessions.len(), 2, "unknown counts are not empty");
    assert!(!catalog.warnings.is_empty());
    assert!(!read_session_at(&path, "ses_empty")
        .unwrap()
        .warnings
        .is_empty());
    let meta = catalog
        .sessions
        .iter()
        .find(|s| s.session_id == "ses_empty")
        .unwrap();
    assert!(search_events_at(&path, meta).is_err());
}

#[test]
fn v2_detail_search_and_deferred_reads_filter_before_expanding_unrelated_bodies() {
    let (_dir, path) = fixture();
    let db = Connection::open(&path).unwrap();
    db.execute_batch(
        "INSERT INTO session_message VALUES ('unrelated','ses_empty','assistant',0,1,1,'{}');
        ALTER TABLE session_message RENAME TO stored_messages;
        CREATE VIEW session_message AS SELECT id,session_id,type,seq,time_created,time_updated,
            CASE WHEN session_id='ses_v2' THEN data ELSE json_extract('broken','$') END AS data
            FROM stored_messages;",
    )
    .unwrap();
    let detail = read_session_mode_at(&path, "ses_v2", true).unwrap();
    assert!(detail.warnings.is_empty(), "{:?}", detail.warnings);
    let token = detail.events[4]["message"]["content"][0]["deferred"]
        .as_str()
        .unwrap();
    assert!(deferred::read(token).is_ok());
    assert!(!search_events_at(&path, &detail.meta).unwrap().is_empty());
}

// Run the existing browser assertions against the same conversation represented
// in either storage format, including deferred tools. No personal data is exported.
pub(super) fn convert_browser_fixture(path: &Path) {
    let db = Connection::open(path).unwrap();
    db.execute_batch(SCHEMA).unwrap();
    db.execute_batch(
        "INSERT INTO session_v2 (id,title,directory,version,time_created,time_updated)
        SELECT id,title,directory,'2.0.0',time_created,time_updated FROM session;",
    )
    .unwrap();
    let messages: Vec<_> = db
        .prepare("SELECT id,session_id,time_created,data FROM message ORDER BY time_created,id")
        .unwrap()
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, String>(3)?,
            ))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect();
    for (seq, (id, session, time, data)) in messages.iter().enumerate() {
        let m: Value = serde_json::from_str(data).unwrap();
        let parts: Vec<Value> = db
            .prepare("SELECT data FROM part WHERE message_id=?1 ORDER BY time_created,id")
            .unwrap()
            .query_map([id], |r| r.get::<_, String>(0))
            .unwrap()
            .map(|r| serde_json::from_str(&r.unwrap()).unwrap())
            .collect();
        let value = if m["role"] == "user" {
            json!({"text":parts.iter().filter_map(|p|p["text"].as_str()).collect::<Vec<_>>().join("\n"),"time":{"created":time},"files":[],"agents":[],"skills":[]})
        } else {
            let content: Vec<_> = parts
                .into_iter()
                .map(|mut p| {
                    if p["type"] == "tool" {
                        p["name"] = p
                            .as_object_mut()
                            .unwrap()
                            .remove("tool")
                            .unwrap_or(Value::Null);
                        p["id"] = p
                            .as_object_mut()
                            .unwrap()
                            .remove("callID")
                            .unwrap_or(Value::Null);
                        if let Some(output) = p["state"].get_mut("output") {
                            let text = output.take();
                            p["state"]["content"] = json!([{"type":"text","text":text}]);
                        }
                        if p["state"]["status"] == "pending" {
                            p["state"]["status"] = json!("streaming");
                        }
                    }
                    p
                })
                .collect();
            json!({"model":{"id":m["modelID"],"providerID":m["providerID"]},"content":content,"time":{"created":time}})
        };
        db.execute(
            "INSERT INTO session_message VALUES (?1,?2,?3,?4,?5,?5,?6)",
            params![
                id,
                session,
                m["role"].as_str().unwrap(),
                seq as i64,
                time,
                value.to_string()
            ],
        )
        .unwrap();
    }
    db.execute_batch("DROP TABLE part; DROP TABLE message; DROP TABLE session;")
        .unwrap();
}
