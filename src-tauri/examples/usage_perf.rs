//! 本机只读复测：输出计量数字与耗时，不打印对话或请求参数。
#![allow(dead_code, unused_imports)]
#[path = "../src/agents/mod.rs"]
mod agents;
#[path = "../src/session_watch.rs"]
mod session_watch;
#[path = "../src/usage/mod.rs"]
mod usage;
fn main() {
    use std::time::Instant;
    let args: Vec<_> = std::env::args().collect();
    // 要求隔离目录，测量与迁移不触碰正在运行应用的账本；源日志只读。
    let dir = args
        .windows(2)
        .find(|a| a[0] == "--storage")
        .map(|a| std::path::PathBuf::from(&a[1]))
        .expect("usage_perf --storage <private directory>");
    if args.iter().any(|a| a == "--cold") {
        assert!(
            !dir.join("cache.sqlite3").exists(),
            "Cold measurements require an empty storage directory; no data is removed"
        );
    }
    let open = || {
        usage::Store::open_with_ledger(
            Some(&dir.join("cache.sqlite3")),
            Some(&dir.join("ledger.sqlite3")),
        )
    };
    let mut store = open();
    for agent in usage::AGENTS {
        let roots = usage::roots(agent).unwrap();
        let started = Instant::now();
        let first = usage::summary::sync_at(agent, &roots, &mut store, &|| false).unwrap();
        let first_ms = started.elapsed().as_secs_f64() * 1000.;
        // 重启后的热同步不能依赖内存里的全量历史。
        drop(store);
        store = open();
        let started = Instant::now();
        let warm = usage::summary::sync_at(agent, &roots, &mut store, &|| false).unwrap();
        let warm_ms = started.elapsed().as_secs_f64() * 1000.;
        println!(
            "{}",
            serde_json::json!({"agent":agent,"checked_files":first.checked_files,"changed_files":first.changed_files,"sync_ms":first_ms,"read_bytes":first.bytes_read,"restart_warm_ms":warm_ms,"warm_changed_files":warm.changed_files,"warm_read_bytes":warm.bytes_read})
        );
    }
    use chrono::TimeZone;
    let now = chrono::Local::now();
    let start = chrono::Local
        .from_local_datetime(&now.date_naive().and_hms_opt(0, 0, 0).unwrap())
        .earliest()
        .unwrap()
        .timestamp_millis();
    let end = chrono::Local
        .from_local_datetime(
            &now.date_naive()
                .succ_opt()
                .unwrap()
                .and_hms_opt(0, 0, 0)
                .unwrap(),
        )
        .earliest()
        .unwrap()
        .timestamp_millis();
    for (label, query) in [
        (
            "today",
            usage::summary::SummaryQuery {
                from: Some(start),
                to: Some(end),
                bounds: (start..end)
                    .step_by(3600000)
                    .chain(std::iter::once(end))
                    .collect(),
            },
        ),
        ("all", Default::default()),
    ] {
        let started = Instant::now();
        let result = usage::summary::read_from(&mut store, query, &|| false).unwrap();
        println!(
            "{}",
            serde_json::json!({"query":label,"query_ms":started.elapsed().as_secs_f64()*1000.,"groups":result.rows.len(),"token_total":result.rows.iter().map(|r|r.usage.total_tokens).sum::<u64>()})
        );
    }
}
