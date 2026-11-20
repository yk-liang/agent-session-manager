//! 监听数据目录，按变化的路径更新用量账本。
use super::{
    service::{self, ProcessLock, RuntimeStatus, Settings},
    ingest::{self, SyncProgress, Synced},
    Store,
};
use notify::{Event, RecursiveMode, Watcher};
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

static STOPPED: AtomicBool = AtomicBool::new(false);
extern "C" fn stop(_: libc::c_int) {
    STOPPED.store(true, Ordering::Relaxed);
}
mod desktop;
pub use desktop::Desktop;

const COALESCE: Duration = Duration::from_millis(150);
const RECONCILE: Duration = Duration::from_secs(60);
type Roots = BTreeMap<String, Vec<PathBuf>>;

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
    manual: bool,
    ledger_changed: bool,
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
    let input = super::source::Source::for_agent(agent);
    let mut paths = BTreeSet::new();
    for path in &event.paths {
        for root in roots {
            if root.starts_with(path) && (root != path || !input.is_file_root(root)) {
                return Changes::All;
            }
            if input.is_file_root(root) {
                paths.extend(input.changed_file(root, path));
                continue;
            }
            if !path.starts_with(root) {
                continue;
            }
            if path
                .components()
                .any(|p| p.as_os_str() == "subagent-artifacts")
            {
                continue;
            }
            if input.descends(path) && path.is_dir() {
                return Changes::All;
            }
            if let Some(path) = input.changed_file(root, path) {
                paths.insert(path);
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
    control_dir: Option<&Path>,
    roots: &Roots,
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
    let mappings: BTreeMap<_, Vec<_>> = roots
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
    let control = control_dir.map(physical);
    let mut watcher = notify::recommended_watcher(move |result: notify::Result<Event>| {
        let mut queue = inbox.lock().unwrap_or_else(|e| e.into_inner());
        match result {
            Ok(event) => {
                if event.kind.is_access() {
                    return;
                }
                if control.as_ref().is_some_and(|dir| {
                    event.paths.iter().any(|p| {
                        p.parent() == Some(dir.as_path())
                            && p.file_name()
                                .is_some_and(|name| name == "settings.json" || name == "check.json")
                    })
                }) {
                    queue.check = true;
                }
                for (id, mappings) in &mappings {
                    let input = super::source::Source::for_agent(id);
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
                                            .map(|relative| {
                                                if relative.as_os_str().is_empty() {
                                                    original.clone()
                                                } else {
                                                    original.join(relative)
                                                }
                                            })
                                    })
                                })
                                .collect(),
                        );
                    }
                    if !matches!(changes, Changes::Empty) {
                        queue.check |= event
                            .paths
                            .iter()
                            .any(|path| paths.iter().any(|root| root.starts_with(path)
                                && (root != path || !input.is_file_root(root))));
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
    .map_err(|_| "无法创建日志监听")?;
    if let Some(dir) = control_dir {
        watcher
            .watch(dir, RecursiveMode::NonRecursive)
            .map_err(|_| "无法监听后台服务控制目录")?;
    }
    let mut targets = BTreeMap::new();
    for root in roots.values().flatten() {
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
struct Pending {
    jobs: BTreeMap<String, Job>,
    next_read: BTreeMap<String, Instant>,
}
impl Pending {
    fn changed(&mut self, id: String, changes: Changes, now: Instant) {
        let due = self
            .next_read
            .get(&id)
            .copied()
            .unwrap_or(now)
            .max(now + COALESCE);
        let job = self.jobs.entry(id).or_insert(Job {
            changes: Changes::Empty,
            due,
            failures: 0,
        });
        job.changes.merge(changes);
        // 不延长首次通知的期限，持续流式写入也会及时入账。
        if job.failures == 0 {
            job.due = job.due.min(due);
        }
    }
    fn all(&mut self, roots: &Roots, now: Instant) {
        for id in roots.keys() {
            self.changed(id.clone(), Changes::All, now);
        }
    }
    fn due(&self, now: Instant) -> Option<String> {
        self.jobs
            .iter()
            .filter(|(_, job)| job.due <= now)
            .min_by_key(|(id, job)| (job.due, id.as_str() == "codex"))
            .map(|(id, _)| id.clone())
    }
    fn wait(&self, now: Instant) -> Duration {
        self.jobs
            .values()
            .map(|job| job.due.saturating_duration_since(now))
            .min()
            .unwrap_or(RECONCILE)
            .min(RECONCILE)
    }
    fn retry(&mut self, id: String, mut job: Job, now: Instant) {
        job.due = now + Duration::from_secs((1u64 << job.failures.min(6)).min(60));
        job.failures = job.failures.saturating_add(1);
        self.jobs.insert(id, job);
    }
    fn finished(&mut self, id: String, elapsed: Duration) {
        let rest = (elapsed * 4).clamp(Duration::from_secs(1), Duration::from_secs(15));
        self.next_read.insert(id, Instant::now() + rest);
    }
    fn syncing(&self) -> BTreeSet<String> {
        self.jobs
            .iter()
            .filter(|(_, job)| job.failures == 0)
            .map(|(id, _)| id.clone())
            .collect()
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
        Err(_) => Some(1),
    }
}
fn service_roots(dir: &Path) -> Result<Option<Roots>, String> {
    let settings = Settings::load(dir)?;
    if settings.roots.iter().any(|(id, paths)| {
        !super::AGENTS.contains(&id.as_str()) || paths.iter().any(|p| !p.is_absolute())
    }) {
        return Err("后台采集目录配置无效".into());
    }
    Ok(settings.enabled.then_some(settings.roots))
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
    let (ledger, cache) = if isolated {
        (dir.join("ledger.sqlite3"), dir.join("cache.sqlite3"))
    } else {
        (super::ledger::path()?, cache_path()?)
    };
    let mut status = RuntimeStatus {
        pid: std::process::id(),
        version: env!("CARGO_PKG_VERSION").into(),
        ..Default::default()
    };
    service::write_json(dir, "status.json", &status)?;
    let (queue, receiver) = Queue::new();
    Collection {
        control_dir: Some(dir),
        ledger,
        cache,
        queue,
        receiver: &receiver,
    }
    .run(
        || service_roots(dir),
        &|| STOPPED.load(Ordering::Relaxed),
        &mut SyncStatus::default(),
        &|_| {},
        |id, result, elapsed| {
            if let Ok(result) = result {
                status.collected_at.insert(id.into(), result.updated_at);
                status.scan_ms = elapsed.as_secs_f64() * 1000.;
                status.bytes_read = result.bytes_read;
                service::write_json(dir, "status.json", &status)?;
            }
            Ok(())
        },
    )
}

fn cache_path() -> Result<PathBuf, String> {
    super::cache::path().ok_or_else(|| "缺少缓存目录".into())
}

#[derive(Clone)]
struct Queue {
    inbox: Arc<Mutex<Inbox>>,
    sender: SyncSender<()>,
}
impl Queue {
    fn new() -> (Self, Receiver<()>) {
        let (sender, receiver) = mpsc::sync_channel(1);
        (
            Self {
                inbox: Arc::new(Mutex::new(Inbox::default())),
                sender,
            },
            receiver,
        )
    }

    fn check_now(&self) {
        self.inbox.lock().unwrap_or_else(|e| e.into_inner()).manual = true;
        wake(&self.sender);
    }

    fn ledger_changed(&self) {
        self.inbox.lock().unwrap_or_else(|e| e.into_inner()).ledger_changed = true;
        wake(&self.sender);
    }

    fn take(&self) -> Inbox {
        std::mem::take(&mut *self.inbox.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatus {
    pub sequence: u64,
    pub revision: u64,
    pub updated_at: Option<i64>,
    pub syncing: BTreeSet<String>,
    pub progress: Option<SyncProgress>,
    pub failed_agents: BTreeSet<String>,
    pub unavailable: bool,
}

struct Publication {
    indexed: i64,
    completed: i64,
    updated_at: Option<i64>,
}

impl Publication {
    fn update(&mut self, next: Self, status: &mut SyncStatus) {
        if next.indexed != self.indexed || next.completed != self.completed {
            status.updated_at = status.updated_at.max(next.updated_at);
            status.revision += 1;
        }
        *self = next;
    }

    fn read(store: &mut Store) -> Result<Self, String> {
        store
            .ledger()?
            .db
            .query_row(
                "SELECT coalesce((SELECT sum(indexed) FROM usage_request_index),0),
                    (SELECT count(*) FROM usage_sync_state),
                    (SELECT max(u.updated) FROM usage_ledger_updates u
                       JOIN usage_request_index i USING(agent) WHERE i.indexed=i.revision)",
                [],
                |r| {
                    Ok(Self {
                        indexed: r.get(0)?,
                        completed: r.get(1)?,
                        updated_at: r.get(2)?,
                    })
                },
            )
            .map_err(|_| "无法读取最新的用量记录版本".into())
    }
}

struct Collection<'a> {
    control_dir: Option<&'a Path>,
    ledger: PathBuf,
    cache: PathBuf,
    queue: Queue,
    receiver: &'a Receiver<()>,
}

impl Collection<'_> {
    fn run(
        &self,
        load_roots: impl Fn() -> Result<Option<Roots>, String>,
        cancelled: &dyn Fn() -> bool,
        status: &mut SyncStatus,
        on_state: &dyn Fn(&SyncStatus),
        mut on_finish: impl FnMut(
            &str,
            &Result<Synced, String>,
            Duration,
        ) -> Result<(), String>,
    ) -> Result<(), String> {
        let Some(mut roots) = load_roots()? else {
            return Ok(());
        };
        let create_watch = |roots: &Roots| {
            watch(
                self.control_dir,
                roots,
                self.queue.inbox.clone(),
                self.queue.sender.clone(),
            )
        };
        let mut watcher = create_watch(&roots);
        let mut pending = Pending::default();
        pending.all(&roots, Instant::now());
        let last_status = RefCell::new(status.clone());
        let publish = |status: &mut SyncStatus| {
            if *status != *last_status.borrow() {
                status.sequence += 1;
                on_state(status);
                last_status.replace(status.clone());
            }
        };
        status.unavailable = false;
        status.syncing = pending.syncing();
        publish(status);
        let mut store = {
            let _writer = super::ledger::SyncLock::acquire(&self.ledger, cancelled)?;
            Store::open_with_ledger(Some(&self.cache), Some(&self.ledger))
        };
        let mut publication = Publication::read(&mut store)?;
        if publication.updated_at > status.updated_at {
            status.updated_at = publication.updated_at;
            status.revision += 1;
            publish(status);
        }
        let status = RefCell::new(status);
        let mut reconcile = Instant::now() + RECONCILE;
        while !cancelled() {
            let queued = self.queue.take();
            let now = Instant::now();
            if queued.check || queued.manual || now >= reconcile {
                let Some(next) = load_roots()? else { break };
                if next != roots || queued.check || watcher.is_err() {
                    watcher = create_watch(&next);
                }
                roots = next;
                pending.jobs.retain(|id, _| roots.contains_key(id));
                pending.all(&roots, now);
                if queued.manual {
                    for job in pending.jobs.values_mut() {
                        job.failures = 0;
                        job.due = now;
                    }
                }
                reconcile = now + RECONCILE;
            }
            for (id, changes) in queued.changes {
                if roots.contains_key(&id) {
                    pending.changed(id, changes, now);
                }
            }
            {
                let mut state = status.borrow_mut();
                if queued.ledger_changed {
                    publication.update(Publication::read(&mut store)?, &mut state);
                }
                state.syncing = pending.syncing();
                publish(&mut state);
            }
            if let Some(id) = pending.due(Instant::now()) {
                let job = pending.jobs.remove(&id).unwrap();
                let paths = match &job.changes {
                    Changes::Paths(paths) => Some(paths),
                    _ => None,
                };
                let started = Instant::now();
                if self.control_dir.is_none() {
                    let mut state = status.borrow_mut();
                    state.syncing.insert(id.clone());
                    state.progress = Some(SyncProgress {
                        agent: id.clone(),
                        phase: "waiting",
                        checked_files: 0,
                        total_files: None,
                    });
                    publish(&mut state);
                }
                let last_progress = RefCell::new((started, "waiting"));
                let progress = |progress: SyncProgress| {
                    if self.control_dir.is_some() {
                        return;
                    }
                    let mut last = last_progress.borrow_mut();
                    if progress.phase == last.1 && last.0.elapsed() < Duration::from_millis(200) {
                        return;
                    }
                    *last = (Instant::now(), progress.phase);
                    let mut state = status.borrow_mut();
                    state.progress = Some(progress);
                    publish(&mut state);
                };
                let collected = (|| {
                    let _writer = super::ledger::SyncLock::acquire(&self.ledger, cancelled)?;
                    ingest::sync_with_store(
                        &id,
                        &roots[&id],
                        &mut store,
                        paths,
                        cancelled,
                        &progress,
                    )
                })();
                if cancelled() {
                    break;
                }
                let elapsed = started.elapsed();
                let next = Publication::read(&mut store)?;
                let mut state = status.borrow_mut();
                publication.update(next, &mut state);
                if collected.is_err() {
                    state.failed_agents.insert(id.clone());
                    pending.retry(id.clone(), job, Instant::now());
                } else {
                    state.failed_agents.remove(&id);
                    pending.finished(id.clone(), elapsed);
                }
                state.progress = None;
                state.syncing = pending.syncing();
                publish(&mut state);
                on_finish(&id, &collected, elapsed)?;
                continue;
            }
            let wait = pending
                .wait(Instant::now())
                .min(reconcile.saturating_duration_since(Instant::now()));
            let wait = if self.control_dir.is_some() {
                wait.min(Duration::from_secs(1))
            } else {
                wait
            };
            let _ = self.receiver.recv_timeout(wait);
        }
        Ok(())
    }
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
            relevant("antigravity", &[root.clone()], &event("session.db-wal")),
            Changes::Paths(paths) if paths == BTreeSet::from([root.join("session.db")])
        ));
        for name in [
            "session.db-shm",
            "conversation_summaries.db",
            "settings.json",
        ] {
            assert!(matches!(
                relevant("antigravity", &[root.clone()], &event(name)),
                Changes::Empty
            ));
        }
        assert!(matches!(
            relevant("claude", &[root.clone()], &event("settings.json")),
            Changes::Empty
        ));
        assert!(matches!(
            relevant("pi", &[root.clone()], &event("subagent-artifacts/a.jsonl")),
            Changes::Empty
        ));
        let history = root.join(crate::usage::codex::failures::DATABASE);
        assert!(matches!(
            relevant("codex", &[history.clone()], &event("thread_history_1.sqlite-wal")),
            Changes::Paths(paths) if paths == BTreeSet::from([history.clone()])
        ));
        assert!(matches!(
            relevant("codex", &[history], &event("thread_history_1.sqlite-shm")),
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
        let _watcher = watch(Some(&control), &settings.roots, inbox.clone(), tx).unwrap();
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
            let job = pending.jobs.remove("codex").unwrap();
            pending.retry("codex".into(), job, now);
            assert_eq!(pending.wait(now), Duration::from_secs(seconds));
            assert_eq!(
                pending.due(now + Duration::from_secs(seconds)).as_deref(),
                Some("codex")
            );
        }
    }

    #[test]
    fn collection_publishes_file_changes_and_keeps_the_update_time_on_empty_checks() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("logs");
        std::fs::create_dir(&root).unwrap();
        let path = root.join("session.jsonl");
        let line = |id| {
            serde_json::json!({
                "type":"assistant","sessionId":"session","timestamp":"2026-11-20T00:00:00Z",
                "message":{"id":id,"model":"fixture","usage":{"input_tokens":10,"output_tokens":1}}
            })
        };
        std::fs::write(&path, format!("{}\n", line("msg_initial"))).unwrap();
        let roots = Roots::from([("claude".into(), vec![root])]);
        let (queue, receiver) = Queue::new();
        let (publisher, published) = mpsc::channel();
        let stopped = AtomicBool::new(false);
        std::thread::scope(|scope| {
            let worker_queue = queue.clone();
            let cancelled = || stopped.load(Ordering::Relaxed);
            let data_dir = dir.path();
            let worker = scope.spawn(move || {
                Collection {
                    control_dir: None,
                    ledger: data_dir.join("ledger.sqlite3"),
                    cache: data_dir.join("cache.sqlite3"),
                    queue: worker_queue,
                    receiver: &receiver,
                }
                .run(
                    || Ok(Some(roots.clone())),
                    &cancelled,
                    &mut SyncStatus::default(),
                    &|state| {
                        let _ = publisher.send(state.clone());
                    },
                    |_, _, _| Ok(()),
                )
            });
            let verify = || -> Result<(), String> {
                let settled = |stage, after| {
                    let deadline = Instant::now() + Duration::from_secs(6);
                    loop {
                        let state = published
                            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                            .map_err(|err| format!("{stage}: {err}"))?;
                        if state.sequence > after && state.syncing.is_empty() && state.revision > 0
                        {
                            break Ok::<_, String>(state);
                        }
                    }
                };
                let initial = settled("initial import", 0)?;
                if initial.updated_at.is_none() || !initial.failed_agents.is_empty() {
                    return Err(format!("Unexpected first import status: {initial:?}"));
                }
                queue.check_now();
                let checked = settled("unchanged check", initial.sequence)?;
                if checked.revision != initial.revision || checked.updated_at != initial.updated_at
                {
                    return Err(format!("An unchanged check was published: {checked:?}"));
                }
                let mut file = std::fs::OpenOptions::new()
                    .append(true)
                    .open(&path)
                    .map_err(|e| e.to_string())?;
                writeln!(file, "{}", line("msg_appended")).map_err(|e| e.to_string())?;
                drop(file);
                let updated = settled("file notification", checked.sequence)?;
                if updated.revision <= checked.revision || updated.updated_at <= checked.updated_at
                {
                    return Err(format!("The append did not publish an update: {updated:?}"));
                }
                Ok(())
            };
            let result = verify();
            stopped.store(true, Ordering::Relaxed);
            wake(&queue.sender);
            worker.join().unwrap().unwrap();
            result.unwrap();
        });
    }
}
