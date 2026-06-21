use std::sync::Arc;

use once_cell::sync::Lazy;
use regex::Regex;
use tokio::io::AsyncReadExt;
use tokio::sync::Mutex;

use super::config::{find_bun_path, find_golang_path, find_python_path};
use crate::python::runner::RunnerState;

pub const DEFAULT_RUN_TIMEOUT_SECS: u64 = 30;
pub const GO_RUN_TIMEOUT_SECS: u64 = 60;
pub const TS_RUN_TIMEOUT_SECS: u64 = 60;

pub static DEBUG_PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| {
    vec![
        Regex::new(r"^\s*\* (Running on|Serving Flask|Debug mode|Debugger)").unwrap(),
        Regex::new(r"^\s*WARNING: this is a development server").unwrap(),
        Regex::new(r"^\s*Press CTRL\+C to quit").unwrap(),
        Regex::new(r"^\s*\* Restarting with").unwrap(),
        Regex::new(r"^\s*\* Debugger is active").unwrap(),
        Regex::new(r"^\s*\* Debugger PIN:").unwrap(),
    ]
});

pub fn is_debug_info(line: &str) -> bool {
    DEBUG_PATTERNS.iter().any(|re| re.is_match(line))
}

fn valid_utf8_len(buf: &[u8]) -> usize {
    match std::str::from_utf8(buf) {
        Ok(_) => buf.len(),
        Err(e) => e.valid_up_to(),
    }
}

pub async fn forward_stream<R, F, Fut>(reader: R, mut on_chunk: F)
where
    R: tokio::io::AsyncRead + Unpin,
    F: FnMut(String) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let mut r = reader;
    let mut buf = vec![0u8; 8192];
    let mut leftover: Vec<u8> = Vec::new();
    loop {
        let n = match r.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => n,
            Err(_) => break,
        };
        leftover.extend_from_slice(&buf[..n]);
        let valid_len = valid_utf8_len(&leftover);
        if valid_len == 0 {
            continue;
        }
        let text = String::from_utf8(leftover[..valid_len].to_vec()).unwrap_or_default();
        leftover = leftover[valid_len..].to_vec();
        on_chunk(text).await;
    }
    if !leftover.is_empty() {
        let text = String::from_utf8_lossy(&leftover).to_string();
        on_chunk(text).await;
    }
}

pub async fn run_with_timeout<F, T>(secs: u64, label: &str, fut: F) -> Result<T, String>
where
    F: std::future::Future<Output = T>,
{
    match tokio::time::timeout(std::time::Duration::from_secs(secs), fut).await {
        Ok(v) => Ok(v),
        Err(_) => Err(format!("{label} 执行超时(>{secs}秒),已强制终止")),
    }
}

pub async fn warmup_runtimes(state: Arc<Mutex<RunnerState>>) {
    let py = find_python_path().await;
    {
        let mut s = state.lock().await;
        s.python_path = py.clone();
        s.python_detected = true;
    }
    let _ = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        tokio::process::Command::new(&py)
            .arg("-c")
            .arg("pass")
            .env("PYTHONIOENCODING", "utf-8")
            .env("PYTHONUTF8", "1")
            .output(),
    )
    .await;
    tracing::info!("Python 预热完成: {py}");

    let go = find_golang_path().await;
    {
        let mut s = state.lock().await;
        s.golang_path = go.clone();
        s.golang_detected = true;
    }
    let _ = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        tokio::process::Command::new(&go).arg("version").output(),
    )
    .await;
    tracing::info!("Golang 预热完成: {go}");

    let bun = find_bun_path().await;
    {
        let mut s = state.lock().await;
        s.bun_path = bun.clone();
        s.bun_detected = true;
    }
    let _ = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        tokio::process::Command::new(&bun).arg("--version").output(),
    )
    .await;
    tracing::info!("Bun 预热完成: {bun}");
}
