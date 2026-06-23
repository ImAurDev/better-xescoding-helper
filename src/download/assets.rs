use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::{oneshot, Mutex};

use crate::config::{asset_path, asset_pool_path, file_server_port};
use crate::http::port::is_port_available;
use crate::utils::flex::{flex_string, flex_string_opt};

static FILE_MANAGER: Lazy<Arc<Mutex<FileManagerInner>>> =
    Lazy::new(|| Arc::new(Mutex::new(FileManagerInner::new())));

const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36";

const CDNS: &[&str] = &[
    "http://static0.xesimg.com",
    "https://static0.xesimg.com",
    "http://static1.xesimg.com",
    "https://static1.xesimg.com",
    "http://static2.xesimg.com",
    "https://static2.xesimg.com",
    "http://static3.xesimg.com",
    "https://static3.xesimg.com",
    "http://static4.xesimg.com",
    "https://static4.xesimg.com",
    "http://static5.xesimg.com",
    "https://static5.xesimg.com",
    "http://static6.xesimg.com",
    "https://static6.xesimg.com",
    "http://static7.xesimg.com",
    "https://static7.xesimg.com",
    "http://static8.xesimg.com",
    "https://static8.xesimg.com",
    "http://static9.xesimg.com",
    "https://static9.xesimg.com",
    "http://static10.xesimg.com",
    "https://static10.xesimg.com",
    "https://livefile.xesimg.com",
    "https://livefile.xesv5.com",
    "https://livefile.xescdn.com",
    "http://livefile.xesimg.com",
    "http://livefile.xesv5.com",
    "http://livefile.xescdn.com",
];

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AssetInfo {
    #[serde(default, deserialize_with = "flex_string")]
    pub id: String,
    #[serde(default, deserialize_with = "flex_string")]
    pub name: String,
    #[serde(rename = "type", default)]
    pub asset_type: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "flex_string_opt"
    )]
    pub md5ext: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "assetId",
        deserialize_with = "flex_string_opt"
    )]
    pub asset_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub children: Option<Vec<AssetInfo>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AssetJson {
    #[serde(rename = "projectId", default, deserialize_with = "flex_string")]
    pub project_id: String,
    #[serde(default)]
    pub assets: Vec<AssetInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preload: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub xml: Option<String>,
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

#[allow(dead_code)]
pub struct AssetResponse {
    pub ok: bool,
    pub error: Option<String>,
}

#[derive(Clone)]
#[allow(dead_code)]
struct FileInfo {
    path: String,
    md5: String,
    uri: String,
    fid: String,
    cid: String,
}

#[allow(dead_code)]
pub enum CompareTag {
    Oversize,
    Count,
    Changed(Value),
}

struct FileManagerInner {
    file_map: HashMap<String, FileEntry>,
    cur_pid: String,
    cur_path: String,
    server_running: bool,
    shutdown: Option<oneshot::Sender<()>>,
}

#[derive(Clone)]
struct FileEntry {
    dict: String,
}

impl FileManagerInner {
    fn new() -> Self {
        Self {
            file_map: HashMap::new(),
            cur_pid: String::new(),
            cur_path: String::new(),
            server_running: false,
            shutdown: None,
        }
    }
}

fn xes_logger(clickname: &str, errmsg: &str) {
    tracing::info!("XES日志: clickname={clickname} errmsg={errmsg} source=XES");
}

fn get_str_md5(s: &str) -> String {
    use md5::{Digest, Md5};
    let mut hasher = Md5::new();
    hasher.update(s.as_bytes());
    let result = hasher.finalize();
    result.iter().map(|b| format!("{:02x}", b)).collect()
}

pub struct AssetManage {
    asset_path: PathBuf,
    asset_pool_path: PathBuf,
    asset_pool_files: Vec<String>,
    preload_path: PathBuf,
    preload_files: Vec<String>,
    json_info: Option<AssetJson>,
    assets: Vec<AssetInfo>,
    asset_map: HashMap<String, FileInfo>,
    dict_ids: HashMap<String, String>,
    local_map: std::collections::HashSet<String>,
}

impl AssetManage {
    pub fn new() -> Self {
        Self {
            asset_path: PathBuf::new(),
            asset_pool_path: asset_pool_path(),
            asset_pool_files: Vec::new(),
            preload_path: PathBuf::new(),
            preload_files: Vec::new(),
            json_info: None,
            assets: Vec::new(),
            asset_map: HashMap::new(),
            dict_ids: HashMap::new(),
            local_map: std::collections::HashSet::new(),
        }
    }

    async fn init(&mut self) -> Result<(), String> {
        if !self.asset_pool_path.exists() {
            tokio::fs::create_dir_all(&self.asset_pool_path)
                .await
                .map_err(|e| e.to_string())?;
        }
        self.asset_pool_files = self.get_files(&self.asset_pool_path).await;
        Ok(())
    }

    pub async fn handle_assets_json(&mut self, json_info: AssetJson) -> AssetResponse {
        if let Err(e) = self.init().await {
            return AssetResponse {
                ok: false,
                error: Some(e),
            };
        }
        self.json_info = Some(json_info.clone());
        self.asset_path = asset_path().join(&json_info.project_id);
        let asset_info_path = self.asset_path.join("asset_info.json");

        if !self.asset_path.exists() {
            let result = self.download_asset_by_json(&json_info).await;
            if let Err(e) = result {
                return AssetResponse {
                    ok: false,
                    error: Some(e),
                };
            }
            let write_res = self
                .create_file(
                    &asset_info_path,
                    &serde_json::to_vec(&json_info).unwrap_or_default(),
                )
                .await;
            if let Err(e) = write_res {
                return AssetResponse {
                    ok: false,
                    error: Some(e),
                };
            }
            return AssetResponse {
                ok: true,
                error: None,
            };
        }

        let local_json = self.get_local_json(&asset_info_path).await;
        match local_json {
            Err(_) => {
                let _ = self.del_files(&self.asset_path, true).await;
                let result = self.download_asset_by_json(&json_info).await;
                if let Err(e) = result {
                    return AssetResponse {
                        ok: false,
                        error: Some(e),
                    };
                }
                let _ = self
                    .create_file(
                        &asset_info_path,
                        &serde_json::to_vec(&json_info).unwrap_or_default(),
                    )
                    .await;
                return AssetResponse {
                    ok: true,
                    error: None,
                };
            }
            Ok(local) => {
                let local_md5 = get_str_md5(&serde_json::to_string(&local).unwrap_or_default());
                let new_md5 = get_str_md5(&serde_json::to_string(&json_info).unwrap_or_default());
                if local_md5 != new_md5 {
                    let _ = self.del_files(&self.asset_path, true).await;
                    let result = self.download_asset_by_json(&json_info).await;
                    if let Err(e) = result {
                        return AssetResponse {
                            ok: false,
                            error: Some(e),
                        };
                    }
                    let _ = self
                        .create_file(
                            &asset_info_path,
                            &serde_json::to_vec(&json_info).unwrap_or_default(),
                        )
                        .await;
                }
                return AssetResponse {
                    ok: true,
                    error: None,
                };
            }
        }
    }

    async fn get_local_json(&self, file_name: &Path) -> Result<AssetJson, String> {
        let content = tokio::fs::read_to_string(file_name).await.map_err(|e| {
            let msg = format!("读取本地json文件 {} 错误 {}", file_name.display(), e);
            xes_logger("read_local_json_err", &msg);
            msg
        })?;
        serde_json::from_str::<AssetJson>(&content).map_err(|e| {
            let msg = format!("解析本地json错误: {e}");
            xes_logger("read_local_json_err", &msg);
            msg
        })
    }

    async fn get_files(&self, dir: &Path) -> Vec<String> {
        let mut names = Vec::new();
        let mut entries = match tokio::fs::read_dir(dir).await {
            Ok(e) => e,
            Err(_) => return Vec::new(),
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            if let Ok(meta) = entry.file_type().await {
                if meta.is_file() {
                    names.push(entry.file_name().to_string_lossy().to_string());
                }
            }
        }
        names
    }

    async fn download_asset_by_url(&self, url: &str) -> Option<Vec<u8>> {
        let client = crate::settings::build_proxy_client();
        let result = crate::utils::retry::retry_async(&crate::utils::retry::RetryPolicy::network(), || {
            let client = client.clone();
            let url = url.to_string();
            async move {
                match client
                    .get(&url)
                    .header("Accept", "*/*")
                    .header("Accept-Language", "zh-CN,zh;q=0.9,en;q=0.8")
                    .header("Referer", "https://code.xueersi.com/")
                    .send()
                    .await
                {
                    Ok(res) => {
                        let status = res.status().as_u16();
                        if status == 200 || status == 304 {
                            res.bytes().await.ok().map(|b| b.to_vec()).ok_or_else(|| {
                                format!("download_asset {url} 读取失败")
                            })
                        } else if crate::utils::retry::is_retryable_status(status) {
                            Err(format!("status {status}"))
                        } else {
                            xes_logger(
                                "download_assets",
                                &format!("download_asset {url} 失败 {status}"),
                            );
                            Err(format!("status {status}"))
                        }
                    }
                    Err(e) => {
                        xes_logger(
                            "download_assets",
                            &format!("download_asset {url} 错误 {}", e),
                        );
                        Err(e.to_string())
                    }
                }
            }
        })
        .await;
        match result {
            Ok(b) => Some(b),
            Err(_) => None,
        }
    }

    async fn download_from_cdns(&self, md5ext: &str) -> Option<Vec<u8>> {
        for cdn in CDNS {
            let url = format!("{}/programme/python_assets/{}", cdn, md5ext);
            if let Some(data) = self.download_asset_by_url(&url).await {
                return Some(data);
            }
        }
        None
    }

    async fn download_asset_by_json(&mut self, json_info: &AssetJson) -> Result<(), String> {
        if !self.asset_path.exists() {
            tokio::fs::create_dir_all(&self.asset_path)
                .await
                .map_err(|e| e.to_string())?;
        }
        self.assets.clear();
        self.build_dict(&self.asset_path.clone(), &json_info.assets, PathBuf::new())
            .await;

        let pool_path = self.asset_pool_path.clone();
        let preload_path = self.preload_path.clone();
        let pool_files = self.asset_pool_files.clone();
        let preload_files = self.preload_files.clone();

        for asset in self.assets.clone() {
            let md5ext = match &asset.md5ext {
                Some(m) if !m.is_empty() => m.clone(),
                _ => continue,
            };
            let asset_dir = match &asset.path {
                Some(p) => PathBuf::from(p),
                None => continue,
            };
            if pool_files.contains(&md5ext) {
                self.copy_and_rename(&pool_path, &md5ext, &asset_dir, &asset.name)
                    .await?;
                continue;
            }
            if preload_files.contains(&md5ext) {
                self.copy_and_rename(&preload_path, &md5ext, &asset_dir, &asset.name)
                    .await?;
                continue;
            }
            let data = match self.download_from_cdns(&md5ext).await {
                Some(d) => d,
                None => return Err("资源下载失败，请重试".into()),
            };
            self.create_file(&pool_path.join(&md5ext), &data).await?;
            self.copy_and_rename(&pool_path, &md5ext, &asset_dir, &asset.name)
                .await?;
        }
        Ok(())
    }

    async fn copy_and_rename(
        &self,
        src_dir: &Path,
        src_name: &str,
        dst_dir: &Path,
        dst_name: &str,
    ) -> Result<(), String> {
        let src_path = src_dir.join(src_name);
        let dst_path = dst_dir.join(src_name);
        self.copy_file(&src_path, dst_dir).await?;
        match tokio::fs::rename(&dst_path, dst_dir.join(dst_name)).await {
            Ok(_) => Ok(()),
            Err(e) => {
                let msg = format!("重命名文件错误: {e}");
                xes_logger("rename_file_err", &msg);
                Err(msg)
            }
        }
    }

    async fn copy_file(&self, src_file: &Path, dst_dir: &Path) -> Result<(), String> {
        let dst_path = dst_dir.join(src_file.file_name().unwrap_or_default());
        match tokio::fs::copy(src_file, &dst_path).await {
            Ok(_) => Ok(()),
            Err(e) => {
                let msg = format!("复制文件错误: {e}");
                xes_logger("copy_file_err", &msg);
                self.handle_err(&msg);
                Err(msg)
            }
        }
    }

    async fn create_file(&self, file_name: &Path, content: &[u8]) -> Result<(), String> {
        if let Some(parent) = file_name.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|e| {
                let msg = format!("create file {} 错误 {}", file_name.display(), e);
                xes_logger("create_file_err", &msg);
                msg
            })?;
        }
        tokio::fs::write(file_name, content).await.map_err(|e| {
            let msg = format!("create file {} 错误 {}", file_name.display(), e);
            xes_logger("create_file_err", &msg);
            self.handle_err(&msg);
            msg
        })
    }

    async fn del_files(&self, path: &Path, del_self: bool) -> Result<(), String> {
        if !path.exists() {
            return Ok(());
        }
        let mut entries = match tokio::fs::read_dir(path).await {
            Ok(e) => e,
            Err(e) => {
                let msg = format!("delete file {} 错误 {}", path.display(), e);
                xes_logger("delete_file_err", &msg);
                return Err(msg);
            }
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let full_path = entry.path();
            match entry.file_type().await {
                Ok(ft) if ft.is_dir() => {
                    Box::pin(self.del_files(&full_path, true)).await?;
                }
                _ => {
                    let _ = tokio::fs::remove_file(&full_path).await;
                }
            }
        }
        if del_self {
            let _ = tokio::fs::remove_dir_all(path).await;
        }
        Ok(())
    }

    async fn build_dict(&mut self, path: &Path, children: &[AssetInfo], rela_path: PathBuf) {
        for child in children {
            if child.disabled == Some(true) {
                continue;
            }
            let cur_path = path.join(&child.name);
            let next_rela = rela_path.join(&child.name);
            match child.asset_type.as_str() {
                "dir" => {
                    if !cur_path.exists() {
                        let _ = tokio::fs::create_dir_all(&cur_path).await;
                    }
                    if let Some(grandchildren) = &child.children {
                        if !grandchildren.is_empty() {
                            Box::pin(self.build_dict(&cur_path, grandchildren, next_rela)).await;
                        }
                    }
                }
                "oss_file" => {
                    let mut c = child.clone();
                    c.path = Some(path.to_string_lossy().to_string());
                    {
                        let mut fm = FILE_MANAGER.lock().await;
                        fm.file_map.insert(
                            c.id.clone(),
                            FileEntry {
                                dict: next_rela.to_string_lossy().to_string(),
                            },
                        );
                    }
                    self.assets.push(c);
                }
                "local_file" => {
                    let _ = self
                        .create_file(
                            &cur_path,
                            child.value.clone().unwrap_or_default().as_bytes(),
                        )
                        .await;
                    let mut fm = FILE_MANAGER.lock().await;
                    fm.file_map.insert(
                        child.id.clone(),
                        FileEntry {
                            dict: next_rela.to_string_lossy().to_string(),
                        },
                    );
                }
                _ => {}
            }
        }
    }

    fn build_assets_map(
        &mut self,
        path: &Path,
        children: &[AssetInfo],
        rela_path: &Path,
        fid: &str,
    ) {
        for child in children {
            if child.disabled == Some(true) {
                continue;
            }
            let cur_path = path.join(&child.name);
            let next_rela = rela_path.join(&child.name);
            if child.asset_type == "dir" {
                self.dict_ids
                    .insert(next_rela.to_string_lossy().to_string(), child.id.clone());
                if let Some(grandchildren) = &child.children {
                    if !grandchildren.is_empty() {
                        self.build_assets_map(&cur_path, grandchildren, &next_rela, &child.id);
                    }
                }
            } else {
                let file_key = rela_path.join(&child.name);
                let port = file_server_port();
                let info = FileInfo {
                    path: format!("http://127.0.0.1:{}/{}", port, file_key.to_string_lossy()),
                    md5: child
                        .asset_id
                        .clone()
                        .unwrap_or_else(|| get_str_md5(&child.value.clone().unwrap_or_default())),
                    uri: child.name.clone(),
                    fid: fid.to_string(),
                    cid: child.id.clone(),
                };
                if child.asset_type == "local_file" {
                    self.local_map
                        .insert(file_key.to_string_lossy().to_string());
                }
                self.asset_map
                    .insert(file_key.to_string_lossy().to_string(), info);
            }
        }
    }

    fn build_dict_map(&self, _path: &Path) -> u64 {
        0
    }

    pub fn compare_assets(&mut self) -> CompareTag {
        self.asset_map.clear();
        self.dict_ids.clear();
        let json_info = match &self.json_info {
            Some(j) => j.clone(),
            None => return CompareTag::Changed(json!({})),
        };
        self.build_assets_map(
            &self.asset_path.clone(),
            &json_info.assets,
            Path::new(""),
            "root",
        );
        let size = self.build_dict_map(&self.asset_path);
        if size / 1024 / 1024 > 20 {
            return CompareTag::Oversize;
        }
        CompareTag::Changed(json!({
            "new": [],
            "del": [],
            "mod": [],
            "dir_del": [],
            "dir_new": [],
        }))
    }

    fn handle_err(&self, msg: &str) {
        if msg.contains("No space left on device") {
            tracing::debug!("磁盘空间不足！");
        }
    }
}

pub async fn get_local_path(pid: &str, fid: &str) -> Option<String> {
    let dict = {
        let fm = FILE_MANAGER.lock().await;
        fm.file_map.get(fid).map(|e| e.dict.clone())
    };
    let dict = dict?;
    tracing::debug!("getLocalPath fid={fid} dict={dict}");

    let need_start = {
        let fm = FILE_MANAGER.lock().await;
        !fm.server_running || fm.cur_pid != pid
    };

    if need_start {
        let mut asset_path = asset_path().join(pid);
        if !asset_path.is_absolute() {
            if let Ok(cwd) = std::env::current_dir() {
                asset_path = cwd.join(asset_path);
            }
        }
        let success = start_file_server(pid.to_string(), asset_path.clone()).await;
        if !success {
            return None;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let url = format!("http://127.0.0.1:{}/", file_server_port());
        match reqwest::Client::new().head(&url).send().await {
            Ok(r) if r.status().is_success() || r.status().as_u16() == 404 => {}
            _ => return None,
        }
    }
    Some(dict.replace('\\', "/"))
}

async fn start_file_server(pid: String, asset_path: PathBuf) -> bool {
    {
        let mut fm = FILE_MANAGER.lock().await;
        if let Some(tx) = fm.shutdown.take() {
            let _ = tx.send(());
        }
        fm.server_running = false;
    }
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    if !asset_path.exists() {
        tracing::error!("资源路径不存在: {}", asset_path.display());
        return false;
    }

    if !is_port_available(file_server_port()).await {
        tracing::error!("端口仍被占用: {}", file_server_port());
        return false;
    }

    let listener = match tokio::net::TcpListener::bind(("0.0.0.0", file_server_port())).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!("无法开启文件服务器: {e}");
            return false;
        }
    };

    let serve_dir = tower_http::services::ServeDir::new(asset_path.clone());
    let app = axum::Router::new().fallback_service(serve_dir);
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

    {
        let mut fm = FILE_MANAGER.lock().await;
        fm.cur_pid = pid;
        fm.cur_path = asset_path.to_string_lossy().to_string();
        fm.server_running = true;
        fm.shutdown = Some(shutdown_tx);
    }
    tracing::info!("文件服务器已开启: {}", file_server_port());

    tokio::spawn(async move {
        let _ = axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _ = shutdown_rx.await;
            })
            .await;
        let mut fm = FILE_MANAGER.lock().await;
        fm.server_running = false;
    });
    true
}
