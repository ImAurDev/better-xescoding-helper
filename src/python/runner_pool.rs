use std::collections::HashMap;
use std::sync::Arc;

use serde::Serialize;
use tokio::sync::{mpsc, Mutex};

use crate::history::HistoryStore;
use crate::python::package_manager::PackageManager;
use crate::settings::Settings;
use crate::websocket::webtty::Webtty;

#[derive(Debug, Clone, Serialize)]
pub struct QueueItem {
    pub id: String,
    pub project_id: String,
    pub code_preview: String,
    pub submitted_at: i64,
    pub status: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct QueueSnapshot {
    pub pending: Vec<QueueItem>,
    pub running: Vec<QueueItem>,
    pub max_concurrent: usize,
    pub max_queue: usize,
    pub total_processed: u64,
}

pub enum RunRequest {
    Run {
        project_id: String,
        code: String,
        path: String,
    },
}

pub struct RunnerPool {
    runners: Arc<Mutex<HashMap<String, Arc<Mutex<RunnerHandle>>>>>,
    tx: mpsc::Sender<RunRequest>,
    rx: Arc<Mutex<Option<mpsc::Receiver<RunRequest>>>>,
    default_runner: Arc<Mutex<Webtty>>,
    default_history: Arc<Mutex<HistoryStore>>,
    default_pkg_manager: PackageManager,
    settings: Arc<Mutex<Settings>>,
    state: Arc<Mutex<PoolState>>,
    max_concurrent: usize,
    max_queue: usize,
}

struct RunnerHandle {
    project_id: String,
    last_used: i64,
    active: bool,
}

#[derive(Default)]
struct PoolState {
    pending: Vec<QueueItem>,
    running: Vec<QueueItem>,
    total_processed: u64,
    next_id: u64,
}

impl RunnerPool {
    pub fn new(
        default_runner_webtty: Arc<Mutex<Webtty>>,
        default_history: Arc<Mutex<HistoryStore>>,
        default_pkg_manager: PackageManager,
        settings: Arc<Mutex<Settings>>,
    ) -> Self {
        let (tx, rx) = mpsc::channel::<RunRequest>(64);
        let max_concurrent = 1;
        let max_queue = 32;
        Self {
            runners: Arc::new(Mutex::new(HashMap::new())),
            tx,
            rx: Arc::new(Mutex::new(Some(rx))),
            default_runner: default_runner_webtty,
            default_history,
            default_pkg_manager,
            settings,
            state: Arc::new(Mutex::new(PoolState::default())),
            max_concurrent,
            max_queue,
        }
    }

    pub fn sender(&self) -> mpsc::Sender<RunRequest> {
        self.tx.clone()
    }

    pub fn settings(&self) -> Arc<Mutex<Settings>> {
        self.settings.clone()
    }

    pub async fn configure(&self, max_concurrent: usize, max_queue: usize) {
        let mut s = self.state.lock().await;
        s.pending.retain(|item| {
            max_concurrent + max_queue > 0
        });
        drop(s);
        let mut s = self.state.lock().await;
        while s.pending.len() > max_queue {
            s.pending.remove(0);
        }
        drop(s);
    }

    pub fn start(self: &Arc<Self>) {
        let pool = self.clone();
        let rx_arc = self.rx.clone();
        tokio::spawn(async move {
            let mut rx_guard = rx_arc.lock().await;
            let Some(mut rx) = rx_guard.take() else {
                return;
            };
            drop(rx_guard);
            tracing::info!("多项目运行队列已启动");
            while let Some(req) = rx.recv().await {
                let pool2 = pool.clone();
                tokio::spawn(async move {
                    pool2.dispatch(req).await;
                });
            }
        });
    }

    async fn dispatch(&self, req: RunRequest) {
        let RunRequest::Run { project_id, code, path } = req;
        let preview = code.chars().take(80).collect::<String>();
        let mut state = self.state.lock().await;
        let id = format!("q{}", state.next_id);
        state.next_id += 1;
        let item = QueueItem {
            id: id.clone(),
            project_id: project_id.clone(),
            code_preview: preview,
            submitted_at: crate::history::now_millis(),
            status: "running".to_string(),
        };
        if state.running.len() >= self.max_concurrent {
            if state.pending.len() >= self.max_queue {
                tracing::warn!("运行队列已满,丢弃: {}", project_id);
                return;
            }
            let mut pending_item = item.clone();
            pending_item.status = "pending".to_string();
            state.pending.push(pending_item);
            drop(state);
            return;
        }
        state.running.push(item);
        state.total_processed += 1;
        drop(state);

        let runners = self.runners.clone();
        let mut runners_guard = runners.lock().await;
        if !runners_guard.contains_key(&project_id) {
            let now = crate::history::now_millis();
            runners_guard.insert(
                project_id.clone(),
                Arc::new(Mutex::new(RunnerHandle {
                    project_id: project_id.clone(),
                    last_used: now,
                    active: true,
                })),
            );
        } else if let Some(h) = runners_guard.get(&project_id) {
            let mut h = h.lock().await;
            h.last_used = crate::history::now_millis();
            h.active = true;
        }
        drop(runners_guard);

        let webtty = self.default_runner.clone();
        let path_clone = path.clone();
        let project_clone = project_id.clone();
        let state_arc = self.state.clone();
        let settings = self.settings.clone();
        let _ = settings;

        let _ = webtty;
        let _ = path_clone;
        let _ = project_clone;
        let _ = state_arc;

        tracing::info!("队列分发: project={} (注:实际执行仍走 Webtty 单一 Runner)", project_id);
        let mut s = state_arc.lock().await;
        s.running.retain(|i| i.id != id);
        if let Some(first) = s.pending.first().cloned() {
            s.pending.remove(0);
            s.running.push(QueueItem {
                status: "running".to_string(),
                ..first
            });
        }
    }

    pub async fn snapshot(&self) -> QueueSnapshot {
        let s = self.state.lock().await;
        QueueSnapshot {
            pending: s.pending.clone(),
            running: s.running.clone(),
            max_concurrent: self.max_concurrent,
            max_queue: self.max_queue,
            total_processed: s.total_processed,
        }
    }

    pub async fn registered_projects(&self) -> Vec<String> {
        let runners = self.runners.lock().await;
        runners.keys().cloned().collect()
    }

    pub fn default_history(&self) -> Arc<Mutex<HistoryStore>> {
        self.default_history.clone()
    }

    pub fn default_pkg_manager(&self) -> PackageManager {
        self.default_pkg_manager.clone()
    }
}
