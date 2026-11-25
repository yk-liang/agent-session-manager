//! 当前 Agent 的原生文件事件：不读取日志正文、不轮询，后台时停止向界面发送通知。
use crate::agents::workspace_groups::normalized;
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone)]
pub struct WatchSpec {
    agent: String,
    root: PathBuf,
    paths: crate::agents::AgentWatchPaths,
    members: Vec<WatchSpec>,
}

impl WatchSpec {
    #[allow(dead_code)] // 供独立监听示例使用。
    pub fn new(agent: &str, root: PathBuf) -> Self {
        let paths = crate::agents::find_source(agent)
            .map(|source| source.watch_paths())
            .unwrap_or_default();
        Self::from_paths(agent, root, paths)
    }

    pub fn from_paths(agent: &str, root: PathBuf, paths: crate::agents::AgentWatchPaths) -> Self {
        let root = std::path::absolute(&root).unwrap_or(root);
        let root = root.ancestors().find_map(|ancestor| {
            let canonical = ancestor.canonicalize().ok()?;
            Some(canonical.join(root.strip_prefix(ancestor).ok()?))
        }).unwrap_or(root);
        Self {
            agent: agent.to_string(),
            root,
            paths,
            members: vec![],
        }
    }

    pub(crate) fn relevant(&self, event: &Event) -> bool {
        if !self.members.is_empty() {
            return self.members.iter().any(|member| member.relevant(event));
        }
        if event.kind.is_access() {
            return false;
        }
        if event.need_rescan() {
            return true;
        }
        let root = normalized(&self.root);
        event.paths.iter().any(|path| {
            let path = normalized(path);
            if root.starts_with(&path) {
                return true;
            }
            for directory in &self.paths.transcript_dirs {
                let tree = root.join(directory);
                if path == tree {
                    return true;
                }
                if let Ok(relative) = path.strip_prefix(&tree) {
                    let depth = relative.components().count();
                    if self.paths.max_depth.is_some_and(|max| depth > max) {
                        continue;
                    }
                    return path.extension().is_some_and(|ext| ext == "jsonl")
                        || path
                            .file_name()
                            .and_then(|name| name.to_str())
                            .is_some_and(|name| {
                                self.paths
                                    .extra_transcript_suffixes
                                    .iter()
                                    .any(|suffix| name.ends_with(suffix))
                            })
                        || path.is_dir()
                        || event.kind.is_remove() && path.extension().is_none();
                }
            }
            if path.parent() != Some(root.as_path()) {
                return false;
            }
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            // 仅匹配 Adapter 声明的文件；SHM/读取事件不能形成查询反馈循环。
            self.paths.root_files.contains(&name)
                || self
                    .paths
                    .root_file_patterns
                    .iter()
                    .any(|(prefix, suffix)| name.starts_with(prefix) && name.ends_with(suffix))
        })
    }

    pub(crate) fn targets(&self) -> BTreeMap<PathBuf, RecursiveMode> {
        let mut targets = BTreeMap::new();
        self.add_targets(&mut targets);
        targets
    }

    fn add_targets(&self, targets: &mut BTreeMap<PathBuf, RecursiveMode>) {
        if !self.members.is_empty() {
            for member in &self.members {
                member.add_targets(targets);
            }
            return;
        }

        if let Some(parent) = self.root.ancestors().find(|path| path.is_dir()) {
            targets.entry(parent.to_path_buf()).or_insert(RecursiveMode::NonRecursive);
        }
        for directory in &self.paths.transcript_dirs {
            let tree = self.root.join(directory);
            if tree.is_dir() {
                targets.insert(tree, RecursiveMode::Recursive);
            }
        }
    }

    fn registration_changed(&self, event: &Event) -> bool {
        if !self.members.is_empty() {
            return self.members.iter().any(|member| member.registration_changed(event));
        }
        let root = normalized(&self.root);
        event.need_rescan() || event.paths.iter().any(|path| {
            let path = normalized(path);
            root.starts_with(&path) || self.paths.transcript_dirs.iter()
                .any(|directory| root.join(directory) == path)
        })
    }

}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeSignal {
    pub agent: String,
    pub generation: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

enum WatchMessage {
    Event(notify::Result<Event>),
    Stop,
}

struct WatchWorker(mpsc::Sender<WatchMessage>);

impl Drop for WatchWorker {
    fn drop(&mut self) {
        let _ = self.0.send(WatchMessage::Stop);
    }
}

fn register_targets(
    watcher: &mut RecommendedWatcher,
    current: &mut BTreeMap<PathBuf, RecursiveMode>,
    desired: BTreeMap<PathBuf, RecursiveMode>,
) -> Option<String> {
    current.retain(|path, mode| {
        if desired.get(path) == Some(mode) {
            return true;
        }
        let _ = watcher.unwatch(path);
        false
    });
    let mut warning = None;
    for (path, mode) in desired {
        if current.get(&path) == Some(&mode) {
            continue;
        }
        match watcher.watch(&path, mode) {
            Ok(()) => { current.insert(path, mode); }
            Err(error) => warning = Some(format!("无法监听会话目录 {}：{error}", path.display())),
        }
    }
    warning
}

#[derive(Clone)]
pub struct SessionWatcher {
    state: Arc<Mutex<Option<WatchWorker>>>,
    generation: Arc<AtomicU64>,
    foreground: Arc<AtomicBool>,
}

impl Default for SessionWatcher {
    fn default() -> Self {
        Self {
            state: Arc::new(Mutex::new(None)),
            generation: Arc::new(AtomicU64::new(0)),
            foreground: Arc::new(AtomicBool::new(true)),
        }
    }
}

impl SessionWatcher {
    pub fn set_foreground(&self, foreground: bool) {
        self.foreground.store(foreground, Ordering::SeqCst);
    }

    pub fn select(
        &self,
        agent: Option<String>,
        generation: u64,
        emit: impl Fn(ChangeSignal) + Send + Sync + 'static,
    ) -> Result<(), String> {
        let spec = agent
            .map(|agent| {
                let source = crate::agents::find_source(&agent)?;
                let mut members: Vec<_> = source.watch_locations().into_iter()
                    .map(|(root, paths)| WatchSpec::from_paths(source.id(), root, paths))
                    .collect();
                Ok::<_, String>(match members.len() {
                    0 => None,
                    1 => members.pop(),
                    _ => Some(WatchSpec {
                        agent,
                        root: PathBuf::new(),
                        paths: Default::default(),
                        members,
                    }),
                })
            })
            .transpose()?
            .flatten();
        self.select_spec(spec, generation, emit)
    }

    pub fn select_spec(
        &self,
        spec: Option<WatchSpec>,
        generation: u64,
        emit: impl Fn(ChangeSignal) + Send + Sync + 'static,
    ) -> Result<(), String> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        // IPC 在线程池中完成次序可能颠倒；过期的停止/启动请求不能覆盖最新选择。
        if generation < self.generation.load(Ordering::SeqCst) {
            return Ok(());
        }
        self.generation.store(generation, Ordering::SeqCst);
        *state = None;
        let Some(spec) = spec else {
            return Ok(());
        };
        let (tx, rx) = mpsc::channel();
        let event_tx = tx.clone();
        let mut watcher = notify::recommended_watcher(move |result| {
            let _ = event_tx.send(WatchMessage::Event(result));
        }).map_err(|error| format!("无法启动会话监听：{error}"))?;
        let mut registered = BTreeMap::new();
        if let Some(error) = register_targets(&mut watcher, &mut registered, spec.targets()) {
            if registered.is_empty() {
                return Err(error);
            }
            eprintln!("{error}");
        }

        let selection = self.generation.clone();
        let foreground = self.foreground.clone();
        std::thread::Builder::new().name("session-watch".into()).spawn(move || {
            let interval = Duration::from_millis(400);
            let mut last_signal: Option<Instant> = None;
            let mut pending = false;
            let mut error_sent = false;
            let send = |error| {
                if foreground.load(Ordering::SeqCst) && selection.load(Ordering::SeqCst) == generation {
                    emit(ChangeSignal { agent: spec.agent.clone(), generation, error });
                }
            };
            loop {
                if selection.load(Ordering::SeqCst) != generation {
                    break;
                }
                let message = if pending {
                    let remaining = interval.saturating_sub(last_signal.unwrap().elapsed());
                    if remaining.is_zero() {
                        send(None);
                        last_signal = Some(Instant::now());
                        pending = false;
                        continue;
                    }
                    rx.recv_timeout(remaining)
                } else {
                    rx.recv().map_err(|_| mpsc::RecvTimeoutError::Disconnected)
                };
                match message {
                    Ok(WatchMessage::Event(Ok(event))) if spec.relevant(&event) => {
                        if spec.registration_changed(&event) {
                            if let Some(error) = register_targets(&mut watcher, &mut registered, spec.targets()) {
                                if !error_sent {
                                    send(Some(error));
                                    error_sent = true;
                                }
                            }
                        }
                        if !foreground.load(Ordering::SeqCst) {
                            continue;
                        }
                        if last_signal.is_none_or(|last| last.elapsed() >= interval) {
                            send(None);
                            last_signal = Some(Instant::now());
                            pending = false;
                        } else {
                            pending = true;
                        }
                    }
                    Ok(WatchMessage::Event(Err(error))) if !error_sent => {
                        send(Some(error.to_string()));
                        error_sent = true;
                    }
                    Ok(WatchMessage::Event(_)) | Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Ok(WatchMessage::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
        }).map_err(|error| format!("无法启动会话监听线程：{error}"))?;
        *state = Some(WatchWorker(tx));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{AccessKind, CreateKind, ModifyKind};

    fn change(path: PathBuf) -> Event {
        Event::new(notify::EventKind::Modify(ModifyKind::Any)).add_path(path)
    }

    #[test]
    fn paths_include_transcripts_and_indexes_but_exclude_noisy_or_private_sidecars() {
        let root = PathBuf::from("/tmp/asm-watch-test");
        let qoder = WatchSpec::new("qoder", root.clone());
        assert!(qoder.relevant(&change(root.join("projects/project/a.jsonl"))));
        assert!(!qoder.relevant(&change(root.join("logs/runs/run/qodercli.log"))));
        assert!(!qoder.relevant(&change(root.join("settings.json"))));
        for agent in ["claude", "qoder"] {
            let family = WatchSpec::new(agent, root.clone());
            for path in [
                "projects/project/aaaa/subagents/agent-bbbb.jsonl",
                "projects/project/aaaa/subagents/agent-bbbb.meta.json",
            ] {
                assert!(family.relevant(&change(root.join(path))));
            }
            assert!(!family.relevant(&change(root.join("projects/project/aaaa/subagents/private.json"))));
        }
        let codex = WatchSpec::new("codex", root.clone());
        for path in [
            "sessions/2099/01/01/a.jsonl",
            "sessions/2099/01/01/a.jsonl.zst",
            "archived_sessions/a.jsonl.zst",
            "session_index.jsonl",
            ".codex-global-state.json",
            "state_5.sqlite-wal",
        ] {
            assert!(codex.relevant(&change(root.join(path))));
        }
        assert!(!codex.relevant(&change(root.join("auth.json"))));
        assert!(!codex.relevant(&change(root.join("sessions/2099/01/01/cache.zst"))));
        let sqlite = WatchSpec::new("opencode", root.clone());
        assert!(sqlite.relevant(&change(root.join("opencode.db-wal"))));
        assert!(!sqlite.relevant(&change(root.join("opencode.db-shm"))));
        assert!(!sqlite.relevant(
            &Event::new(notify::EventKind::Access(AccessKind::Read))
                .add_path(root.join("opencode.db"))
        ));
        let pi = WatchSpec::new("pi", root.clone());
        assert!(pi.relevant(
            &Event::new(notify::EventKind::Create(CreateKind::File))
                .add_path(root.join("sessions/project/a.jsonl"))
        ));
        assert!(pi.relevant(&change(root.join("sessions/project/a/run-id/run-0/session.jsonl"))));
    }

    #[test]
    fn watcher_matches_adapter_descriptions_without_knowing_agent_ids() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::create_dir(root.join("records")).unwrap();
        let mut paths = crate::agents::AgentWatchPaths::transcripts("records", Some(2));
        paths.root_files.push("catalog.sqlite");
        let spec = WatchSpec::from_paths("custom-agent", root.clone(), paths);
        assert!(spec.relevant(&change(root.join("records/project/a.jsonl"))));
        assert!(!spec.relevant(&change(root.join("records/project/children/a.jsonl"))));
        assert!(spec.relevant(&change(root.join("catalog.sqlite"))));
        assert!(!spec.relevant(&change(root.join("session_index.jsonl"))));
        assert!(spec
            .targets()
            .get(&root.join("records")) == Some(&RecursiveMode::Recursive));
        assert!(
            WatchSpec::from_paths("custom-agent", root.clone(), Default::default())
                .targets()
                .get(&root) == Some(&RecursiveMode::NonRecursive)
        );
    }

    #[test]
    fn native_events_observe_new_directories_and_appends_and_stop_after_unwatch() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let watch = SessionWatcher::default();
        let (tx, rx) = std::sync::mpsc::channel();
        watch
            .select_spec(
                Some(WatchSpec::new("qoder", dir.path().to_path_buf())),
                1,
                move |signal| {
                    let _ = tx.send(signal);
                },
            )
            .unwrap();
        let project = dir.path().join("projects/new-project");
        std::fs::create_dir_all(&project).unwrap();
        let file = project.join("aaaa.jsonl");
        std::fs::write(&file, "{}\n").unwrap();
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)).unwrap().agent,
            "qoder"
        );
        writeln!(
            std::fs::OpenOptions::new()
                .append(true)
                .open(&file)
                .unwrap(),
            "{{}}"
        )
        .unwrap();
        assert!(rx
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .error
            .is_none());
        watch.select_spec(None, 3, |_| {}).unwrap();
        // 颠倒到达的旧启动请求不能重新建立监听。
        watch
            .select_spec(
                Some(WatchSpec::new("qoder", dir.path().to_path_buf())),
                2,
                |_| panic!("stale watcher resumed"),
            )
            .unwrap();
        assert!(watch.state.lock().unwrap().is_none());
    }
}
