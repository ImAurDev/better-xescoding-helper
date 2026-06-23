use std::sync::Arc;
use std::time::Duration;

use once_cell::sync::Lazy;
use serde::Serialize;
use tokio::sync::Mutex;

use crate::utils::process_stats::sample_rss_bytes;

#[derive(Debug, Clone, Default, Serialize)]
pub struct RunMetrics {
    pub id: String,
    pub project_id: String,
    pub start_ts: i64,
    pub end_ts: i64,
    pub duration_ms: i64,
    pub exit_code: Option<i32>,
    pub success: bool,
    pub peak_rss_bytes: u64,
    pub avg_rss_bytes: u64,
    pub samples: u32,
    pub auto_install_attempts: u32,
    pub auto_install_success: u32,
    pub lint_issues: u32,
    pub missing_imports_resolved: u32,
    pub has_go_blocks: bool,
}

const RING_CAPACITY: usize = 200;

#[derive(Default)]
struct MetricsState {
    ring: Vec<RunMetrics>,
    pending: Option<PendingMetrics>,
    capacity: usize,
}

struct PendingMetrics {
    id: String,
    project_id: String,
    start_ts: i64,
    has_go_blocks: bool,
    samples: Vec<u64>,
    auto_install_attempts: u32,
    auto_install_success: u32,
    lint_issues: u32,
    missing_imports_resolved: u32,
    running: bool,
    pid: Option<u32>,
    poll_task: Option<tokio::task::JoinHandle<()>>,
}

impl MetricsState {
    fn push_ring(&mut self, m: RunMetrics) {
        self.ring.push(m);
        let cap = self.capacity.max(1);
        if self.ring.len() > cap {
            let drop = self.ring.len() - cap;
            self.ring.drain(0..drop);
        }
    }
}

static STATE: Lazy<Arc<Mutex<MetricsState>>> = Lazy::new(|| {
    Arc::new(Mutex::new(MetricsState {
        capacity: RING_CAPACITY,
        ..Default::default()
    }))
});

pub async fn configure_capacity(cap: usize) {
    let mut s = STATE.lock().await;
    s.capacity = cap.max(1);
    if s.ring.len() > s.capacity {
        let drop = s.ring.len() - s.capacity;
        s.ring.drain(0..drop);
    }
}

pub async fn begin_run(project_id: &str, has_go_blocks: bool) -> String {
    let id = crate::history::gen_id();
    let mut s = STATE.lock().await;
    s.pending = Some(PendingMetrics {
        id: id.clone(),
        project_id: project_id.to_string(),
        start_ts: crate::history::now_millis(),
        has_go_blocks,
        samples: Vec::new(),
        auto_install_attempts: 0,
        auto_install_success: 0,
        lint_issues: 0,
        missing_imports_resolved: 0,
        running: true,
        pid: None,
        poll_task: None,
    });
    id
}

pub async fn track_pid(pid: u32) {
    let mut s = STATE.lock().await;
    if let Some(p) = s.pending.as_mut() {
        p.pid = Some(pid);
        if p.poll_task.is_none() {
            let state_arc = STATE.clone();
            p.poll_task = Some(tokio::spawn(async move {
                loop {
                    tokio::time::sleep(Duration::from_millis(250)).await;
                    let (running, pid, samples_count) = {
                        let mut s = state_arc.lock().await;
                        let Some(p) = s.pending.as_mut() else {
                            break;
                        };
                        if !p.running {
                            break;
                        }
                        if let Some(pid) = p.pid {
                            if let Some(rss) = sample_rss_bytes(pid) {
                                p.samples.push(rss);
                            }
                        }
                        (p.running, p.pid, p.samples.len())
                    };
                    let _ = (running, pid, samples_count);
                }
            }));
        }
    }
}

pub async fn record_auto_install(success: bool) {
    let mut s = STATE.lock().await;
    if let Some(p) = s.pending.as_mut() {
        p.auto_install_attempts += 1;
        if success {
            p.auto_install_success += 1;
        }
    }
}

pub async fn record_lint(issues: u32) {
    let mut s = STATE.lock().await;
    if let Some(p) = s.pending.as_mut() {
        p.lint_issues = p.lint_issues.saturating_add(issues);
    }
}

pub async fn record_missing_imports_resolved(count: u32) {
    let mut s = STATE.lock().await;
    if let Some(p) = s.pending.as_mut() {
        p.missing_imports_resolved = p.missing_imports_resolved.saturating_add(count);
    }
}

pub async fn end_run(exit_code: Option<i32>, success: bool) -> Option<RunMetrics> {
    let mut s = STATE.lock().await;
    let mut pending = s.pending.take()?;
    pending.running = false;
    let end_ts = crate::history::now_millis();
    let duration_ms = end_ts - pending.start_ts;
    let mut samples = pending.samples.clone();
    samples.sort_unstable();
    let peak = samples.iter().copied().max().unwrap_or(0);
    let avg = if samples.is_empty() {
        0
    } else {
        samples.iter().sum::<u64>() / samples.len() as u64
    };
    let metrics = RunMetrics {
        id: pending.id,
        project_id: pending.project_id,
        start_ts: pending.start_ts,
        end_ts,
        duration_ms,
        exit_code,
        success,
        peak_rss_bytes: peak,
        avg_rss_bytes: avg,
        samples: pending.samples.len() as u32,
        auto_install_attempts: pending.auto_install_attempts,
        auto_install_success: pending.auto_install_success,
        lint_issues: pending.lint_issues,
        missing_imports_resolved: pending.missing_imports_resolved,
        has_go_blocks: pending.has_go_blocks,
    };
    s.push_ring(metrics.clone());
    if let Some(handle) = pending.poll_task {
        handle.abort();
    }
    Some(metrics)
}

pub async fn recent(limit: usize) -> Vec<RunMetrics> {
    let s = STATE.lock().await;
    let n = limit.min(s.ring.len());
    s.ring.iter().rev().take(n).cloned().collect()
}

pub async fn summary() -> serde_json::Value {
    let s = STATE.lock().await;
    let total = s.ring.len();
    let successful = s.ring.iter().filter(|m| m.success).count();
    let avg_duration = if total == 0 {
        0
    } else {
        s.ring.iter().map(|m| m.duration_ms).sum::<i64>() / total as i64
    };
    let avg_peak = if total == 0 {
        0
    } else {
        s.ring.iter().map(|m| m.peak_rss_bytes).sum::<u64>() / total as u64
    };
    serde_json::json!({
        "total_runs": total,
        "successful_runs": successful,
        "success_rate": if total == 0 { 0.0 } else { successful as f64 / total as f64 },
        "avg_duration_ms": avg_duration,
        "avg_peak_rss_bytes": avg_peak,
        "auto_install_total": s.ring.iter().map(|m| m.auto_install_attempts).sum::<u32>(),
        "auto_install_success": s.ring.iter().map(|m| m.auto_install_success).sum::<u32>(),
    })
}
