//! 只读测量首屏、精确摘要、长会话详情。导出仅供本机 UI 回归，不记录正文到控制台。
#![allow(dead_code, unused_imports)]
#[path = "../src/agents/mod.rs"]
mod agents;
use agents::AgentSource;
use std::time::Instant;
fn main() {
    let source = agents::codex::CodexSource;
    let start = Instant::now();
    let quick = source.quick_catalog(false).unwrap();
    println!(
        "{}",
        serde_json::json!({"phase":"first-screen","ms":start.elapsed().as_secs_f64()*1000.,"sessions":quick.sessions.len(),"models":quick.sessions.iter().filter(|s|s.model.is_some()).count(),"pending":quick.sessions.iter().filter(|s|s.stats_pending).count(),"warnings":quick.warnings.len()})
    );
    if std::env::args().any(|s| s == "--quick-only") {
        return;
    }
    let start = Instant::now();
    let full = source.catalog(false).unwrap();
    println!(
        "{}",
        serde_json::json!({"phase":"refined","ms":start.elapsed().as_secs_f64()*1000.,"sessions":full.sessions.len(),"models":full.sessions.iter().filter(|s|s.model.is_some()).count(),"warnings":full.warnings.len()})
    );
    let start = Instant::now();
    let cached = source.quick_catalog(false).unwrap();
    println!(
        "{}",
        serde_json::json!({"phase":"cached-list","ms":start.elapsed().as_secs_f64()*1000.,"models":cached.sessions.iter().filter(|s|s.model.is_some()).count()})
    );
    if std::env::args().any(|s| s == "--catalog-only") {
        return;
    }
    if let Some(meta) = full
        .sessions
        .iter()
        .filter(|s| {
            s.cwd
                .as_deref()
                .is_some_and(|cwd| cwd.ends_with("/agent-session-manager"))
        })
        .max_by_key(|s| s.size_bytes)
    {
        let start = Instant::now();
        let detail = source
            .read_session_view(&meta.project_dir, &meta.session_id)
            .unwrap();
        println!(
            "{}",
            serde_json::json!({"phase":"project-view","ms":start.elapsed().as_secs_f64()*1000.,"events":detail.events.len(),"source_mib":meta.size_bytes as f64/1048576.})
        );
    }
    let largest = full.sessions.iter().max_by_key(|s| s.size_bytes).unwrap();
    let start = Instant::now();
    let detail = source
        .read_session_view(&largest.project_dir, &largest.session_id)
        .unwrap();
    let elapsed = start.elapsed().as_secs_f64() * 1000.;
    let json = serde_json::to_vec(&detail).unwrap();
    println!(
        "{}",
        serde_json::json!({"phase":"largest-view","ms":elapsed,"source_mib":largest.size_bytes as f64/1048576.,"events":detail.events.len(),"ipc_kib":json.len() as f64/1024.})
    );
    if let Ok(path) = std::env::var("ASM_PERF_EXPORT") {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(path)
            .unwrap();
        f.write_all(&json).unwrap();
    }
}
