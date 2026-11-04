use super::*;

const OPTIONS: FamilyOptions = FamilyOptions {
    title_events: true,
    filter_non_cli_entrypoint: true,
    filter_qoderwork_cwd: true,
};

#[test]
fn detail_reports_content_warning_and_missing_source_separately() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    fs::create_dir(&project).unwrap();
    let path = project.join("aaaa.jsonl");
    fs::write(&path, "{\"type\":\"user\",\"message\":{\"content\":\"fixture\"}}\n{\"private\":\"PRIVATE_BODY\",}\n").unwrap();
    let detail = read_session(temp.path(), "project", "aaaa", OPTIONS).unwrap();
    assert_eq!(detail.events.len(), 1);
    assert_eq!(detail.meta.real_user_count, 1);
    assert_eq!(detail.warnings.len(), 1);
    assert!(detail.warnings[0].contains("stage: parse_json"));
    assert!(detail.warnings[0].contains("operation: session_detail"));
    assert!(!detail.warnings[0].contains("PRIVATE_BODY"));
    fs::write(&path, "{\"private\":\"PRIVATE_BODY\",}\n").unwrap();
    let error = read_session(temp.path(), "project", "aaaa", OPTIONS)
        .err()
        .unwrap();
    assert!(error.contains("会话内容无法解析"));
    assert!(error.contains("stage: parse_json"));
    fs::remove_file(&path).unwrap();
    let error = read_session(temp.path(), "project", "aaaa", OPTIONS)
        .err()
        .unwrap();
    assert!(error.contains("stage: path_metadata"));
    assert!(error.contains("error_kind: NotFound"));
    assert!(error.contains(&format!("os_code: {}", libc::ENOENT)));
    assert!(!error.contains("无法解析"));
}

#[test]
fn detail_still_rejects_fifo_and_reads_regular_symlink() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    fs::create_dir(&project).unwrap();
    let path = project.join("aaaa.jsonl");
    let c_path = std::ffi::CString::new(path.to_str().unwrap()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);
    let base = temp.path().to_owned();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        tx.send(read_session(&base, "project", "aaaa", OPTIONS).err())
            .unwrap();
    });
    let error = rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .expect("FIFO guard must not block")
        .unwrap();
    assert!(error.contains("stage: validate_file_type"));
    assert!(error.contains("error_kind: InvalidInput"));
    assert!(error.contains("os_code: none"));
    fs::remove_file(&path).unwrap();
    let original = temp.path().join("source.jsonl");
    fs::write(
        &original,
        "{\"type\":\"user\",\"message\":{\"content\":\"fixture\"}}\n",
    )
    .unwrap();
    std::os::unix::fs::symlink(&original, &path).unwrap();
    let detail = read_session(temp.path(), "project", "aaaa", OPTIONS).unwrap();
    assert_eq!(detail.events.len(), 1);
    assert!(detail.warnings.is_empty());
}

#[test]
fn cached_summary_does_not_hide_detail_open_error() {
    const CHILD: &str = "ASM_DIAGNOSTIC_FD_CHILD";
    if std::env::var_os(CHILD).is_none() {
        // 文件描述符限制是进程级状态，必须在独立测试进程里制造故障。
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["agents::claude_family::diagnostic_tests::cached_summary_does_not_hide_detail_open_error", "--exact", "--nocapture"])
            .env(CHILD, "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    fs::create_dir(&project).unwrap();
    let path = project.join("aaaa.jsonl");
    fs::write(
        &path,
        "{\"type\":\"user\",\"message\":{\"content\":\"fixture\"}}\n",
    )
    .unwrap();
    assert_eq!(
        catalog(temp.path(), OPTIONS, false).unwrap().sessions.len(),
        1
    );
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    assert_eq!(
        unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) },
        0
    );
    let reduced = libc::rlimit {
        rlim_cur: limit.rlim_cur.min(128),
        rlim_max: limit.rlim_max,
    };
    assert_eq!(unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &reduced) }, 0);
    let mut held = Vec::new();
    while let Ok(file) = fs::File::open("/dev/null") {
        held.push(file);
    }
    let io_error = super::super::open_jsonl(&path).unwrap_err();
    assert_eq!(io_error.raw_os_error(), Some(libc::EMFILE));
    let cached = super::super::catalog_cache::summary(
        "family-context-v2:true:true:true:project",
        &[path],
        || panic!("warm summary must come from cache"),
    );
    let error = read_session(temp.path(), "project", "aaaa", OPTIONS)
        .err()
        .unwrap();
    drop(held);
    assert_eq!(unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &limit) }, 0);
    assert_eq!(cached.unwrap().0.title.as_deref(), Some("fixture"));
    assert!(
        error.contains("stage: open"),
        "detail lost the operation stage: {error}"
    );
    assert!(
        error.contains(&format!("os_code: {}", libc::EMFILE)),
        "detail lost the original errno: {error}"
    );
    assert!(
        error.contains("operation: session_detail"),
        "detail lost the GUI operation: {error}"
    );
    assert!(
        !error.contains("无法解析"),
        "access failure was misreported as parsing failure"
    );
}
