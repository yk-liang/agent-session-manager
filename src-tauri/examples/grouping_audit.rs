//! 只读归组审计：输出各 Agent 的数量，不输出会话正文、标题或完整路径。
#![allow(dead_code, unused_imports)]
#[path = "../src/agents/mod.rs"]
mod agents;

fn main() {
    use std::collections::{HashMap, HashSet};
    let requested = std::env::args().nth(1);
    for source in agents::all_sources() {
        if requested.as_deref().is_some_and(|id| id != source.id()) {
            continue;
        }
        if !source.is_installed() {
            println!(
                "{}",
                serde_json::json!({"agent":source.id(),"installed":false})
            );
            continue;
        }
        let catalog = source.catalog(false).expect("Catalog audit failed");
        let hints: HashMap<_, _> = catalog
            .session_group_hints
            .iter()
            .map(|h| {
                (
                    (h.project_dir.as_str(), h.session_id.as_str()),
                    h.root.as_deref(),
                )
            })
            .collect();
        let mut projectless = 0;
        let mut temporary = 0;
        let mut pytest = 0;
        let mut ungrouped_pytest = 0;
        let mut groups = HashSet::new();
        for session in &catalog.sessions {
            let cwd = session.cwd.as_deref().unwrap_or_default();
            let is_temp = [
                "/tmp/",
                "/private/tmp/",
                "/var/tmp/",
                "/private/var/tmp/",
                "/var/folders/",
                "/private/var/folders/",
            ]
            .iter()
            .any(|prefix| cwd.starts_with(prefix));
            let is_pytest = is_temp && cwd.contains("/pytest-of-") && cwd.contains("/pytest-");
            temporary += usize::from(is_temp);
            pytest += usize::from(is_pytest);
            let key = (session.project_dir.as_str(), session.session_id.as_str());
            let group = match hints.get(&key) {
                Some(None) => None,
                Some(Some(root)) => Some(format!("root:{root}")),
                None if catalog
                    .projectless_session_ids
                    .contains(&session.session_id) =>
                {
                    None
                }
                None => Some(format!("source:{}", session.project_dir)),
            };
            if group.is_none() {
                projectless += 1;
            }
            if is_pytest && group.is_some() {
                ungrouped_pytest += 1;
            }
            groups.insert(group);
        }
        println!(
            "{}",
            serde_json::json!({"agent":source.id(),"sessions":catalog.sessions.len(),
            "display_groups":groups.len(),"projectless_sessions":projectless,"temporary_cwd_sessions":temporary,
            "pytest_sessions":pytest,"pytest_not_projectless":ungrouped_pytest,"warnings":catalog.warnings.len()})
        );
    }
}
