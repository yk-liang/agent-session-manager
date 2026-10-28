//! 当前 Agent 的原生文件事件：不读取日志正文、不轮询，后台时停止向界面发送通知。
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone)]
pub struct WatchSpec {
    agent: String,
    root: PathBuf,
    paths: crate::agents::AgentWatchPaths,
    all_usage: bool,
    members: Vec<WatchSpec>,
}

impl WatchSpec {
    pub(crate) fn for_usage(agent: &str, root: PathBuf) -> Self {
        Self::registered(agent, root, true)
    }

    pub fn new(agent: &str, root: PathBuf) -> Self {
        Self::registered(agent, root, false)
    }

    fn registered(agent: &str, root: PathBuf, usage: bool) -> Self {
        let paths = crate::agents::find_source(agent)
            .map(|source| source.watch_paths(usage))
            .unwrap_or_default();
        Self::from_paths(agent, root, paths, usage)
    }

    pub fn from_paths(
        agent: &str,
        root: PathBuf,
        paths: crate::agents::AgentWatchPaths,
        all_usage: bool,
    ) -> Self {
        let root = root.canonicalize().unwrap_or(root);
        Self {
            agent: agent.to_string(),
            root,
            paths,
            all_usage,
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
        event.paths.iter().any(|path| {
            if path == &self.root {
                return true;
            }
            for directory in &self.paths.transcript_dirs {
                let tree = self.root.join(directory);
                if path == &tree {
                    return true;
                }
                if let Ok(relative) = path.strip_prefix(&tree) {
                    let depth = relative.components().count();
                    if self.paths.max_depth.is_some_and(|max| depth > max) {
                        continue;
                    }
                    if self.all_usage
                        && relative
                            .components()
                            .any(|c| c.as_os_str() == "subagent-artifacts")
                    {
                        return false;
                    }
                    return path
                        .extension()
                        .is_some_and(|ext| ext == "jsonl" || self.all_usage && ext == "zst")
                        || path.is_dir()
                        || event.kind.is_remove() && path.extension().is_none();
                }
            }
            if path.parent() != Some(self.root.as_path()) {
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

    pub(crate) fn targets(&self) -> Vec<(PathBuf, RecursiveMode)> {
        if !self.members.is_empty() {
            let mut seen = std::collections::HashSet::new();
            return self
                .members
                .iter()
                .flat_map(|s| s.targets())
                .filter(|(p, _)| seen.insert(p.clone()))
                .collect();
        }
        // 用量关注五个数据根，递归元数据通知也能捕获后创建的 transcripts 目录；
        // relevant 仍过滤配置/凭据/请求日志，不读取它们的内容。
        if self.all_usage {
            return vec![(self.root.clone(), RecursiveMode::Recursive)];
        }
        let mut targets = vec![(self.root.clone(), RecursiveMode::NonRecursive)];
        for directory in &self.paths.transcript_dirs {
            let tree = self.root.join(directory);
            if tree.is_dir() {
                targets.push((tree, RecursiveMode::Recursive));
            }
        }
        targets
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

#[derive(Default)]
struct WatchState {
    generation: u64,
    watcher: Option<RecommendedWatcher>,
}

#[derive(Clone)]
pub struct SessionWatcher {
    state: Arc<Mutex<WatchState>>,
    generation: Arc<AtomicU64>,
    foreground: Arc<AtomicBool>,
}

impl Default for SessionWatcher {
    fn default() -> Self {
        Self {
            state: Arc::new(Mutex::new(WatchState::default())),
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
                if agent == "usage" {
                    let mut members = Vec::new();
                    for source in crate::agents::all_sources() {
                        if let Some(root) = source.data_dir().filter(|p| p.is_dir()) {
                            members.push(WatchSpec::from_paths(
                                source.id(),
                                root,
                                source.watch_paths(true),
                                true,
                            ));
                        }
                    }
                    if members.is_empty() {
                        return Ok(None);
                    }
                    return Ok(Some(WatchSpec {
                        agent,
                        root: PathBuf::new(),
                        paths: Default::default(),
                        all_usage: true,
                        members,
                    }));
                }
                let source = crate::agents::find_source(&agent)?;
                let root = source.data_dir().ok_or("缺少会话数据目录")?;
                Ok::<_, String>(Some(WatchSpec::from_paths(
                    &agent,
                    root,
                    source.watch_paths(false),
                    false,
                )))
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
        if generation < state.generation {
            return Ok(());
        }
        state.generation = generation;
        self.generation.store(generation, Ordering::SeqCst);
        state.watcher = None;
        let Some(spec) = spec else {
            return Ok(());
        };
        let targets = spec.targets();
        let active_generation = self.generation.clone();
        let foreground = self.foreground.clone();
        let mut last_signal: Option<Instant> = None;
        let mut error_sent = false;
        let mut watcher = notify::recommended_watcher(move |result: notify::Result<Event>| {
            if active_generation.load(Ordering::SeqCst) != generation
                || !foreground.load(Ordering::SeqCst)
            {
                return;
            }
            let error = match result {
                Ok(event) if spec.relevant(&event) => None,
                Ok(_) => return,
                Err(error) if !error_sent => {
                    error_sent = true;
                    Some(error.to_string())
                }
                Err(_) => return,
            };
            // 原生事件也可能成批涌入；最多每 400ms 通知一次，正文仍由界面安静期后检查。
            if error.is_none()
                && last_signal.is_some_and(|last| last.elapsed() < Duration::from_millis(400))
            {
                return;
            }
            last_signal = Some(Instant::now());
            emit(ChangeSignal {
                agent: spec.agent.clone(),
                generation,
                error,
            });
        })
        .map_err(|error| format!("无法启动会话监听：{error}"))?;
        for (path, recursive) in targets {
            watcher
                .watch(&path, recursive)
                .map_err(|error| format!("无法监听会话目录：{error}"))?;
        }
        state.watcher = Some(watcher);
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
    fn usage_scope_observes_hidden_archived_and_compressed_records_without_private_sidecars() {
        let root = PathBuf::from("/tmp/asm-usage-watch");
        let usage = WatchSpec {
            agent: "usage".into(),
            root: PathBuf::new(),
            paths: Default::default(),
            all_usage: true,
            members: vec![
                WatchSpec::for_usage("claude", root.join("claude")),
                WatchSpec::for_usage("codex", root.join("codex")),
                WatchSpec::for_usage("opencode", root.join("opencode")),
            ],
        };
        assert!(usage.relevant(&change(root.join("claude/projects/p/subagents/a.jsonl"))));
        assert!(usage.relevant(&change(root.join("codex/archived_sessions/a.jsonl.zst"))));
        assert!(usage.relevant(&change(root.join("codex/sessions/2099/01/01/a.jsonl"))));
        assert!(!usage.relevant(&change(
            root.join("claude/projects/p/subagent-artifacts/a.jsonl")
        )));
        assert!(usage.relevant(&change(root.join("opencode/opencode.db-wal"))));
        assert!(usage.relevant(&change(root.join("opencode/opencode-archived.db-wal"))));
        assert!(usage.relevant(&change(root.join("opencode/opencode-archived.db"))));
        assert!(!usage.relevant(&change(root.join("opencode/opencode-archived.db-shm"))));
        assert!(!usage.relevant(&change(root.join("opencode/opencode.db-shm"))));
        assert!(!usage.relevant(&change(root.join("claude/settings.json"))));
        assert!(!usage.relevant(&change(root.join("codex/auth.json"))));
    }

    #[test]
    fn paths_include_transcripts_and_indexes_but_exclude_noisy_or_private_sidecars() {
        let root = PathBuf::from("/tmp/asm-watch-test");
        let qoder = WatchSpec::new("qoder", root.clone());
        assert!(qoder.relevant(&change(root.join("projects/project/a.jsonl"))));
        assert!(!qoder.relevant(&change(root.join("logs/runs/run/qodercli.log"))));
        assert!(!qoder.relevant(&change(root.join("settings.json"))));
        assert!(!qoder.relevant(&change(root.join("projects/project/subagents/a.jsonl"))));
        let codex = WatchSpec::new("codex", root.clone());
        for path in [
            "sessions/2099/01/01/a.jsonl",
            "session_index.jsonl",
            ".codex-global-state.json",
        ] {
            assert!(codex.relevant(&change(root.join(path))));
        }
        assert!(!codex.relevant(&change(root.join("auth.json"))));
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
    }

    #[test]
    fn watcher_matches_adapter_descriptions_without_knowing_agent_ids() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::create_dir(root.join("records")).unwrap();
        let mut paths = crate::agents::AgentWatchPaths::transcripts("records", Some(2));
        paths.root_files.push("catalog.sqlite");
        let spec = WatchSpec::from_paths("custom-agent", root.clone(), paths, false);
        assert!(spec.relevant(&change(root.join("records/project/a.jsonl"))));
        assert!(!spec.relevant(&change(root.join("records/project/children/a.jsonl"))));
        assert!(spec.relevant(&change(root.join("catalog.sqlite"))));
        assert!(!spec.relevant(&change(root.join("session_index.jsonl"))));
        assert!(spec
            .targets()
            .contains(&(root.join("records"), RecursiveMode::Recursive)));
        assert!(
            WatchSpec::from_paths("custom-agent", root.clone(), Default::default(), true)
                .targets()
                .contains(&(root, RecursiveMode::Recursive))
        );
    }

    #[test]
    fn native_events_observe_new_directories_and_appends_and_stop_after_unwatch() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("projects")).unwrap();
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
        std::fs::create_dir(&project).unwrap();
        let file = project.join("aaaa.jsonl");
        std::fs::write(&file, "{}\n").unwrap();
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)).unwrap().agent,
            "qoder"
        );
        std::thread::sleep(Duration::from_millis(450));
        while rx.try_recv().is_ok() {}
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
        assert!(watch.state.lock().unwrap().watcher.is_none());
    }
}
