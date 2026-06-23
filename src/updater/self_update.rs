use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::json;
use tokio::sync::Mutex;

use crate::config::cache_dir;
use crate::settings::Settings;
use crate::updater::github::{
    fetch_latest_release, parse_version, pick_asset, ReleaseAsset, ReleaseInfo,
};

pub struct UpdaterService {
    settings: std::sync::Arc<Mutex<Settings>>,
    cached: std::sync::Arc<Mutex<Option<ReleaseInfo>>>,
    http: reqwest::Client,
}

impl UpdaterService {
    pub fn new(settings: std::sync::Arc<Mutex<Settings>>) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent("xescoding-helper-updater")
            .build()
            .unwrap_or_default();
        Self {
            settings,
            cached: std::sync::Arc::new(Mutex::new(None)),
            http,
        }
    }

    pub fn current_version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    pub async fn status(&self) -> serde_json::Value {
        let cfg = self.settings.lock().await.updater.clone();
        let cached = self.cached.lock().await.clone();
        let current = semver::Version::parse(self.current_version()).ok();
        let latest_version = cached
            .as_ref()
            .and_then(|r| parse_version(&r.tag_name));
        let update_available = match (current, latest_version) {
            (Some(a), Some(b)) => b > a,
            _ => false,
        };
        let target = crate::updater::github::target_asset_name();
        json!({
            "enabled": cfg.enabled,
            "current_version": self.current_version(),
            "repo": cfg.effective_repo(),
            "check_interval_hours": cfg.check_interval_hours,
            "last_check_ts": cfg.last_check_ts,
            "target_asset": target,
            "cached_release": cached,
            "update_available": update_available,
        })
    }

    pub async fn check(&self, force: bool) -> Result<ReleaseInfo, String> {
        let (repo, include_pre) = {
            let s = self.settings.lock().await;
            (s.updater.effective_repo(), s.updater.include_prerelease)
        };
        let now_ms = crate::history::now_millis();
        let should_check = {
            let s = self.settings.lock().await;
            force
                || s.updater.last_check_ts == 0
                || now_ms - s.updater.last_check_ts
                    > (s.updater.check_interval_hours.max(1) as i64) * 3600 * 1000
        };
        if !should_check {
            if let Some(c) = self.cached.lock().await.clone() {
                return Ok(c);
            }
        }
        let release = fetch_latest_release(&self.http, &repo, include_pre).await?;
        *self.cached.lock().await = Some(release.clone());
        {
            let mut s = self.settings.lock().await;
            s.updater.last_check_ts = now_ms;
        }
        crate::settings::persist_arc(&self.settings).await;
        Ok(release)
    }

    pub async fn download(&self, release: &ReleaseInfo) -> Result<PathBuf, String> {
        let asset = pick_asset(release)
            .ok_or_else(|| "当前平台没有匹配的发布资产".to_string())?;
        download_asset(&self.http, asset).await
    }

    pub async fn apply(&self, asset_path: &Path) -> Result<ApplyReport, String> {
        let current = std::env::current_exe()
            .map_err(|e| format!("获取当前可执行文件路径失败: {e}"))?;
        replace_executable(&current, asset_path).await?;
        Ok(ApplyReport {
            new_executable: current.to_string_lossy().to_string(),
            restart_required: true,
        })
    }

    pub async fn perform_update(&self) -> Result<ApplyReport, String> {
        let release = self.check(false).await?;
        let asset = pick_asset(&release)
            .ok_or_else(|| "当前平台没有匹配的发布资产".to_string())?;
        let path = download_asset(&self.http, asset).await?;
        self.apply(&path).await
    }

    pub async fn auto_check(&self) {
        let enabled = {
            let s = self.settings.lock().await;
            s.updater.enabled && s.updater.auto_check
        };
        if !enabled {
            return;
        }
        match self.check(false).await {
            Ok(r) => {
                let cur = semver::Version::parse(self.current_version()).ok();
                let new = parse_version(&r.tag_name);
                if let (Some(a), Some(b)) = (cur, new) {
                    if b > a {
                        tracing::info!("发现新版本: {} -> {}", a, b);
                    }
                }
            }
            Err(e) => tracing::warn!("updater auto check failed: {e}"),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ApplyReport {
    pub new_executable: String,
    pub restart_required: bool,
}

async fn download_asset(
    client: &reqwest::Client,
    asset: &ReleaseAsset,
) -> Result<PathBuf, String> {
    let staging_dir = cache_dir().join("updates");
    tokio::fs::create_dir_all(&staging_dir)
        .await
        .map_err(|e| format!("创建缓存目录失败: {e}"))?;
    let safe_name = asset
        .name
        .replace(['/', '\\', ':', '*', '?', '"', '<', '>', '|'], "_");
    let dest = staging_dir.join(safe_name);
    tracing::info!("下载更新: {} -> {:?}", asset.browser_download_url, dest);
    let resp = client
        .get(&asset.browser_download_url)
        .header("User-Agent", "xescoding-helper-updater")
        .send()
        .await
        .map_err(|e| format!("下载失败: {e}"))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!("下载失败 ({}): {}", status.as_u16(), asset.name));
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("读取下载内容失败: {e}"))?;
    tokio::fs::write(&dest, &bytes)
        .await
        .map_err(|e| format!("写入文件失败: {e}"))?;
    Ok(dest)
}

#[cfg(windows)]
async fn replace_executable(
    current: &Path,
    new_path: &Path,
) -> Result<(), String> {
    let backup = current.with_extension("old.exe");
    if backup.exists() {
        let _ = tokio::fs::remove_file(&backup).await;
    }
    if let Err(e) = tokio::fs::rename(current, &backup).await {
        return Err(format!("备份当前可执行文件失败: {e}"));
    }
    if let Err(e) = tokio::fs::copy(new_path, current).await {
        let _ = tokio::fs::rename(&backup, current).await;
        return Err(format!("替换可执行文件失败: {e}"));
    }
    Ok(())
}

#[cfg(not(windows))]
async fn replace_executable(
    current: &Path,
    new_path: &Path,
) -> Result<(), String> {
    let _ = tokio::fs::copy(new_path, current)
        .await
        .map_err(|e| format!("覆盖可执行文件失败: {e}"))?;
    use std::os::unix::fs::PermissionsExt;
    let mut perms = tokio::fs::metadata(current)
        .await
        .map_err(|e| format!("读取权限失败: {e}"))?
        .permissions();
    perms.set_mode(0o755);
    tokio::fs::set_permissions(current, perms.clone())
        .await
        .map_err(|e| format!("设置可执行权限失败: {e}"))?;
    let _ = perms;
    Ok(())
}
