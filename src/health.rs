use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Serialize;
use sysinfo::System;
use tokio::sync::Mutex;

use crate::history::HistoryStore;
use crate::settings::Settings;
use crate::utils::log_buffer;
use crate::websocket::webtty::Webtty;

#[derive(Serialize)]
pub struct RuntimeStatus {
    pub available: bool,
    pub path: String,
    pub version: Option<String>,
}

#[derive(Serialize)]
pub struct HealthReport {
    pub status: &'static str,
    pub uptime_secs: u64,
    pub server_version: &'static str,
    pub system: SystemInfo,
    pub runtimes: RuntimesInfo,
    pub caches: CacheInfo,
    pub history: HistoryInfo,
    pub logs: LogInfo,
    pub server_error: Option<serde_json::Value>,
    pub active_clients: usize,
}

#[derive(Serialize)]
pub struct SystemInfo {
    pub os: String,
    pub arch: String,
    pub cpu_count: usize,
    pub total_memory_bytes: u64,
    pub used_memory_bytes: u64,
    pub process_rss_bytes: u64,
    pub process_cpu_pct: f32,
    pub load_avg: Option<[f64; 3]>,
}

#[derive(Serialize)]
pub struct RuntimesInfo {
    pub python: RuntimeStatus,
    pub golang: RuntimeStatus,
    pub bun: RuntimeStatus,
}

#[derive(Serialize)]
pub struct CacheInfo {
    pub root: String,
    pub asset_bytes: u64,
    pub asset_pool_bytes: u64,
    pub snapshot_bytes: u64,
    pub snapshot_count: usize,
}

#[derive(Serialize)]
pub struct HistoryInfo {
    pub total: usize,
    pub last_duration_ms: Option<i64>,
    pub last_success: Option<bool>,
}

#[derive(Serialize)]
pub struct LogInfo {
    pub buffered: usize,
    pub panics: usize,
}

pub fn process_rss_bytes() -> u64 {
    let pid = std::process::id();
    crate::utils::process_stats::sample_rss_bytes(pid).unwrap_or(0)
}

pub fn system_snapshot() -> SystemInfo {
    let mut sys = System::new();
    sys.refresh_memory();
    sys.refresh_cpu_usage();
    let total = sys.total_memory();
    let used = sys.used_memory();
    let cpu_count = sys.cpus().len();
    let load_avg = {
        let la = System::load_average();
        if la.one > 0.0 || la.five > 0.0 || la.fifteen > 0.0 {
            Some([la.one, la.five, la.fifteen])
        } else {
            None
        }
    };
    let process_rss = process_rss_bytes();
    let proc_sys = System::new();
    let me = std::process::id();
    let proc_cpu = proc_sys
        .process(sysinfo::Pid::from_u32(me))
        .map(|p| p.cpu_usage())
        .unwrap_or(0.0);
    SystemInfo {
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        cpu_count,
        total_memory_bytes: total,
        used_memory_bytes: used,
        process_rss_bytes: process_rss,
        process_cpu_pct: proc_cpu,
        load_avg,
    }
}

pub async fn build_report(
    started: Instant,
    settings: &Settings,
    history: &Arc<Mutex<HistoryStore>>,
    webtty: &Arc<Mutex<Webtty>>,
    server_error: Option<serde_json::Value>,
) -> HealthReport {
    let sys = tokio::task::spawn_blocking(system_snapshot)
        .await
        .unwrap_or_else(|_| SystemInfo {
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            cpu_count: 0,
            total_memory_bytes: 0,
            used_memory_bytes: 0,
            process_rss_bytes: 0,
            process_cpu_pct: 0.0,
            load_avg: None,
        });

    let py = describe_python(&settings).await;
    let go = describe_golang(&settings).await;
    let bun = describe_bun(&settings).await;

    let caches = build_cache_info(&settings).await;
    let history_info = build_history_info(history).await;
    let (log_buf, log_panics) = log_buffer::log_counts().await;
    let active_clients = {
        let wt = webtty.lock().await;
        if wt.client_tx_is_some() { 1 } else { 0 }
    };

    let status = if server_error.is_some() {
        "degraded"
    } else if !py.available {
        "degraded"
    } else {
        "ok"
    };

    HealthReport {
        status,
        uptime_secs: started.elapsed().as_secs(),
        server_version: env!("CARGO_PKG_VERSION"),
        system: sys,
        runtimes: RuntimesInfo {
            python: py,
            golang: go,
            bun: bun,
        },
        caches,
        history: history_info,
        logs: LogInfo {
            buffered: log_buf,
            panics: log_panics,
        },
        server_error,
        active_clients,
    }
}

async fn describe_python(_settings: &Settings) -> RuntimeStatus {
    let path = crate::python::current_python_path().await;
    let version = run_version(&path, &["-V"]).await;
    RuntimeStatus {
        available: !version.is_empty(),
        path,
        version: if version.is_empty() { None } else { Some(version) },
    }
}

async fn describe_golang(_settings: &Settings) -> RuntimeStatus {
    let path = crate::python::current_golang_path().await;
    let version = run_version(&path, &["version"]).await;
    RuntimeStatus {
        available: !version.is_empty(),
        path,
        version: if version.is_empty() { None } else { Some(version) },
    }
}

async fn describe_bun(_settings: &Settings) -> RuntimeStatus {
    let path = crate::python::current_bun_path().await;
    let version = run_version(&path, &["--version"]).await;
    RuntimeStatus {
        available: !version.is_empty(),
        path,
        version: if version.is_empty() { None } else { Some(version) },
    }
}

async fn run_version(path: &str, args: &[&str]) -> String {
    let path = path.to_string();
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let res = tokio::time::timeout(
        Duration::from_secs(3),
        tokio::process::Command::new(&path).args(&args).output(),
    )
    .await;
    match res {
        Ok(Ok(out)) if out.status.success() => {
            let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if s.is_empty() {
                String::from_utf8_lossy(&out.stderr).trim().to_string()
            } else {
                s
            }
        }
        _ => String::new(),
    }
}

async fn build_cache_info(_settings: &Settings) -> CacheInfo {
    let root = crate::config::cache_dir().to_string_lossy().to_string();
    let asset = crate::config::asset_path();
    let pool = crate::config::asset_pool_path();
    let snapshot_dir = crate::config::cache_dir().join("snapshots");
    let asset_bytes = dir_size(&asset).await;
    let pool_bytes = dir_size(&pool).await;
    let snapshot_bytes = dir_size(&snapshot_dir).await;
    let snapshot_count = count_files(&snapshot_dir).await;
    CacheInfo {
        root,
        asset_bytes,
        asset_pool_bytes: pool_bytes,
        snapshot_bytes,
        snapshot_count,
    }
}

async fn dir_size(path: &std::path::Path) -> u64 {
    let mut total = 0u64;
    let mut stack = vec![path.to_path_buf()];
    while let Some(p) = stack.pop() {
        if !p.exists() {
            continue;
        }
        let Ok(mut rd) = tokio::fs::read_dir(&p).await else {
            continue;
        };
        while let Ok(Some(entry)) = rd.next_entry().await {
            let Ok(meta) = entry.metadata().await else {
                continue;
            };
            if meta.is_dir() {
                stack.push(entry.path());
            } else if meta.is_file() {
                total += meta.len();
            }
        }
    }
    total
}

async fn count_files(path: &std::path::Path) -> usize {
    if !path.exists() {
        return 0;
    }
    let Ok(mut rd) = tokio::fs::read_dir(path).await else {
        return 0;
    };
    let mut count = 0;
    while let Ok(Some(entry)) = rd.next_entry().await {
        if let Ok(meta) = entry.metadata().await {
            if meta.is_file() {
                count += 1;
            }
        }
    }
    count
}

async fn build_history_info(history: &Arc<Mutex<HistoryStore>>) -> HistoryInfo {
    let records = history.lock().await.list().await;
    let last = records.first().cloned();
    HistoryInfo {
        total: records.len(),
        last_duration_ms: last.as_ref().map(|r| r.duration),
        last_success: last.as_ref().map(|r| r.success),
    }
}
