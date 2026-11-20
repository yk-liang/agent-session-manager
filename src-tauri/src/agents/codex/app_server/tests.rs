use super::*;
use std::io::Cursor;

#[test]
fn mutations_use_the_verified_thread_and_ignore_notifications() {
    for (action, method) in [
        (ThreadAction::Archive, "thread/archive"),
        (ThreadAction::Unarchive, "thread/unarchive"),
        (ThreadAction::Delete, "thread/delete"),
    ] {
        let responses = b"{\"id\":1,\"result\":{}}\n\
            {\"id\":2,\"result\":{\"thread\":{\"id\":\"test-thread\",\"cwd\":\"/tmp/project\"}}}\n\
            {\"method\":\"thread/archived\",\"params\":{}}\n\
            {\"id\":3,\"result\":{}}\n";
        let mut input = Vec::new();
        update_thread(
            &mut input,
            Cursor::new(responses),
            action,
            &escape_cwd("/private/tmp/project"),
            "test-thread",
        )
        .unwrap();
        let sent: Vec<Value> = Cursor::new(input)
            .lines()
            .map(|line| serde_json::from_str(&line.unwrap()).unwrap())
            .collect();
        assert_eq!(
            sent.iter()
                .map(|row| row["method"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["initialize", "initialized", "thread/read", method]
        );
        assert_eq!(sent[3]["params"]["threadId"], "test-thread");
    }
}

#[test]
fn rejects_a_mismatched_project_before_sending_a_mutation() {
    let responses = b"{\"id\":1,\"result\":{}}\n\
        {\"id\":2,\"result\":{\"thread\":{\"id\":\"test-thread\",\"cwd\":\"/tmp/other\"}}}\n";
    let mut input = Vec::new();
    let result = update_thread(
        &mut input,
        Cursor::new(responses),
        ThreadAction::Delete,
        &escape_cwd("/tmp/project"),
        "test-thread",
    );
    assert!(matches!(result, Err(UpdateError::Operation(message)) if message.contains("所属项目")));
    assert_eq!(Cursor::new(input).lines().count(), 3);
}

#[test]
fn only_an_explicitly_unsupported_mutation_can_try_another_runtime() {
    let preflight = b"{\"id\":1,\"result\":{}}\n\
        {\"id\":2,\"result\":{\"thread\":{\"id\":\"test-thread\",\"cwd\":\"/tmp/project\"}}}\n";
    let run = |reply: &[u8]| {
        update_thread(
            std::io::sink(),
            Cursor::new([preflight.as_slice(), reply].concat()),
            ThreadAction::Delete,
            &escape_cwd("/tmp/project"),
            "test-thread",
        )
    };
    assert!(matches!(
        run(b"{\"id\":3,\"error\":{\"code\":-32601}}\n"),
        Err(UpdateError::Unavailable(_))
    ));
    assert!(matches!(
        run(b"{\"id\":3,\"error\":{\"code\":-32603,\"message\":\"Deletion failed\"}}\n"),
        Err(UpdateError::Operation(_))
    ));
    assert!(matches!(run(b""), Err(UpdateError::Operation(_))));
}

#[test]
#[ignore = "需要本机有支持归档和永久删除的 Codex App Server"]
fn local_app_server_moves_and_deletes_a_temporary_rollout() {
    let mut executables = runtime::executables();
    if let Some(bin) = std::env::var_os("ASM_TEST_CODEX_RUNTIME") {
        let requested = PathBuf::from(bin).canonicalize().unwrap();
        executables.retain(|path| path.canonicalize().is_ok_and(|path| path == requested));
    }
    assert!(!executables.is_empty(), "No matching Codex App Server");
    check_runtime(&executables);
}

fn check_runtime(executables: &[PathBuf]) {
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
    let update = |action| update_using(executables, &home, action, &project, id).unwrap();
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
    update(ThreadAction::Delete);
    assert!(!archived.exists());
    assert!(super::super::read_session_in(&root, id).is_err());
}
