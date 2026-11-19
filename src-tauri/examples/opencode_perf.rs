//! 本机只读验证 OpenCode 列表、模型与按需工具读取，不打印对话正文。
#![allow(dead_code, unused_imports)]
#[path = "../src/agents/mod.rs"]
mod agents;
use agents::AgentSource;
use std::time::Instant;
fn main() {
    let source = agents::opencode::OpenCodeSource;
    let start = Instant::now();
    let catalog = source.catalog(false).unwrap();
    let catalog_ms = start.elapsed().as_secs_f64() * 1000.;
    let start = Instant::now();
    source.catalog(false).unwrap();
    let cached_catalog_ms = start.elapsed().as_secs_f64() * 1000.;
    let requested = std::env::var("ASM_OPENCODE_SESSION").ok();
    let session_id = requested.as_deref().unwrap_or_else(|| {
        &catalog
            .sessions
            .iter()
            .max_by_key(|session| session.size_bytes)
            .expect("No OpenCode sessions")
            .session_id
    });
    let start = Instant::now();
    let detail = source.read_session_view("", session_id).unwrap();
    let read_ms = start.elapsed().as_secs_f64() * 1000.;
    if let Ok(path) = std::env::var("ASM_OPENCODE_VIEW_EXPORT") {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(path)
            .unwrap();
        file.write_all(&serde_json::to_vec(&detail).unwrap())
            .unwrap();
    }
    let payloads: Vec<_> = detail
        .events
        .iter()
        .filter_map(|e| {
            e.pointer("/message/content/0/deferred")
                .and_then(serde_json::Value::as_str)
        })
        .collect();
    let start = Instant::now();
    for token in payloads.iter().take(2) {
        agents::opencode::deferred::read(token).unwrap();
    }
    println!(
        "{}",
        serde_json::json!({"sessions":catalog.sessions.len(),"catalog_ms":catalog_ms,"cached_catalog_ms":cached_catalog_ms,
        "models":catalog.sessions.iter().filter(|s|s.model.is_some()).count(),
        "sessions_with_size":catalog.sessions.iter().filter(|s| s.size_bytes.is_some()).count(),
        "source_mib":detail.meta.size_bytes.map(|bytes| bytes as f64 / 1048576.),"events":detail.events.len(),
        "read_ms":read_ms,"first_tool_ms":start.elapsed().as_secs_f64()*1000.,"ipc_kib":serde_json::to_vec(&detail).unwrap().len() as f64/1024.,
        "tools":detail.events.iter().filter(|e|e.pointer("/message/content/0/type").and_then(serde_json::Value::as_str)==Some("tool_use")).count(),
        "thinking":detail.events.iter().filter(|e|e.pointer("/message/content/0/type").and_then(serde_json::Value::as_str)==Some("thinking")).count()})
    );
}
