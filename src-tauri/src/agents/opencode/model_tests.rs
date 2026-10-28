use super::*;

fn fixture() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("opencode.db");
    let c = Connection::open(&path).unwrap();
    c.execute_batch("CREATE TABLE session(id TEXT PRIMARY KEY,title TEXT,directory TEXT,model TEXT,time_created INTEGER,time_updated INTEGER,parent_id TEXT,time_archived INTEGER);
        CREATE TABLE message(id TEXT PRIMARY KEY,session_id TEXT,time_created INTEGER,data TEXT);
        CREATE TABLE part(id TEXT PRIMARY KEY,message_id TEXT,time_created INTEGER,data TEXT);
        INSERT INTO session VALUES('ses_history','Model history','/tmp/models','{\"id\":\"future-selection\",\"providerID\":\"new\"}',1000,9000,NULL,NULL);
        INSERT INTO session VALUES('ses_requested','No actual response','/tmp/models',NULL,1000,9000,NULL,NULL);").unwrap();
    for (id, session, time, data) in [
        (
            "m1",
            "ses_history",
            1000,
            json!({"role":"user","model":{"providerID":"request-only","modelID":"unexecuted"}}),
        ),
        (
            "m2",
            "ses_history",
            2000,
            json!({"role":"assistant","providerID":"provider-a","modelID":"old-model"}),
        ),
        (
            "m3",
            "ses_history",
            3000,
            json!({"role":"assistant","providerID":"provider-b","modelID":"new-model"}),
        ),
        (
            "m4",
            "ses_history",
            3000,
            json!({"role":"assistant","providerID":"provider-c","modelID":"new-model"}),
        ),
        (
            "m5",
            "ses_history",
            4000,
            json!({"role":"user","model":{"providerID":"future","modelID":"not-used"}}),
        ),
        (
            "m6",
            "ses_requested",
            1000,
            json!({"role":"user","model":{"providerID":"future","modelID":"not-used"}}),
        ),
    ] {
        c.execute(
            "INSERT INTO message VALUES(?1,?2,?3,?4)",
            rusqlite::params![id, session, time, data.to_string()],
        )
        .unwrap();
        c.execute(
            "INSERT INTO part VALUES(?1,?2,?3,?4)",
            rusqlite::params![
                format!("p-{id}"),
                id,
                time,
                json!({"type":"text","text":"visible message"}).to_string()
            ],
        )
        .unwrap();
    }
    (dir, path)
}

#[test]
fn history_model_comes_from_assistant_messages_not_current_selection() {
    let (_dir, path) = fixture();
    let cat = catalog_at(&path, false).unwrap();
    let meta = cat
        .sessions
        .iter()
        .find(|m| m.session_id == "ses_history")
        .unwrap();
    assert_eq!(meta.model.as_deref(), Some("provider-c/new-model"));
    assert_eq!(meta.model_name.as_deref(), Some("new-model"));
    assert!(cat
        .sessions
        .iter()
        .find(|m| m.session_id == "ses_requested")
        .unwrap()
        .model
        .is_none());
    let detail = read_session_at(&path, "ses_history").unwrap();
    assert_eq!(detail.meta.model, meta.model);
    let actual: Vec<_> = detail
        .events
        .iter()
        .filter(|e| e["type"] == "assistant")
        .map(|e| e["message"]["model"].as_str().unwrap())
        .collect();
    assert_eq!(
        actual,
        [
            "provider-a/old-model",
            "provider-b/new-model",
            "provider-c/new-model"
        ]
    );
}

#[test]
fn a_new_assistant_model_in_wal_updates_cached_list_and_detail() {
    let (_dir, path) = fixture();
    let c = Connection::open(&path).unwrap();
    c.execute_batch(
        "PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0; PRAGMA wal_checkpoint(TRUNCATE);",
    )
    .unwrap();
    assert_eq!(
        catalog_at(&path, false)
            .unwrap()
            .sessions
            .iter()
            .find(|m| m.session_id == "ses_history")
            .unwrap()
            .model
            .as_deref(),
        Some("provider-c/new-model")
    );
    let before = path.metadata().unwrap().modified().unwrap();
    c.execute(
        "INSERT INTO message VALUES ('m7','ses_history',7000,?1)",
        [json!({"role":"assistant","modelID":"continued","providerID":"next"}).to_string()],
    )
    .unwrap();
    c.execute(
        "INSERT INTO part VALUES ('p7','m7',7000,?1)",
        [json!({"type":"text","text":"continued answer"}).to_string()],
    )
    .unwrap();
    assert_eq!(path.metadata().unwrap().modified().unwrap(), before);
    let latest = catalog_at(&path, false)
        .unwrap()
        .sessions
        .into_iter()
        .find(|m| m.session_id == "ses_history")
        .unwrap();
    assert_eq!(latest.model.as_deref(), Some("next/continued"));
    assert_eq!(
        read_session_mode_at(&path, "ses_history", true)
            .unwrap()
            .meta
            .model,
        latest.model
    );
}

#[test]
fn readable_names_decorate_the_correct_historical_provider_only() {
    let (_dir, path) = fixture();
    let mut detail = read_session_at(&path, "ses_history").unwrap();
    let names = super::models::Names::from([
        ("provider-b/new-model".into(), "Provider B label".into()),
        ("provider-c/new-model".into(), "Provider C label".into()),
    ]);
    super::models::decorate_detail(&mut detail, &names);
    assert_eq!(detail.meta.model_name.as_deref(), Some("Provider C label"));
    let messages: Vec<_> = detail
        .events
        .iter()
        .filter(|e| e["type"] == "assistant")
        .collect();
    assert_eq!(messages[0]["message"]["modelName"], "old-model");
    assert_eq!(messages[1]["message"]["modelName"], "Provider B label");
    assert_eq!(messages[2]["message"]["modelName"], "Provider C label");
}

#[test]
fn real_session_models_match_saved_assistant_ids_if_present() {
    let path = db_path();
    if !path.is_file() {
        return;
    }
    let c = open_db(&path).unwrap();
    let catalog = OpenCodeSource.catalog(false).unwrap();
    let mut recognized = 0;
    for meta in &catalog.sessions {
        let actual:Option<(Option<String>,String)>=c.query_row("SELECT json_extract(data,'$.providerID'),json_extract(data,'$.modelID') FROM message
            WHERE session_id=?1 AND json_valid(data) AND json_extract(data,'$.role')='assistant' AND json_type(data,'$.modelID')='text'
            AND trim(json_extract(data,'$.modelID'))<>'' ORDER BY time_created DESC,id DESC LIMIT 1",[&meta.session_id],|r|Ok((r.get(0)?,r.get(1)?))).ok();
        if let Some((provider, model)) = actual {
            let id = provider.map(|p| format!("{p}/{model}")).unwrap_or(model);
            assert_eq!(meta.model.as_deref(), Some(id.as_str()));
            assert!(meta.model_name.is_some());
            recognized += 1;
        }
    }
    assert!(recognized > 0);
    println!("OpenCode historical models: {recognized}/{} visible sessions have confirmed assistant model IDs",catalog.sessions.len());
}
