use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::config::{asset_pool_path, cache_dir};

const SNAPSHOT_MAX_AGE_DAYS: u64 = 7;
const ASSET_POOL_MAX_AGE_DAYS: u64 = 30;
const TEMP_MAX_AGE_HOURS: u64 = 24;
const CLEANUP_INTERVAL_HOURS: u64 = 6;

const MAX_OUTPUT_BYTES: u64 = 64 * 1024 * 1024;

fn file_age_seconds(path: &Path) -> Option<u64> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta.modified().ok()?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?;
    let age = now.checked_sub(mtime.duration_since(UNIX_EPOCH).ok()?)?;
    Some(age.as_secs())
}

async fn remove_if_older_than(path: &Path, max_age_secs: u64) -> std::io::Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    let age = file_age_seconds(path).unwrap_or(0);
    if age > max_age_secs {
        if path.is_dir() {
            tokio::fs::remove_dir_all(path).await?;
        } else {
            tokio::fs::remove_file(path).await?;
        }
        Ok(true)
    } else {
        Ok(false)
    }
}

async fn clean_dir_older_than(dir: &Path, max_age_secs: u64) -> (usize, u64) {
    let mut count = 0usize;
    let mut bytes_freed = 0u64;
    let mut entries = match tokio::fs::read_dir(dir).await {
        Ok(e) => e,
        Err(_) => return (0, 0),
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let p = entry.path();
        let meta = match entry.metadata().await {
            Ok(m) => m,
            Err(_) => continue,
        };
        let size = meta.len();
        let age = file_age_seconds(&p).unwrap_or(0);
        if age > max_age_secs {
            let result = if meta.is_dir() {
                tokio::fs::remove_dir_all(&p).await
            } else {
                tokio::fs::remove_file(&p).await
            };
            if result.is_ok() {
                count += 1;
                bytes_freed += size;
            }
        }
    }
    (count, bytes_freed)
}

async fn run_cleanup_pass() {
    let cache = cache_dir();
    let snapshots_dir = cache.join("snapshots");
    let asset_pool = asset_pool_path();
    let temp_dir = std::env::temp_dir().join("xes-coding-helper");

    let snapshot_secs = SNAPSHOT_MAX_AGE_DAYS * 24 * 60 * 60;
    let asset_secs = ASSET_POOL_MAX_AGE_DAYS * 24 * 60 * 60;
    let temp_secs = TEMP_MAX_AGE_HOURS * 60 * 60;

    let (snap_count, snap_bytes) = clean_dir_older_than(&snapshots_dir, snapshot_secs).await;
    let (pool_count, pool_bytes) = clean_dir_older_than(&asset_pool, asset_secs).await;
    let (temp_count, temp_bytes) = clean_dir_older_than(&temp_dir, temp_secs).await;

    if let Some(parent) = snapshots_dir.parent() {
        let mut entries = match tokio::fs::read_dir(parent).await {
            Ok(e) => e,
            Err(_) => return,
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("snapshot_") {
                let _ = remove_if_older_than(&entry.path(), snapshot_secs).await;
            }
        }
    }

    if snap_count + pool_count + temp_count > 0 {
        tracing::info!(
            "缓存清理完成: 快照 {} 项({} KB) 资产池 {} 项({} KB) 临时 {} 项({} KB)",
            snap_count,
            snap_bytes / 1024,
            pool_count,
            pool_bytes / 1024,
            temp_count,
            temp_bytes / 1024,
        );
    }
}

pub fn start() {
    let interval = Duration::from_secs(CLEANUP_INTERVAL_HOURS * 60 * 60);
    tokio::spawn(async move {
        run_cleanup_pass().await;
        let mut ticker = tokio::time::interval(interval);
        ticker.tick().await;
        loop {
            ticker.tick().await;
            run_cleanup_pass().await;
        }
    });
    tracing::info!(
        "缓存清理任务已启动: 快照 {} 天 / 资产池 {} 天 / 临时 {} 小时, 每 {} 小时执行一次",
        SNAPSHOT_MAX_AGE_DAYS,
        ASSET_POOL_MAX_AGE_DAYS,
        TEMP_MAX_AGE_HOURS,
        CLEANUP_INTERVAL_HOURS
    );
}

pub async fn save_code_snapshot(code: &str, run_id: &str) -> Option<std::path::PathBuf> {
    let dir = cache_dir().join("snapshots");
    if let Err(e) = tokio::fs::create_dir_all(&dir).await {
        tracing::warn!("创建快照目录失败: {e}");
        return None;
    }
    let path = dir.join(format!("snapshot_{}_{}.py", run_id, now_secs()));
    if let Err(e) = tokio::fs::write(&path, code.as_bytes()).await {
        tracing::warn!("保存快照失败: {e}");
        return None;
    }
    Some(path)
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub const fn max_output_bytes() -> u64 {
    MAX_OUTPUT_BYTES
}
