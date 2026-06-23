use std::sync::Arc;

use once_cell::sync::Lazy;
use serde::Serialize;
use tokio::sync::Mutex;

const LOG_BUFFER_SIZE: usize = 500;

#[derive(Debug, Clone, Serialize)]
pub struct LogEntry {
    pub timestamp: i64,
    pub level: String,
    pub target: String,
    pub message: String,
}

#[derive(Default)]
struct LogBuffer {
    entries: Vec<LogEntry>,
    panics: Vec<LogEntry>,
}

static LOG_STATE: Lazy<Arc<Mutex<LogBuffer>>> =
    Lazy::new(|| Arc::new(Mutex::new(LogBuffer::default())));

pub fn record_log(entry: LogEntry) {
    if let Ok(mut state) = LOG_STATE.try_lock() {
        if entry.level == "ERROR" && entry.target == "panic" {
            state.panics.push(entry.clone());
            if state.panics.len() > 50 {
                let drop_count = state.panics.len() - 50;
                state.panics.drain(0..drop_count);
            }
        }
        state.entries.push(entry);
        if state.entries.len() > LOG_BUFFER_SIZE {
            let drop_count = state.entries.len() - LOG_BUFFER_SIZE;
            state.entries.drain(0..drop_count);
        }
    }
}

pub async fn recent_logs(level: Option<&str>, limit: usize) -> Vec<LogEntry> {
    let state = LOG_STATE.lock().await;
    let mut out: Vec<LogEntry> = state
        .entries
        .iter()
        .filter(|e| match level {
            Some(lvl) if !lvl.is_empty() => e.level.eq_ignore_ascii_case(lvl),
            _ => true,
        })
        .cloned()
        .collect();
    out.reverse();
    if out.len() > limit {
        out.truncate(limit);
    }
    out
}

pub async fn recent_panics() -> Vec<LogEntry> {
    let state = LOG_STATE.lock().await;
    state.panics.clone()
}

pub async fn log_counts() -> (usize, usize) {
    let state = LOG_STATE.lock().await;
    (state.entries.len(), state.panics.len())
}
