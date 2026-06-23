use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tokio::task::JoinHandle;

use crate::config::{asset_path, asset_pool_path, cache_dir};
use crate::settings::CleanupPolicy;

const SNAPSHOT_MAX_AGE_DAYS: u64 = 7;
const ASSET_POOL_MAX_AGE_DAYS: u64 = 30;
const TEMP_MAX_AGE_HOURS: u64 = 24;
const CLEANUP_INTERVAL_HOURS: u64 = 6;

const MAX_OUTPUT_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, Serialize)]
pub struct CacheListInfo {
    pub cache_root: String,
    pub asset_bytes: u64,
    pub asset_pool_bytes: u64,
    pub snapshot_bytes: u64,
    pub snapshot_count: usize,
    pub project_count: usize,
    pub venv_bytes: u64,
    pub venv_count: usize,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct CleanupResult {
    pub snapshots_removed: usize,
    pub snapshots_bytes_freed: u64,
    pub asset_pool_removed: usize,
    pub asset_pool_bytes_freed: u64,
    pub temp_removed: usize,
    pub temp_bytes_freed: u64,
    pub lru_removed: usize,
    pub lru_bytes_freed: u64,
    pub ran_lru: bool,
    pub duration_ms: u64,
}

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

pub async fn run_cleanup_with_policy(policy: &CleanupPolicy) -> CleanupResult {
    let start = std::time::Instant::now();
    let mut result = CleanupResult::default();
    let cache = cache_dir();
    let snapshots_dir = cache.join("snapshots");
    let asset_pool = asset_pool_path();
    let temp_dir = std::env::temp_dir().join("xes-coding-helper");

    let snapshot_secs = SNAPSHOT_MAX_AGE_DAYS * 24 * 60 * 60;
    let asset_secs = ASSET_POOL_MAX_AGE_DAYS * 24 * 60 * 60;
    let temp_secs = TEMP_MAX_AGE_HOURS * 60 * 60;

    let (snap_count, snap_bytes) = clean_dir_older_than(&snapshots_dir, snapshot_secs).await;
    result.snapshots_removed = snap_count;
    result.snapshots_bytes_freed = snap_bytes;

    if let Some(parent) = snapshots_dir.parent() {
        let mut entries = match tokio::fs::read_dir(parent).await {
            Ok(e) => e,
            Err(_) => return result,
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("snapshot_") {
                let _ = remove_if_older_than(&entry.path(), snapshot_secs).await;
            }
        }
    }

    let (pool_count, pool_bytes) = clean_dir_older_than(&asset_pool, asset_secs).await;
    result.asset_pool_removed = pool_count;
    result.asset_pool_bytes_freed = pool_bytes;

    let (temp_count, temp_bytes) = clean_dir_older_than(&temp_dir, temp_secs).await;
    result.temp_removed = temp_count;
    result.temp_bytes_freed = temp_bytes;

    let lru = enforce_lru(&asset_pool, policy.max_asset_pool_bytes).await;
    result.lru_removed = lru.0;
    result.lru_bytes_freed = lru.1;
    result.ran_lru = lru.0 > 0;

    let snap_lru = enforce_snapshot_lru(&snapshots_dir, policy.max_snapshot_bytes, policy.max_snapshot_count).await;
    result.lru_removed += snap_lru.0;
    result.lru_bytes_freed += snap_lru.1;
    if snap_lru.0 > 0 {
        result.ran_lru = true;
    }

    result.duration_ms = start.elapsed().as_millis() as u64;
    result
}

async fn enforce_lru(dir: &Path, max_bytes: u64) -> (usize, u64) {
    if max_bytes == 0 {
        return (0, 0);
    }
    if !dir.exists() {
        return (0, 0);
    }
    let total = dir_size(dir).await;
    if total <= max_bytes {
        return (0, 0);
    }
    let mut entries: Vec<(PathBuf, u64, u64)> = Vec::new();
    if let Ok(mut rd) = tokio::fs::read_dir(dir).await {
        while let Ok(Some(entry)) = rd.next_entry().await {
            let path = entry.path();
            let meta = match entry.metadata().await {
                Ok(m) => m,
                Err(_) => continue,
            };
            let size = if meta.is_dir() { 0 } else { meta.len() };
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            entries.push((path, size, mtime));
        }
    }
    if entries.is_empty() {
        return (0, 0);
    }
    entries.sort_by_key(|(_, _, mtime)| *mtime);
    let mut current = total;
    let target = max_bytes;
    let mut removed = 0usize;
    let mut freed = 0u64;
    for (path, size, _) in entries {
        if current <= target {
            break;
        }
        let res = if path.is_dir() {
            tokio::fs::remove_dir_all(&path).await
        } else {
            tokio::fs::remove_file(&path).await
        };
        if res.is_ok() {
            current = current.saturating_sub(size);
            removed += 1;
            freed += size;
        }
    }
    (removed, freed)
}

async fn enforce_snapshot_lru(
    dir: &Path,
    max_bytes: u64,
    max_count: usize,
) -> (usize, u64) {
    if !dir.exists() {
        return (0, 0);
    }
    let mut entries: Vec<(PathBuf, u64, u64)> = Vec::new();
    if let Ok(mut rd) = tokio::fs::read_dir(dir).await {
        while let Ok(Some(entry)) = rd.next_entry().await {
            let meta = match entry.metadata().await {
                Ok(m) => m,
                Err(_) => continue,
            };
            if !meta.is_file() {
                continue;
            }
            let path = entry.path();
            let size = meta.len();
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            entries.push((path, size, mtime));
        }
    }
    if entries.is_empty() {
        return (0, 0);
    }
    let total: u64 = entries.iter().map(|(_, s, _)| s).sum();
    let by_count = entries.len() > max_count;
    let by_size = max_bytes > 0 && total > max_bytes;
    if !by_count && !by_size {
        return (0, 0);
    }
    entries.sort_by_key(|(_, _, mtime)| *mtime);
    let mut current = total;
    let mut current_count = entries.len();
    let mut removed = 0usize;
    let mut freed = 0u64;
    for (path, size, _) in entries {
        if (!by_size || current <= max_bytes) && (!by_count || current_count <= max_count) {
            break;
        }
        if tokio::fs::remove_file(&path).await.is_ok() {
            current = current.saturating_sub(size);
            current_count = current_count.saturating_sub(1);
            removed += 1;
            freed += size;
        }
    }
    (removed, freed)
}

pub async fn list_cache_info() -> CacheListInfo {
    let cache_root = cache_dir().to_string_lossy().to_string();
    let asset_dir = asset_path();
    let pool_dir = asset_pool_path();
    let snapshot_dir = cache_dir().join("snapshots");
    let venv_dir = cache_dir().join("venvs");
    let asset_bytes = dir_size(&asset_dir).await;
    let pool_bytes = dir_size(&pool_dir).await;
    let snapshot_bytes = dir_size(&snapshot_dir).await;
    let snapshot_count = count_files(&snapshot_dir).await;
    let venv_bytes = dir_size(&venv_dir).await;
    let venv_count = count_dirs(&venv_dir).await;
    let project_count = count_dirs(&asset_dir).await;
    CacheListInfo {
        cache_root,
        asset_bytes,
        asset_pool_bytes: pool_bytes,
        snapshot_bytes,
        snapshot_count,
        project_count,
        venv_bytes,
        venv_count,
    }
}

async fn dir_size(path: &Path) -> u64 {
    if !path.exists() {
        return 0;
    }
    let mut total = 0u64;
    let mut stack = vec![path.to_path_buf()];
    while let Some(p) = stack.pop() {
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

async fn count_files(path: &Path) -> usize {
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

async fn count_dirs(path: &Path) -> usize {
    if !path.exists() {
        return 0;
    }
    let Ok(mut rd) = tokio::fs::read_dir(path).await else {
        return 0;
    };
    let mut count = 0;
    while let Ok(Some(entry)) = rd.next_entry().await {
        if let Ok(meta) = entry.metadata().await {
            if meta.is_dir() {
                count += 1;
            }
        }
    }
    count
}

async fn run_cleanup_pass(policy: CleanupPolicy) {
    let result = run_cleanup_with_policy(&policy).await;
    if result.snapshots_removed + result.asset_pool_removed + result.temp_removed + result.lru_removed
        > 0
    {
        tracing::info!(
            "缓存清理完成: 快照 {} 项({} KB) 资产池 {} 项({} KB) 临时 {} 项({} KB) LRU {} 项({} KB)",
            result.snapshots_removed,
            result.snapshots_bytes_freed / 1024,
            result.asset_pool_removed,
            result.asset_pool_bytes_freed / 1024,
            result.temp_removed,
            result.temp_bytes_freed / 1024,
            result.lru_removed,
            result.lru_bytes_freed / 1024,
        );
    }
}

pub fn start() {
    let interval = Duration::from_secs(CLEANUP_INTERVAL_HOURS * 60 * 60);
    tokio::spawn(async move {
        let policy = CleanupPolicy::default();
        run_cleanup_pass(policy).await;
        let mut ticker = tokio::time::interval(interval);
        ticker.tick().await;
        loop {
            ticker.tick().await;
            let policy = CleanupPolicy::default();
            run_cleanup_pass(policy).await;
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

pub fn start_with_policy_handle(policy: CleanupPolicy) -> JoinHandle<()> {
    let interval = Duration::from_secs(CLEANUP_INTERVAL_HOURS * 60 * 60);
    tokio::spawn(async move {
        run_cleanup_pass(policy.clone()).await;
        let mut ticker = tokio::time::interval(interval);
        ticker.tick().await;
        loop {
            ticker.tick().await;
            run_cleanup_pass(policy.clone()).await;
        }
    })
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
