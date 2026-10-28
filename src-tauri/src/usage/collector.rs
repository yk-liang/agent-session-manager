//! 无窗口的用量采集进程：文件事件驱动，变化来源立即入账，报表按需汇总。
use super::{
    ingest,
    service::{self, ProcessLock, RuntimeStatus, Settings},
    Store,
};
use notify::{Event, RecursiveMode, Watcher};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, SyncSender},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

static STOPPED: AtomicBool = AtomicBool::new(false);
extern "C" fn stop(_: libc::c_int) {
    STOPPED.store(true, Ordering::Relaxed);
}
const COALESCE: Duration = Duration::from_millis(150);
const RECONCILE: Duration = Duration::from_secs(60);

#[derive(Clone, Default)]
enum Changes {
    All,
    #[default]
    Empty,
    Paths(BTreeSet<PathBuf>),
}
impl Changes {
    fn merge(&mut self, other: Self) {
        match (&mut *self, other) {
            (Self::All, _) | (_, Self::Empty) => {}
            (_, Self::All) => *self = Self::All,
            (Self::Empty, paths) => *self = paths,
            (Self::Paths(current), Self::Paths(incoming)) => {
                current.extend(incoming);
                // 即使有大量创建/删除，内存与通知队列也保持有界。
                if current.len() > 256 {
                    *self = Self::All;
                }
            }
        }
    }
}
#[derive(Default)]
struct Inbox {
    changes: BTreeMap<String, Changes>,
    check: bool,
}
fn wake(sender: &SyncSender<()>) {
    let _ = sender.try_send(());
}
fn relevant(agent: &str, roots: &[PathBuf], event: &Event) -> Changes {
    if event.kind.is_access() {
        return Changes::Empty;
    }
    if event.need_rescan() {
        return Changes::All;
    }
    let mut paths = BTreeSet::new();
    for path in &event.paths {
        for root in roots {
            // 根目录首次创建、移走或删除时重新注册监听；不递归监听整个 Home。
            if path == root || root.starts_with(path) {
                return Changes::All;
            }
            if !path.starts_with(root) {
                continue;
            }
            if path.is_dir() {
                return Changes::All;
            }
            if path
                .components()
                .any(|p| p.as_os_str() == "subagent-artifacts")
            {
                continue;
            }
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            if agent == "opencode" {
                if path.parent() != Some(root) {
                    continue;
                }
                if name.starts_with("opencode") && name.ends_with(".db") {
                    paths.insert(path.clone());
                } else if name.starts_with("opencode") && name.ends_with(".db-wal") {
                    paths.insert(path.with_file_name(name.trim_end_matches("-wal")));
                }
            } else if name.ends_with(".jsonl") || agent == "codex" && name.ends_with(".jsonl.zst") {
                paths.insert(path.clone());
            }
        }
    }
    if paths.is_empty() {
        Changes::Empty
    } else {
        Changes::Paths(paths)
    }
}

fn watch(
    dir: &Path,
    settings: &Settings,
    inbox: Arc<Mutex<Inbox>>,
    sender: SyncSender<()>,
) -> Result<notify::RecommendedWatcher, String> {
    // FSEvents 返回物理路径（例如 /private/var），缓存仍使用 Adapter 的原始路径。
    // 连尚未创建的根也解析其最近已有祖先，避免首次创建时漏掉通知。
    fn physical(path: &Path) -> PathBuf {
        for parent in path.ancestors() {
            if let Ok(base) = parent.canonicalize() {
                return base.join(path.strip_prefix(parent).unwrap());
            }
        }
        path.to_owned()
    }
    let roots: BTreeMap<_, Vec<_>> = settings
        .roots
        .iter()
        .map(|(id, roots)| {
            (
                id.clone(),
                roots
                    .iter()
                    .map(|root| (root.clone(), physical(root)))
                    .collect(),
            )
        })
        .collect();
    let control = physical(dir);
    let mut watcher = notify::recommended_watcher(move |result: notify::Result<Event>| {
        let mut queue = inbox.lock().unwrap_or_else(|e| e.into_inner());
        match result {
            Ok(event) => {
                if event.kind.is_access() {
                    return;
                }
                if event.paths.iter().any(|p| {
                    [control.join("settings.json"), control.join("check.json")].contains(p)
                }) {
                    queue.check = true;
                }
                for (id, mappings) in &roots {
                    let paths: Vec<_> = mappings.iter().map(|(_, p)| p.clone()).collect();
                    let mut changes = relevant(id, &paths, &event);
                    if let Changes::Paths(found) = changes {
                        changes = Changes::Paths(
                            found
                                .iter()
                                .filter_map(|path| {
                                    mappings.iter().find_map(|(original, physical)| {
                                        path.strip_prefix(physical)
                                            .ok()
                                            .map(|relative| original.join(relative))
                                    })
                                })
                                .collect(),
                        );
                    }
                    if !matches!(changes, Changes::Empty) {
                        queue.check |= event
                            .paths
                            .iter()
                            .any(|path| paths.iter().any(|root| root.starts_with(path)));
                        queue.changes.entry(id.clone()).or_default().merge(changes);
                    }
                }
            }
            Err(_) => queue.check = true,
        }
        if queue.check || !queue.changes.is_empty() {
            wake(&sender);
        }
    })
    .map_err(|_| "无法创建后台日志监听")?;
    watcher
        .watch(dir, RecursiveMode::NonRecursive)
        .map_err(|_| "无法监听后台服务控制目录")?;
    let mut targets = BTreeMap::new();
    for root in settings.roots.values().flatten() {
        let mut target = root.as_path();
        while !target.is_dir() {
            target = target.parent().ok_or("无法定位日志监听目录")?;
        }
        let recursive = target == root;
        targets
            .entry(target.to_owned())
            .and_modify(|value| *value |= recursive)
            .or_insert(recursive);
    }
    for (path, recursive) in targets {
        watcher
            .watch(
                &path,
                if recursive {
                    RecursiveMode::Recursive
                } else {
                    RecursiveMode::NonRecursive
                },
            )
            .map_err(|_| "部分日志目录无法监听，将自动重试")?;
    }
    Ok(watcher)
}

struct Job {
    changes: Changes,
    due: Instant,
    failures: u32,
}
#[derive(Default)]
struct Pending(BTreeMap<String, Job>);
impl Pending {
    fn changed(&mut self, id: String, changes: Changes, now: Instant) {
        let job = self.0.entry(id).or_insert(Job {
            changes: Changes::Empty,
            due: now + COALESCE,
            failures: 0,
        });
        job.changes.merge(changes);
        // 不延长首次通知的期限，持续流式写入也会及时入账。
        if job.failures == 0 {
            job.due = job.due.min(now + COALESCE);
        }
    }
    fn all(&mut self, settings: &Settings, now: Instant) {
        for id in settings.roots.keys() {
            self.changed(id.clone(), Changes::All, now);
        }
    }
    fn due(&self, now: Instant) -> Option<String> {
        self.0
            .iter()
            .filter(|(_, job)| job.due <= now)
            .min_by_key(|(id, job)| (job.due, id.as_str() == "codex"))
            .map(|(id, _)| id.clone())
    }
    fn wait(&self, now: Instant) -> Duration {
        self.0
            .values()
            .map(|job| job.due.saturating_duration_since(now))
            .min()
            .unwrap_or(RECONCILE)
            .min(RECONCILE)
    }
    fn retry(&mut self, id: String, mut job: Job, now: Instant) {
        job.due = now + Duration::from_secs((1u64 << job.failures.min(6)).min(60));
        job.failures = job.failures.saturating_add(1);
        self.0.insert(id, job);
    }
}

/// 只在明确的命令行模式运行；在 Tauri/WebView 初始化前分流。
pub fn run_if_requested() -> Option<i32> {
    let args: Vec<_> = std::env::args_os().collect();
    if args.get(1).is_none_or(|s| s != "--usage-collector") {
        return None;
    }
    let isolated = args
        .windows(2)
        .find(|a| a[0] == "--collector-directory")
        .map(|a| PathBuf::from(&a[1]));
    let dir = match isolated.clone().map(Ok).unwrap_or_else(service::directory) {
        Ok(dir) if dir.is_absolute() => dir,
        _ => {
            eprintln!("Invalid collector directory");
            return Some(1);
        }
    };
    match run(&dir, isolated.is_some()) {
        Ok(()) => Some(0),
        Err(_) => {
            eprintln!("Token collector failed; inspect the usage page for service status");
            Some(1)
        }
    }
}
fn run(dir: &Path, isolated: bool) -> Result<(), String> {
    let Some(_lock) = ProcessLock::acquire(dir)? else {
        return Ok(());
    };
    STOPPED.store(false, Ordering::Relaxed);
    unsafe {
        libc::signal(libc::SIGTERM, stop as *const () as libc::sighandler_t);
        libc::signal(libc::SIGINT, stop as *const () as libc::sighandler_t);
        libc::setpriority(libc::PRIO_PROCESS, 0, 10);
    }
    let mut settings = Settings::load(dir)?;
    if !settings.enabled {
        return Ok(());
    }
    if settings.roots.iter().any(|(id, paths)| {
        !super::AGENTS.contains(&id.as_str()) || paths.iter().any(|p| !p.is_absolute())
    }) {
        return Err("后台采集目录配置无效".into());
    }
    let ledger = if isolated {
        dir.join("ledger.sqlite3")
    } else {
        dirs::data_dir()
            .ok_or("缺少应用数据目录")?
            .join("Agent Session Manager/usage-ledger-v1.sqlite3")
    };
    let cache = if isolated {
        dir.join("cache.sqlite3")
    } else {
        dirs::cache_dir()
            .ok_or("缺少缓存目录")?
            .join("com.liangyukun.agent-session-manager/token-usage-v1.sqlite3")
    };
    // 单个 Store 即可处理所有来源；不在内存中常驻完整账本或所有历史报表。
    let mut store = Store::open_with_ledger(Some(&cache), Some(&ledger));
    let inbox = Arc::new(Mutex::new(Inbox::default()));
    let (sender, receiver) = mpsc::sync_channel(1);
    let mut watcher = watch(dir, &settings, inbox.clone(), sender.clone());
    let mut status = RuntimeStatus {
        pid: std::process::id(),
        version: env!("CARGO_PKG_VERSION").into(),
        ..Default::default()
    };
    service::write_json(dir, "status.json", &status)?;
    let mut pending = Pending::default();
    pending.all(&settings, Instant::now());
    let mut reconcile = Instant::now() + RECONCILE;
    while !STOPPED.load(Ordering::Relaxed) {
        let queued = std::mem::take(&mut *inbox.lock().unwrap_or_else(|e| e.into_inner()));
        if queued.check || Instant::now() >= reconcile {
            let next = Settings::load(dir)?;
            if !next.enabled {
                break;
            }
            settings = next;
            watcher = watch(dir, &settings, inbox.clone(), sender.clone());
            pending.all(&settings, Instant::now());
            reconcile = Instant::now() + RECONCILE;
        }
        for (id, changes) in queued.changes {
            pending.changed(id, changes, Instant::now());
        }
        if let Some(id) = pending.due(Instant::now()) {
            let job = pending.0.remove(&id).unwrap();
            let Some(roots) = settings.roots.get(&id) else {
                continue;
            };
            let paths = match &job.changes {
                Changes::Paths(paths) => Some(paths),
                _ => None,
            };
            let started = Instant::now();
            match ingest::collect(&id, roots, &mut store, paths, &|| {
                STOPPED.load(Ordering::Relaxed)
            }) {
                Ok(result) => {
                    status
                        .collected_at
                        .insert(id.clone(), chrono::Utc::now().timestamp_millis());
                    status.errors.remove(&id);
                    status.scan_ms = started.elapsed().as_secs_f64() * 1000.;
                    status.bytes_read = result.bytes_read;
                }
                Err(_) if STOPPED.load(Ordering::Relaxed) => break,
                Err(_) => {
                    eprintln!("Token collection failed for {id}; retry scheduled");
                    status
                        .errors
                        .insert(id.clone(), "来源采集失败，正在重试".into());
                    pending.retry(id, job, Instant::now());
                }
            }
            if let Err(error) = &watcher {
                status.errors.insert("watcher".into(), error.clone());
            } else {
                status.errors.remove("watcher");
            }
            service::write_json(dir, "status.json", &status)?;
            continue;
        }
        let wait = pending
            .wait(Instant::now())
            .min(reconcile.saturating_duration_since(Instant::now()))
            .min(Duration::from_secs(1));
        let _ = receiver.recv_timeout(wait);
    }
    drop(watcher);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::{
        event::{CreateKind, ModifyKind},
        EventKind,
    };
    #[test]
    fn streaming_changes_do_not_postpone_capture_and_queue_is_bounded() {
        let now = Instant::now();
        let mut pending = Pending::default();
        for i in 0..100 {
            pending.changed(
                "claude".into(),
                Changes::Paths(BTreeSet::from([PathBuf::from(format!("{i}"))])),
                now + Duration::from_millis(i),
            );
        }
        assert_eq!(pending.due(now + COALESCE).as_deref(), Some("claude"));
        let mut changes = Changes::Empty;
        for i in 0..1000 {
            changes.merge(Changes::Paths(BTreeSet::from([PathBuf::from(format!(
                "{i}"
            ))])));
        }
        assert!(matches!(changes, Changes::All));
    }
    #[test]
    fn source_events_filter_private_files_and_normalize_database_wal() {
        let root = PathBuf::from("/data");
        let event = |p: &str| Event::new(EventKind::Modify(ModifyKind::Any)).add_path(root.join(p));
        assert!(
            matches!(relevant("opencode",&[root.clone()],&event("opencode.db-wal")),Changes::Paths(paths) if paths==BTreeSet::from([root.join("opencode.db")]))
        );
        assert!(matches!(
            relevant("opencode", &[root.clone()], &event("opencode.db-shm")),
            Changes::Empty
        ));
        assert!(matches!(
            relevant("claude", &[root.clone()], &event("settings.json")),
            Changes::Empty
        ));
        assert!(matches!(
            relevant("pi", &[root.clone()], &event("subagent-artifacts/a.jsonl")),
            Changes::Empty
        ));
    }
    #[test]
    fn missing_root_is_observed_from_its_existing_parent() {
        let dir = tempfile::tempdir().unwrap();
        let control = dir.path().join("control");
        service::prepare_directory(&control).unwrap();
        let root = dir.path().join("new/projects");
        let settings = Settings {
            enabled: true,
            roots: BTreeMap::from([("claude".into(), vec![root.clone()])]),
        };
        let inbox = Arc::new(Mutex::new(Inbox::default()));
        let (tx, rx) = mpsc::sync_channel(1);
        let _watcher = watch(&control, &settings, inbox.clone(), tx).unwrap();
        std::fs::create_dir_all(&root).unwrap();
        rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(inbox.lock().unwrap().changes.contains_key("claude"));
        let event =
            Event::new(EventKind::Create(CreateKind::Folder)).add_path(dir.path().join("new"));
        assert!(matches!(relevant("claude", &[root], &event), Changes::All));
    }
    #[test]
    fn failures_retry_without_new_events() {
        let mut pending = Pending::default();
        let now = Instant::now();
        pending.changed("codex".into(), Changes::All, now);
        for seconds in [1, 2, 4, 8, 16, 32, 60, 60] {
            let job = pending.0.remove("codex").unwrap();
            pending.retry("codex".into(), job, now);
            assert_eq!(pending.wait(now), Duration::from_secs(seconds));
            assert_eq!(
                pending.due(now + Duration::from_secs(seconds)).as_deref(),
                Some("codex")
            );
        }
    }
}
