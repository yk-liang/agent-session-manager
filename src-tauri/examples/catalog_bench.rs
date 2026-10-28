//! 只读性能复测：cargo run --release --offline --locked --example catalog_bench
#![allow(dead_code, unused_imports)]
#[path = "../src/agents/mod.rs"]
mod agents;

fn main() {
    let mut names = None;
    for round in 1..=3 {
        for source in agents::all_sources() {
            if !source.is_installed() {
                continue;
            }
            let start = std::time::Instant::now();
            let catalog = source.catalog(false).expect("catalog scan failed");
            let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
            let start = std::time::Instant::now();
            agents::search::register(source.as_ref(), &catalog);
            let register_ms = start.elapsed().as_secs_f64() * 1000.0;
            println!(
                "{}",
                serde_json::json!({"round": round, "agent": source.id(),
                "sessions": catalog.sessions.len(), "catalog_ms": elapsed_ms,
                "register_ms": register_ms, "warnings": catalog.warnings.len()})
            );
            if round == 1 && source.id() == "qoder" {
                let models: Vec<_> = catalog
                    .sessions
                    .iter()
                    .filter_map(|s| s.model.clone())
                    .collect();
                names = Some(std::thread::spawn(move || {
                    let start = std::time::Instant::now();
                    let resolved = agents::find_source("qoder")
                        .unwrap()
                        .resolve_model_names(&models);
                    println!(
                        "{}",
                        serde_json::json!({"agent":"qoder", "background_names_ms":
                        start.elapsed().as_secs_f64() * 1000.0, "resolved_names": resolved.len()})
                    );
                }));
            }
        }
    }
    if let Some(task) = names {
        task.join().unwrap();
    }
}
