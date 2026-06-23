use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use once_cell::sync::OnceCell;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::config::cache_dir;

static PROXY_OVERRIDE: OnceCell<std::sync::RwLock<ProxyConfig>> = OnceCell::new();

fn settings_path() -> PathBuf {
    cache_dir().join("settings.json")
}

pub fn current_proxy() -> ProxyConfig {
    PROXY_OVERRIDE
        .get()
        .and_then(|r| r.read().ok().map(|c| c.clone()))
        .unwrap_or_default()
}

pub fn set_global_proxy(p: ProxyConfig) {
    let cell = PROXY_OVERRIDE.get_or_init(|| std::sync::RwLock::new(ProxyConfig::default()));
    if let Ok(mut w) = cell.write() {
        *w = p;
    }
}

pub fn build_proxy_client() -> reqwest::Client {
    let proxy = current_proxy();
    let mut builder = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36");
    if let Some(p) = proxy.https.as_ref().or(proxy.http.as_ref()) {
        match reqwest::Proxy::all(p) {
            Ok(prx) => {
                builder = builder.proxy(prx);
            }
            Err(e) => {
                tracing::warn!("代理配置无效: {e}");
            }
        }
    }
    if let Some(np) = &proxy.no_proxy {
        if let Ok(prx) = reqwest::Proxy::all(np) {
            builder = builder.proxy(prx);
        }
    }
    builder.build().unwrap_or_default()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ProxyConfig {
    pub http: Option<String>,
    pub https: Option<String>,
    pub no_proxy: Option<String>,
    pub pip_index: Option<String>,
    pub asset_cdn_override: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CleanupPolicy {
    pub max_cache_bytes: u64,
    pub max_asset_pool_bytes: u64,
    pub max_snapshot_bytes: u64,
    pub max_snapshot_count: usize,
    pub run_metrics_history: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct VenvConfig {
    pub enabled: bool,
    pub inherit_base_packages: bool,
    pub pinned_packages: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PrewarmConfig {
    pub enabled: bool,
    pub packages: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AiConfig {
    pub enabled: bool,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub model: Option<String>,
    pub timeout_secs: u64,
    pub max_tokens: u32,
    pub temperature: f32,
    pub system_prompt: Option<String>,
    pub auto_explain_on_error: bool,
}

impl AiConfig {
    pub fn effective_base_url(&self) -> String {
        self.base_url
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "https://api.openai.com/v1".to_string())
    }
    pub fn effective_model(&self) -> String {
        self.model
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "gpt-4o-mini".to_string())
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdaterConfig {
    pub enabled: bool,
    pub repo: Option<String>,
    pub channel: Option<String>,
    pub auto_check: bool,
    pub check_interval_hours: u64,
    pub include_prerelease: bool,
    pub last_check_ts: i64,
}

impl UpdaterConfig {
    pub fn effective_repo(&self) -> String {
        self.repo
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "anomalyco/better-xescoding-helper".to_string())
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SandboxConfig {
    pub enabled: bool,
    pub mode: Option<String>,
    pub memory_limit_bytes: u64,
    pub cpu_time_limit_secs: u64,
    pub no_network: bool,
    pub read_only_paths: Vec<String>,
    pub writable_paths: Vec<String>,
    pub drop_capabilities: bool,
}

impl SandboxConfig {
    pub fn effective_mode(&self) -> String {
        self.mode
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "auto".to_string())
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RunLimits {
    pub python_timeout_secs: u64,
    pub go_timeout_secs: u64,
    pub ts_timeout_secs: u64,
    pub auto_install_on_missing: bool,
    pub lint_before_run: bool,
    pub detect_missing_imports: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub proxy: ProxyConfig,
    pub env_vars: HashMap<String, HashMap<String, String>>,
    pub cleanup: CleanupPolicy,
    pub venv: VenvConfig,
    pub prewarm: PrewarmConfig,
    pub run_limits: RunLimits,
    pub max_concurrent_runners: usize,
    pub ai: AiConfig,
    pub updater: UpdaterConfig,
    pub sandbox: SandboxConfig,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            proxy: ProxyConfig::default(),
            env_vars: HashMap::new(),
            cleanup: CleanupPolicy {
                max_cache_bytes: 4 * 1024 * 1024 * 1024,
                max_asset_pool_bytes: 8 * 1024 * 1024 * 1024,
                max_snapshot_bytes: 256 * 1024 * 1024,
                max_snapshot_count: 50,
                run_metrics_history: 200,
            },
            venv: VenvConfig {
                enabled: false,
                inherit_base_packages: true,
                pinned_packages: vec![
                    "pip".to_string(),
                    "setuptools".to_string(),
                    "wheel".to_string(),
                ],
            },
            prewarm: PrewarmConfig {
                enabled: false,
                packages: vec![
                    "xes-lib".to_string(),
                    "Pillow".to_string(),
                    "qrcode".to_string(),
                ],
            },
            ai: AiConfig {
                enabled: false,
                base_url: None,
                api_key: None,
                model: None,
                timeout_secs: 30,
                max_tokens: 1024,
                temperature: 0.2,
                system_prompt: None,
                auto_explain_on_error: false,
            },
            updater: UpdaterConfig {
                enabled: false,
                repo: None,
                channel: None,
                auto_check: false,
                check_interval_hours: 24,
                include_prerelease: false,
                last_check_ts: 0,
            },
            sandbox: SandboxConfig {
                enabled: false,
                mode: None,
                memory_limit_bytes: 512 * 1024 * 1024,
                cpu_time_limit_secs: 30,
                no_network: false,
                read_only_paths: Vec::new(),
                writable_paths: Vec::new(),
                drop_capabilities: true,
            },
            run_limits: RunLimits {
                python_timeout_secs: 30,
                go_timeout_secs: 60,
                ts_timeout_secs: 60,
                auto_install_on_missing: true,
                lint_before_run: false,
                detect_missing_imports: true,
            },
            max_concurrent_runners: 4,
        }
    }
}

pub struct SettingsStore {
    inner: Arc<Mutex<Settings>>,
    path: PathBuf,
}

impl SettingsStore {
    pub async fn load() -> Self {
        let path = settings_path();
        let mut settings = Settings::default();
        if let Ok(content) = tokio::fs::read_to_string(&path).await {
            if !content.trim().is_empty() {
                match serde_json::from_str::<Settings>(&content) {
                    Ok(s) => settings = s,
                    Err(e) => tracing::warn!("settings 解析失败,使用默认: {e}"),
                }
            }
        }
        Self {
            inner: Arc::new(Mutex::new(settings)),
            path,
        }
    }

    pub fn shared(self) -> Arc<Mutex<Settings>> {
        self.inner
    }

    pub async fn snapshot(&self) -> Settings {
        self.inner.lock().await.clone()
    }

    pub async fn mutate<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&mut Settings) -> R,
    {
        let mut guard = self.inner.lock().await;
        let r = f(&mut guard);
        let data = match serde_json::to_string_pretty(&*guard) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("settings 序列化失败: {e}");
                return r;
            }
        };
        if let Some(parent) = self.path.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
        if let Err(e) = tokio::fs::write(&self.path, data).await {
            tracing::error!("settings 持久化失败: {e}");
        }
        r
    }
}

pub async fn mutate_arc<F, R>(arc: &Arc<Mutex<Settings>>, f: F) -> R
where
    F: FnOnce(&mut Settings) -> R,
{
    let mut guard = arc.lock().await;
    f(&mut guard)
}

pub async fn persist_arc(arc: &Arc<Mutex<Settings>>) {
    let snapshot = arc.lock().await.clone();
    let path = settings_path();
    if let Some(parent) = path.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    let data = match serde_json::to_string_pretty(&snapshot) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("settings 序列化失败: {e}");
            return;
        }
    };
    if let Err(e) = tokio::fs::write(&path, data).await {
        tracing::error!("settings 持久化失败: {e}");
    }
}
