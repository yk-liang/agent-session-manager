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
    let path = source.data_dir().unwrap().join("opencode.db");
    let conn = agents::opencode::open_db(&path).unwrap();
    let requested = std::env::var("ASM_OPENCODE_SESSION").ok();
    let (id,bytes):(String,i64)=conn.query_row("SELECT s.id,sum(length(p.data)) FROM session s JOIN message m ON m.session_id=s.id JOIN part p ON p.message_id=m.id
        WHERE s.parent_id IS NULL AND s.time_archived IS NULL AND (?1 IS NULL OR s.id=?1) GROUP BY s.id ORDER BY 2 DESC LIMIT 1",[requested],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
    let start = Instant::now();
    let detail = source.read_session_view("", &id).unwrap();
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
        "source_mib":bytes as f64/1048576.,"events":detail.events.len(),
        "read_ms":read_ms,"first_tool_ms":start.elapsed().as_secs_f64()*1000.,"ipc_kib":serde_json::to_vec(&detail).unwrap().len() as f64/1024.,
        "tools":detail.events.iter().filter(|e|e.pointer("/message/content/0/type").and_then(serde_json::Value::as_str)==Some("tool_use")).count(),
        "thinking":detail.events.iter().filter(|e|e.pointer("/message/content/0/type").and_then(serde_json::Value::as_str)==Some("thinking")).count()})
    );
}
