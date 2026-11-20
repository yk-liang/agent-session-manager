use super::{cache_path, wake, Collection, Queue, Roots, SyncStatus, RECONCILE};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

pub struct Desktop {
    queue: Queue,
    stop: Arc<AtomicBool>,
    status: Arc<Mutex<SyncStatus>>,
}

impl Desktop {
    pub fn start(emit: impl Fn(&SyncStatus) + Send + 'static) -> Self {
        let (queue, receiver) = Queue::new();
        let stop = Arc::new(AtomicBool::new(false));
        let status = Arc::new(Mutex::new(SyncStatus::default()));
        let worker_queue = queue.clone();
        let worker_stop = stop.clone();
        let worker_status = status.clone();
        std::thread::spawn(move || {
            let publish = |state: &SyncStatus| {
                *worker_status.lock().unwrap_or_else(|e| e.into_inner()) = state.clone();
                emit(state);
            };
            let cancelled = || worker_stop.load(Ordering::Relaxed);
            let mut state = SyncStatus::default();
            let load_roots = || {
                crate::usage::AGENTS
                    .iter()
                    .map(|&id| crate::usage::roots(id).map(|roots| (id.to_owned(), roots)))
                    .collect::<Result<Roots, String>>()
                    .map(Some)
            };
            while !cancelled() {
                let run = (|| {
                    Collection {
                        control_dir: None,
                        ledger: crate::usage::ledger::path()?,
                        cache: cache_path()?,
                        queue: worker_queue.clone(),
                        receiver: &receiver,
                    }
                    .run(
                        load_roots,
                        &cancelled,
                        &mut state,
                        &publish,
                        |_, _, _| Ok(()),
                    )
                })();
                if run.is_ok() || cancelled() {
                    break;
                }
                state.unavailable = true;
                state.syncing.clear();
                state.recollecting.clear();
                state.progress = None;
                state.sequence += 1;
                publish(&state);
                let _ = receiver.recv_timeout(RECONCILE);
            }
        });
        Self {
            queue,
            stop,
            status,
        }
    }

    pub fn status(&self) -> SyncStatus {
        let mut status = self.status
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        status.recollecting.extend(self.queue.queued_recollections());
        status
    }

    pub fn check_now(&self) {
        self.queue.check_now();
    }

    pub fn recollect(&self, agent: String) -> Result<SyncStatus, String> {
        if !crate::usage::AGENTS.contains(&agent.as_str()) {
            return Err("不支持的用量来源".into());
        }
        if !self.status().recollecting.contains(&agent) {
            self.queue.recollect(agent);
        }
        Ok(self.status())
    }

    pub fn ledger_changed(&self) {
        self.queue.ledger_changed();
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
        wake(&self.queue.sender);
    }
}

impl Drop for Desktop {
    fn drop(&mut self) {
        self.stop();
    }
}
