//! 临时目录上的原生监听资源复测，不写入用户会话目录。
#![allow(dead_code, unused_imports)]
#[path = "../src/agents/mod.rs"]
mod agents;
#[path = "../src/session_watch.rs"]
mod session_watch;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

fn usage() -> (f64, i64) {
    unsafe {
        let mut usage: libc::rusage = std::mem::zeroed();
        assert_eq!(libc::getrusage(libc::RUSAGE_SELF, &mut usage), 0);
        let cpu = usage.ru_utime.tv_sec as f64
            + usage.ru_utime.tv_usec as f64 / 1e6
            + usage.ru_stime.tv_sec as f64
            + usage.ru_stime.tv_usec as f64 / 1e6;
        (cpu, usage.ru_maxrss)
    }
}

fn main() {
    use std::io::Write;
    let dir = tempfile::tempdir().unwrap();
    let projects = dir.path().join("projects");
    std::fs::create_dir(&projects).unwrap();
    let signals = Arc::new(AtomicUsize::new(0));
    let count = signals.clone();
    let watcher = session_watch::SessionWatcher::default();
    let before = usage();
    watcher
        .select_spec(
            Some(session_watch::WatchSpec::new(
                "qoder",
                dir.path().to_path_buf(),
            )),
            1,
            move |_| {
                count.fetch_add(1, Ordering::SeqCst);
            },
        )
        .unwrap();
    std::thread::sleep(Duration::from_millis(300));
    signals.store(0, Ordering::SeqCst);
    let idle = usage();
    let started = Instant::now();
    std::thread::sleep(Duration::from_secs(10));
    let after_idle = usage();
    println!(
        "{}",
        serde_json::json!({"phase":"idle", "seconds":started.elapsed().as_secs_f64(),
        "cpu_ms":(after_idle.0-idle.0)*1000.0, "signals":signals.load(Ordering::SeqCst),
        "peak_rss_growth_bytes":after_idle.1-before.1})
    );
    let project = projects.join("new-project");
    std::fs::create_dir(&project).unwrap();
    let mut file = std::fs::File::create(project.join("aaaa.jsonl")).unwrap();
    let active = usage();
    for _ in 0..100 {
        writeln!(file, "{{}}").unwrap();
        std::thread::sleep(Duration::from_millis(10));
    }
    std::thread::sleep(Duration::from_millis(1500));
    let after_active = usage();
    println!(
        "{}",
        serde_json::json!({"phase":"100_writes", "cpu_ms":(after_active.0-active.0)*1000.0,
        "signals":signals.swap(0, Ordering::SeqCst)})
    );
    watcher.set_foreground(false);
    for _ in 0..100 {
        writeln!(file, "{{}}").unwrap();
    }
    std::thread::sleep(Duration::from_millis(500));
    println!(
        "{}",
        serde_json::json!({"phase":"background", "signals":signals.load(Ordering::SeqCst)})
    );
    watcher.select_spec(None, 2, |_| {}).unwrap();
}
