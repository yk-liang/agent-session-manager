use super::*;
use serde_json::json;

pub(super) fn check_and_export(
    name: &'static str,
    detail: SessionDetail,
    hidden: &str,
    visible: &str,
) {
    struct Source(SessionDetail);
    impl AgentSource for Source {
        fn id(&self) -> &'static str {
            "context-fixture"
        }
        fn display_name(&self) -> &'static str {
            "Context fixture"
        }
        fn is_installed(&self) -> bool {
            true
        }
        fn data_dir(&self) -> Option<PathBuf> {
            None
        }
        fn catalog(&self, _: bool) -> Result<Catalog, String> {
            Ok(assemble_catalog(vec![(
                self.0.meta.clone(),
                "/tmp/context".into(),
            )]))
        }
        fn read_session(&self, _: &str, _: &str) -> Result<SessionDetail, String> {
            Ok(SessionDetail {
                meta: self.0.meta.clone(),
                events: self.0.events.clone(),
                warnings: vec![],
            })
        }
        fn delete_session(&self, _: &str, _: &str) -> Result<(), String> {
            panic!("read-only test")
        }
        fn resume_command(&self, _: &str, _: bool) -> Option<String> {
            None
        }
    }
    let source = Source(detail);
    let catalog = source.catalog(false).unwrap();
    search::register(&source, &catalog);
    let target = search::SessionRef::from(&source.0.meta);
    assert!(
        search::search(&source, &[target.clone()], hidden, search::start())
            .unwrap()
            .hits
            .is_empty()
    );
    let result = search::search(&source, &[target], visible, search::start()).unwrap();
    assert!(!result.hits.is_empty(), "missing visible text for {name}");
    for hit in &result.hits {
        let event = &source.0.events[hit.event_index];
        assert_ne!(event["type"], "context");
        assert_eq!(event["uuid"].as_str(), hit.event_id.as_deref());
    }
    let ids: Vec<_> = source
        .0
        .events
        .iter()
        .filter_map(|e| e["uuid"].as_str())
        .collect();
    assert_eq!(
        ids.len(),
        ids.iter().collect::<std::collections::HashSet<_>>().len()
    );
    if let Ok(dir) = std::env::var("ASM_AGENT_CONTEXT_EXPORT") {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(PathBuf::from(dir).join(format!("{name}.json")))
            .unwrap();
        file.write_all(&serde_json::to_vec(&json!({"catalog":catalog,"detail":source.0,"search":result,"hidden":hidden,"visible":visible})).unwrap()).unwrap();
    }
}
