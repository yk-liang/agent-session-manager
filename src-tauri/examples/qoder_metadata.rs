//! 只读验证指定会话的展示元数据；不输出正文，不改写日志或桌面数据库。
#![allow(dead_code, unused_imports)]
#[path = "../src/agents/mod.rs"]
mod agents;

fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let id = args
        .first()
        .expect("qoder_metadata <session-id> [expected-title] [expected-model]");
    let source = agents::find_source("qoder").unwrap();
    let catalog = source.catalog(false).expect("Catalog read failed");
    let sessions: Vec<_> = catalog
        .sessions
        .iter()
        .filter(|session| &session.session_id == id)
        .collect();
    assert_eq!(
        sessions.len(),
        1,
        "Requested session must be present and unambiguous"
    );
    let session = sessions[0];
    let detail = source
        .read_session(&session.project_dir, id)
        .expect("Session detail read failed");
    assert_eq!(
        detail.meta.title, session.title,
        "Catalog and detail titles differ"
    );
    assert_eq!(
        detail.meta.model, session.model,
        "Catalog and detail model selections differ"
    );
    if let Some(expected) = args.get(1) {
        assert_eq!(session.title.as_ref(), Some(expected));
    }
    if let Some(expected) = args.get(2) {
        assert_eq!(session.model.as_ref(), Some(expected));
    }
    println!(
        "{}",
        serde_json::json!({"sessionId":session.session_id,"title":session.title,
        "model":session.model,"catalogWarnings":catalog.warnings.len(),"detailWarnings":detail.warnings.len()})
    );
}
