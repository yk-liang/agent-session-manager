use super::*;
use crate::agents::claude_family::{self, FamilyOptions};
use crate::agents::SessionDetail;
use serde_json::json;
use std::fs;
use std::sync::atomic::AtomicUsize;

const OPTIONS: FamilyOptions = FamilyOptions {
    title_events: true,
    filter_non_cli_entrypoint: true,
    filter_qoderwork_cwd: true,
};

struct Fixture {
    dir: tempfile::TempDir,
    reads: AtomicUsize,
    on_read: Option<Box<dyn Fn() + Send + Sync>>,
}

impl Fixture {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
            reads: AtomicUsize::new(0),
            on_read: None,
        }
    }
    fn file(&self, project: &str, id: &str) -> PathBuf {
        self.dir
            .path()
            .join("projects")
            .join(project)
            .join(format!("{id}.jsonl"))
    }
    fn write(&self, project: &str, id: &str, events: &[Value]) {
        let path = self.file(project, id);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            path,
            events
                .iter()
                .map(|event| format!("{event}\n"))
                .collect::<String>(),
        )
        .unwrap();
    }
    fn register(&self, engine: &SearchEngine) -> Vec<SessionRef> {
        let catalog = self.catalog(true).unwrap();
        let targets = catalog.sessions.iter().map(SessionRef::from).collect();
        engine.register(self, &catalog);
        targets
    }
}

impl AgentSource for Fixture {
    fn id(&self) -> &'static str {
        "fixture"
    }
    fn display_name(&self) -> &'static str {
        "Fixture"
    }
    fn is_installed(&self) -> bool {
        true
    }
    fn data_dir(&self) -> Option<PathBuf> {
        Some(self.dir.path().to_path_buf())
    }
    fn catalog(&self, include_empty: bool) -> Result<Catalog, String> {
        claude_family::catalog(&self.dir.path().join("projects"), OPTIONS, include_empty)
    }
    fn read_session(&self, project: &str, id: &str) -> Result<SessionDetail, String> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        if let Some(callback) = &self.on_read {
            callback();
        }
        claude_family::read_session(&self.dir.path().join("projects"), project, id, OPTIONS)
    }
    fn delete_session(&self, _: &str, _: &str) -> Result<(), String> {
        panic!("Search must never delete files")
    }
    fn resume_command(&self, _: &str, _: bool) -> Option<String> {
        None
    }
}

fn message(text: &str) -> Value {
    json!({"type":"assistant","uuid":"answer","message":{"content":text}})
}

#[test]
fn matches_visible_body_and_keeps_detail_event_position() {
    let fixture = Fixture::new();
    fixture.write("one", "aaaa", &[
        json!({"type":"user","isMeta":true,"message":{"content":"needle meta"}}),
        json!({"type":"system","content":"needle system"}),
        json!({"type":"assistant","message":{"content":[{"type":"thinking","thinking":"needle"},{"type":"tool_use","input":{"needle":true}}]}}),
        json!({"type":"user","message":{"content":[{"type":"tool_result","content":"needle tool result"}]}}),
        json!({"type":"assistant","uuid":"hit-id","message":{"content":[null,{"type":"text","text":{"malformed":true}},{"type":"text","text":"正常正文"},{"type":"text","text":"中文 😀 NeEdLe 的正文"}]}}),
    ]);
    let engine = SearchEngine::new(1024);
    let targets = fixture.register(&engine);
    let result = engine
        .search(&fixture, &targets, "needle", engine.start())
        .unwrap();
    assert_eq!(result.hits.len(), 1);
    let hit = &result.hits[0];
    assert_eq!(hit.event_index, 4);
    assert_eq!(hit.event_id.as_deref(), Some("hit-id"));
    assert_eq!(hit.role, "assistant");
    assert!(hit.snippet.contains("😀 NeEdLe"));
    assert_eq!(result.skipped, 0);
}

#[test]
fn project_and_data_root_isolation_and_unregistered_targets() {
    let first = Fixture::new();
    first.write("one", "aaaa", &[message("needle in first project")]);
    first.write("two", "aaaa", &[message("different content")]);
    let second = Fixture::new();
    second.write("one", "aaaa", &[message("another data root")]);
    let engine = SearchEngine::new(1024);
    let first_targets = first.register(&engine);
    let second_targets = second.register(&engine);
    let result = engine
        .search(&first, &first_targets, "needle", engine.start())
        .unwrap();
    assert_eq!(result.hits.len(), 1);
    assert_eq!(result.hits[0].session.project_dir, "one");
    let result = engine
        .search(&second, &second_targets, "needle", engine.start())
        .unwrap();
    assert!(result.hits.is_empty());
    let before = first.reads.load(Ordering::Relaxed);
    let result = engine
        .search(
            &first,
            &[SessionRef {
                project_dir: "../outside".into(),
                session_id: "aaaa".into(),
            }],
            "needle",
            engine.start(),
        )
        .unwrap();
    assert_eq!(result.skipped, 1);
    assert_eq!(first.reads.load(Ordering::Relaxed), before);
}

#[test]
fn file_replacement_wal_change_and_deletion_invalidate_cached_text() {
    let fixture = Fixture::new();
    fixture.write("one", "aaaa", &[message("old needle")]);
    let engine = SearchEngine::new(1024);
    let mut catalog = fixture.catalog(true).unwrap();
    let wal = fixture.dir.path().join("fake.db-wal");
    catalog.sessions[0].source_files.push(wal.clone());
    let targets: Vec<_> = catalog.sessions.iter().map(SessionRef::from).collect();
    engine.register(&fixture, &catalog);
    assert_eq!(
        engine
            .search(&fixture, &targets, "old", engine.start())
            .unwrap()
            .hits
            .len(),
        1
    );
    assert_eq!(
        engine
            .search(&fixture, &targets, "needle", engine.start())
            .unwrap()
            .hits
            .len(),
        1
    );
    assert_eq!(fixture.reads.load(Ordering::Relaxed), 1);
    fs::write(&wal, "changed WAL").unwrap();
    engine
        .search(&fixture, &targets, "needle", engine.start())
        .unwrap();
    assert_eq!(fixture.reads.load(Ordering::Relaxed), 2);
    fixture.write("one", "aaaa", &[message("new replacement text")]);
    assert!(engine
        .search(&fixture, &targets, "old", engine.start())
        .unwrap()
        .hits
        .is_empty());
    assert_eq!(
        engine
            .search(&fixture, &targets, "replacement", engine.start())
            .unwrap()
            .hits
            .len(),
        1
    );
    fs::remove_file(fixture.file("one", "aaaa")).unwrap();
    let result = engine
        .search(&fixture, &targets, "replacement", engine.start())
        .unwrap();
    assert_eq!(result.skipped, 1);
    assert!(result.hits.is_empty());
}

#[test]
fn obsolete_search_stops_before_reading_more_sessions() {
    let engine = Arc::new(SearchEngine::new(1024));
    let mut fixture = Fixture::new();
    fixture.write("one", "aaaa", &[message("needle")]);
    fixture.write("one", "bbbb", &[message("needle")]);
    let targets = fixture.register(&engine);
    fixture.on_read = Some(Box::new({
        let engine = engine.clone();
        move || {
            engine.start();
        }
    }));
    let result = engine
        .search(&fixture, &targets, "needle", engine.start())
        .unwrap();
    assert!(result.cancelled);
    assert!(result.hits.is_empty());
    assert_eq!(fixture.reads.load(Ordering::Relaxed), 1);
}

#[test]
fn literal_unicode_queries_preserve_original_snippet_and_blank_search_reads_nothing() {
    let fixture = Fixture::new();
    let text = format!("{}😀İABC [a].* 尾部{}", "前".repeat(100), "后".repeat(100));
    fixture.write("one", "aaaa", &[message(&text)]);
    let engine = SearchEngine::new(2048);
    let targets = fixture.register(&engine);
    assert!(engine
        .search(&fixture, &targets, "  ", engine.start())
        .unwrap()
        .hits
        .is_empty());
    assert_eq!(fixture.reads.load(Ordering::Relaxed), 0);
    for query in ["i", "abc", "[a].*", "😀"] {
        let result = engine
            .search(&fixture, &targets, query, engine.start())
            .unwrap();
        assert_eq!(result.hits.len(), 1);
        assert!(result.hits[0].snippet.contains("😀İABC [a].*"));
        assert!(result.hits[0].snippet.starts_with('…'));
        assert!(result.hits[0].snippet.ends_with('…'));
    }
    assert!(engine
        .search(&fixture, &targets, &"中".repeat(257), engine.start())
        .is_err());
    let serialized = serde_json::to_string(&fixture.catalog(true).unwrap()).unwrap();
    assert!(!serialized.contains("sourceFiles"));
}

#[test]
fn memory_budget_evicts_old_documents_and_reloads_them() {
    let fixture = Fixture::new();
    for id in ["aaaa", "bbbb", "cccc"] {
        fixture.write("one", id, &[message("needle with content")]);
    }
    let engine = SearchEngine::new(64);
    let targets = fixture.register(&engine);
    engine
        .search(&fixture, &targets, "needle", engine.start())
        .unwrap();
    assert!(engine.index.lock().unwrap().bytes <= 64);
    let before = fixture.reads.load(Ordering::Relaxed);
    engine
        .search(&fixture, &targets, "needle", engine.start())
        .unwrap();
    assert!(fixture.reads.load(Ordering::Relaxed) > before);
}
