use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use tokio::sync::Mutex;

use crate::config::cache_dir;
use crate::python::config::find_python_path;
use crate::settings::{PrewarmConfig, Settings};
use crate::utils::retry::{retry_async, RetryPolicy};
use crate::websocket::webtty::{Webtty, WsCmd};

#[derive(Debug, Clone, Serialize)]
pub struct PrewarmReport {
    pub requested: Vec<String>,
    pub installed: Vec<String>,
    pub failed: Vec<PrewarmFailure>,
    pub started_at: i64,
    pub finished_at: i64,
    pub duration_ms: i64,
    pub from_cache: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct PrewarmFailure {
    pub package: String,
    pub reason: String,
}

pub fn marker_path() -> std::path::PathBuf {
    cache_dir().join("prewarm.done")
}

pub fn is_already_warmed(marker_ttl_hours: u64) -> bool {
    let path = marker_path();
    if !path.exists() {
        return false;
    }
    let Ok(meta) = std::fs::metadata(&path) else {
        return false;
    };
    let Ok(modified) = meta.modified() else {
        return false;
    };
    let Ok(elapsed) = std::time::SystemTime::now().duration_since(modified) else {
        return false;
    };
    elapsed < Duration::from_secs(marker_ttl_hours * 3600)
}

pub fn write_marker() {
    let path = marker_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&path, crate::history::now_millis().to_string());
}

pub async fn run_prewarm(
    config: &PrewarmConfig,
    settings: &Settings,
    webtty: Option<Arc<Mutex<Webtty>>>,
) -> PrewarmReport {
    let started = crate::history::now_millis();
    let mut report = PrewarmReport {
        requested: config.packages.clone(),
        installed: Vec::new(),
        failed: Vec::new(),
        started_at: started,
        finished_at: started,
        duration_ms: 0,
        from_cache: false,
    };
    if !config.enabled || config.packages.is_empty() {
        return report;
    }
    let python = find_python_path().await;
    if python.is_empty() {
        for pkg in &config.packages {
            report.failed.push(PrewarmFailure {
                package: pkg.clone(),
                reason: "未找到 Python 解释器".to_string(),
            });
        }
        report.finished_at = crate::history::now_millis();
        report.duration_ms = report.finished_at - report.started_at;
        return report;
    }
    for pkg in &config.packages {
        if let Some(wt) = &webtty {
            let mut w = wt.lock().await;
            w.send_msg(&WsCmd::BackendEvent {
                data: format!("\x1b[44;37m[预热] 预装依赖: {}\x1b[0m\r\n", pkg),
            })
            .await;
        }
        let proxy = settings.proxy.https.clone().or(settings.proxy.http.clone());
        let result = install_with_proxy(&python, pkg, proxy.as_deref()).await;
        match result {
            InstallOutcome::Success => report.installed.push(pkg.clone()),
            InstallOutcome::Failed(reason) => report.failed.push(PrewarmFailure {
                package: pkg.clone(),
                reason,
            }),
        }
    }
    write_marker();
    report.finished_at = crate::history::now_millis();
    report.duration_ms = report.finished_at - report.started_at;
    report
}

pub enum InstallOutcome {
    Success,
    Failed(String),
}

pub async fn install_with_proxy(
    python_path: &str,
    pkg: &str,
    proxy: Option<&str>,
) -> InstallOutcome {
    let proxy_owned = proxy.map(|s| s.to_string());
    let pkg_owned = pkg.to_string();
    let py_owned = python_path.to_string();
    let res = retry_async(&RetryPolicy::network(), || {
        let py = py_owned.clone();
        let pkg_name = pkg_owned.clone();
        let proxy = proxy_owned.clone();
        async move {
            let mut cmd = tokio::process::Command::new(&py);
            cmd.args([
                "-m",
                "pip",
                "install",
                &pkg_name,
                "--no-cache-dir",
                "--no-warn-script-location",
                "--disable-pip-version-check",
            ]);
            if let Some(p) = proxy.as_deref() {
                cmd.env("PIP_INDEX_URL", p);
            }
            cmd.stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped());
            let out = tokio::time::timeout(Duration::from_secs(180), cmd.output()).await;
            match out {
                Ok(Ok(o)) if o.status.success() => Ok(InstallOutcome::Success),
                Ok(Ok(o)) => {
                    let err = String::from_utf8_lossy(&o.stderr).to_string();
                    let trimmed = err.trim();
                    let reason = if trimmed.is_empty() {
                        format!("exit {:?}", o.status.code())
                    } else {
                        trimmed.chars().take(200).collect()
                    };
                    Err(reason)
                }
                Ok(Err(e)) => Err(e.to_string()),
                Err(_) => Err("安装超时".to_string()),
            }
        }
    })
    .await;
    match res {
        Ok(outcome) => outcome,
        Err(reason) => InstallOutcome::Failed(reason),
    }
}
