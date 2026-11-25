use super::*;
use std::io::Cursor;
use std::sync::{Arc, Mutex};

/// 记录发给 App Server 的请求。
#[derive(Clone, Default)]
struct Sent(Arc<Mutex<Vec<u8>>>);

impl Write for Sent {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Sent {
    fn requests(&self) -> Vec<Value> {
        Cursor::new(self.0.lock().unwrap().clone())
            .lines()
            .map(|line| serde_json::from_str(&line.unwrap()).unwrap())
            .collect()
    }
}

fn server(responses: &[u8]) -> (AppServer, Sent) {
    let sent = Sent::default();
    (AppServer::over(Path::new("codex"), sent.clone(), Cursor::new(responses.to_vec())), sent)
}

const THREAD: &[u8] = b"{\"id\":1,\"result\":{\"thread\":{\"id\":\"test-thread\",\"cwd\":\"/tmp/project\"}}}\n";

#[test]
fn mutations_use_the_verified_thread_and_ignore_notifications() {
    for (action, method) in [
        (ThreadAction::Archive, "thread/archive"),
        (ThreadAction::Unarchive, "thread/unarchive"),
        (ThreadAction::Delete, "thread/delete"),
    ] {
        let (mut server, sent) = server(&[THREAD, b"{\"method\":\"thread/archived\",\"params\":{}}\n{\"id\":2,\"result\":{}}\n"].concat());
        server.read_thread(&escape_cwd("/private/tmp/project"), "test-thread").unwrap();
        server.mutate(action, "test-thread").unwrap();
        let sent = sent.requests();
        assert_eq!(sent.iter().map(|row| row["method"].as_str().unwrap()).collect::<Vec<_>>(), ["thread/read", method]);
        assert_eq!(sent[1]["params"]["threadId"], "test-thread");
    }
}

#[test]
fn rejects_a_mismatched_project_before_a_mutation() {
    let (mut server, _) = server(b"{\"id\":1,\"result\":{\"thread\":{\"id\":\"test-thread\",\"cwd\":\"/tmp/other\"}}}\n");
    let result = server.read_thread(&escape_cwd("/tmp/project"), "test-thread");
    assert!(matches!(result, Err(UpdateError::Operation(message)) if message.contains("所属项目")));
}

#[test]
fn only_an_explicitly_unsupported_mutation_can_try_another_runtime() {
    let run = |reply: &[u8]| {
        let (mut server, _) = server(&[THREAD, reply].concat());
        server.read_thread(&escape_cwd("/tmp/project"), "test-thread").unwrap();
        server.mutate(ThreadAction::Delete, "test-thread")
    };
    assert!(matches!(run(b"{\"id\":2,\"error\":{\"code\":-32601}}\n"), Err(UpdateError::Unavailable(_))));
    assert!(matches!(
        run(b"{\"id\":2,\"error\":{\"code\":-32603,\"message\":\"Deletion failed\"}}\n"),
        Err(UpdateError::Operation(_))
    ));
    assert!(matches!(run(b""), Err(UpdateError::Operation(_))));
}

#[test]
#[ignore = "需要本机有支持归档和永久删除的 Codex App Server"]
fn local_app_server_moves_and_deletes_a_temporary_rollout() {
    use std::fs;
    let directory = tempfile::tempdir().unwrap();
    let home = directory.path().canonicalize().unwrap();
    let root = home.join("sessions");
    let day = root.join("2026/10/27");
    fs::create_dir_all(&day).unwrap();
    let id = "019a2856-2000-7000-8000-000000000001";
    let name = format!("rollout-2026-10-27T00-00-00-{id}.jsonl");
    let active = day.join(&name);
    let archived = super::super::storage::archived_root(&root).join(&name);
    let header = json!({
        "timestamp": "2026-10-27T00:00:00Z", "type": "session_meta",
        "payload": {"id": id, "timestamp": "2026-10-27T00:00:00Z", "cwd": home,
            "source": "cli", "originator": "codex_cli_rs", "cli_version": "0.170.0",
            "model_provider": "openai"}
    });
    let message = super::super::history_tests::message(1, "user", "Archive test");
    fs::write(&active, format!("{header}\n{message}\n")).unwrap();

    let project = escape_cwd(home.to_str().unwrap());
    let update = |action| update(&home, action, &project, id).unwrap();
    update(ThreadAction::Archive);
    assert!(!active.exists());
    assert!(archived.is_file());
    assert!(
        super::super::read_session_in(&root, id)
            .unwrap()
            .meta
            .archived
    );

    update(ThreadAction::Unarchive);
    assert!(active.is_file());
    assert!(!archived.exists());
    assert!(
        !super::super::read_session_in(&root, id)
            .unwrap()
            .meta
            .archived
    );

    update(ThreadAction::Archive);
    let children = [
        ("019a2856-2000-7000-8000-000000000002", id),
        (
            "019a2856-2000-7000-8000-000000000003",
            "019a2856-2000-7000-8000-000000000002",
        ),
    ];
    for (child, parent) in children {
        let mut header = header.clone();
        header["payload"]["id"] = json!(child);
        header["payload"]["source"] =
            json!({"subagent":{"thread_spawn":{"parent_thread_id":parent,"depth":1}}});
        header["payload"]["parent_thread_id"] = json!(parent);
        let child_path =
            archived.with_file_name(format!("rollout-2026-10-27T00-00-00-{child}.jsonl"));
        fs::write(child_path, format!("{header}\n{message}\n")).unwrap();
    }
    let request = crate::agents::search::SessionRef { project_dir: project.clone(), session_id: id.into() };
    let requests = [request];
    let progress = |_: usize| {};
    let mut outcome = crate::agents::deletion::Outcome::new(&requests, &progress);
    super::super::deletion::delete(&home, &requests, &mut Kept, &mut outcome).unwrap();
    let result = outcome.finish();
    assert!(result.failures.is_empty(), "{:?}", result.failures);
    assert_eq!(result.removed.len(), 3);
    assert!(!archived.exists());
    assert!(super::super::read_session_in(&root, id).is_err());
}

struct Kept;

impl crate::agents::deletion::PreserveUsage for Kept {
    fn save(&mut self, _: &[PathBuf]) -> Result<crate::agents::deletion::Unchanged, String> {
        Ok(Box::new(|| Ok(())))
    }
}
