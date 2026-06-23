use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use tokio::process::Command;
use tokio::sync::Mutex;

use crate::python::package::PackageData;
use crate::utils::executor::find_first_executable;

#[derive(Debug, Clone, Serialize)]
pub struct Mirror {
    pub name: String,
    pub mirror: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipPackage {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProcessInfo {
    pub name: String,
    pub progress: u32,
    pub state: String,
    pub msg: String,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct InstallRequest {
    pub name: String,
    pub version: Option<String>,
    pub url: Option<String>,
    pub pip_source: Option<String>,
}

#[derive(Clone)]
pub struct PackageManager {
    inner: Arc<Mutex<PmInner>>,
    cancel: Arc<AtomicBool>,
}

struct PmInner {
    mirrors: Vec<Mirror>,
    mirror_index: usize,
    python_path: String,
    user_lib_path: PathBuf,
    current_installing: Option<String>,
    last_completed_info: Option<ProcessInfo>,
    local_list_cache: Option<(Instant, LocalListData)>,
}

#[derive(Clone)]
struct LocalListData {
    user: Vec<PipPackage>,
    lib: Vec<PipPackage>,
}

impl PackageManager {
    pub fn new() -> Self {
        let user_lib_path = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".thonny")
            .join("lib");
        Self {
            inner: Arc::new(Mutex::new(PmInner {
                mirrors: vec![
                    Mirror {
                        name: "默认".into(),
                        mirror: "https://mirrors.aliyun.com/pypi/simple/".into(),
                    },
                    Mirror {
                        name: "清华".into(),
                        mirror: "https://pypi.tuna.tsinghua.edu.cn/simple".into(),
                    },
                    Mirror {
                        name: "豆瓣".into(),
                        mirror: "https://pypi.douban.com/simple/".into(),
                    },
                    Mirror {
                        name: "Python 官方".into(),
                        mirror: "https://pypi.org/simple".into(),
                    },
                ],
                mirror_index: 0,
                python_path: "python".into(),
                user_lib_path,
                current_installing: None,
                last_completed_info: None,
                local_list_cache: None,
            })),
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }

    pub async fn init(&self) {
        let candidates: &[&str] = if cfg!(windows) {
            &["python", "py", "python3"]
        } else {
            &["python3", "python"]
        };
        let py = find_first_executable(candidates, None);
        let mut inner = self.inner.lock().await;
        inner.python_path = py;
        if !inner.user_lib_path.exists() {
            let _ = tokio::fs::create_dir_all(&inner.user_lib_path).await;
        }
    }

    pub async fn get_mirrors(&self) -> (Vec<Mirror>, usize) {
        let inner = self.inner.lock().await;
        (inner.mirrors.clone(), inner.mirror_index)
    }

    pub async fn set_mirror_index(&self, index: usize) {
        let mut inner = self.inner.lock().await;
        inner.mirror_index = index;
    }

    pub async fn get_local_list(&self) -> (Vec<PipPackage>, Vec<PipPackage>) {
        {
            let inner = self.inner.lock().await;
            if let Some((t, data)) = &inner.local_list_cache {
                if t.elapsed() < std::time::Duration::from_secs(5) {
                    return (data.user.clone(), data.lib.clone());
                }
            }
        }
        let python_path = self.inner.lock().await.python_path.clone();
        let user_lib_path = self.inner.lock().await.user_lib_path.clone();
        let user = list_packages(&python_path, &user_lib_path).await;
        let data = LocalListData {
            user: user.clone(),
            lib: Vec::new(),
        };
        let mut inner = self.inner.lock().await;
        inner.local_list_cache = Some((Instant::now(), data));
        (user, Vec::new())
    }

    #[allow(dead_code)]
    pub async fn get_module_info(
        &self,
        package_name: &str,
    ) -> std::collections::HashMap<String, String> {
        let python_path = self.inner.lock().await.python_path.clone();
        let output = Command::new(&python_path)
            .args(["-m", "pip", "show", package_name])
            .output()
            .await;
        let mut info = std::collections::HashMap::new();
        if let Ok(out) = output {
            let stdout = String::from_utf8_lossy(&out.stdout);
            for line in stdout.lines() {
                if let Some(idx) = line.find(':') {
                    let key = line[..idx].trim().to_string();
                    let value = line[idx + 1..].trim().to_string();
                    info.insert(key, value);
                }
            }
        }
        info
    }

    pub async fn handle_install(&self, pack: InstallRequest) -> String {
        let mut inner = self.inner.lock().await;
        if inner.current_installing.is_some() {
            return "waiting".to_string();
        }
        inner.current_installing = Some(pack.name.clone());
        drop(inner);
        let this = self.clone();
        tokio::spawn(async move {
            this.run_single_install(&pack).await;
        });
        "installing".to_string()
    }

    async fn run_single_install(&self, pack: &InstallRequest) -> ProcessInfo {
        let result = self.do_install(pack).await;
        let mut inner = self.inner.lock().await;
        inner.last_completed_info = Some(result.clone());
        inner.current_installing = None;
        result
    }

    async fn do_install(&self, pack: &InstallRequest) -> ProcessInfo {
        let (python_path, user_lib_path, mirror) = {
            let inner = self.inner.lock().await;
            let mirror = inner.mirrors.get(inner.mirror_index).cloned();
            (
                inner.python_path.clone(),
                inner.user_lib_path.clone(),
                mirror,
            )
        };

        let mut args: Vec<String> = vec!["-m".into(), "pip".into(), "install".into()];
        if let Some(url) = &pack.url {
            args.push("--target".into());
            args.push(user_lib_path.to_string_lossy().to_string());
            args.push(url.clone());
            args.push("--upgrade".into());
        } else {
            let mirror = match mirror {
                Some(m) => m,
                None => {
                    return ProcessInfo {
                        name: pack.name.clone(),
                        progress: 0,
                        state: "error".into(),
                        msg: "镜像索引无效".into(),
                    };
                }
            };
            let mut package_name = pack.name.clone();
            if let Some(v) = &pack.version {
                if !v.is_empty() {
                    package_name.push_str("==");
                    package_name.push_str(v);
                }
            }
            args.push("--target".into());
            args.push(user_lib_path.to_string_lossy().to_string());
            args.push(package_name);
            args.push("--no-cache-dir".into());
            args.push("--no-warn-script-location".into());
            args.push("--upgrade".into());
            args.push("--index-url".into());
            args.push(mirror.mirror);
        }

        let mut cmd = Command::new(&python_path);
        cmd.args(&args)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                return ProcessInfo {
                    name: pack.name.clone(),
                    progress: 0,
                    state: "error".into(),
                    msg: e.to_string(),
                };
            }
        };

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let stderr_buf: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
        if let Some(mut err) = stderr {
            let buf = stderr_buf.clone();
            tokio::spawn(async move {
                use tokio::io::AsyncReadExt;
                let mut data = Vec::new();
                let _ = err.read_to_end(&mut data).await;
                *buf.lock().await = String::from_utf8_lossy(&data).to_string();
            });
        }

        let mut last_msg = String::from("开始安装");
        if let Some(out) = stdout {
            use tokio::io::AsyncBufReadExt;
            let reader = tokio::io::BufReader::new(out);
            let mut lines = reader.lines();
            loop {
                tokio::select! {
                    line = lines.next_line() => {
                        match line {
                            Ok(Some(l)) => {
                                last_msg = l.chars().take(100).collect();
                            }
                            _ => break,
                        }
                    }
                    _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {
                        if self.cancel.load(Ordering::SeqCst) {
                            let _ = child.kill().await;
                            break;
                        }
                    }
                }
            }
        }

        let status = child.wait().await;
        self.cancel.store(false, Ordering::SeqCst);
        let stderr_str = stderr_buf.lock().await.clone();

        match status {
            Ok(s) if s.success() => ProcessInfo {
                name: pack.name.clone(),
                progress: 100,
                state: "installed".into(),
                msg: "安装成功".into(),
            },
            Ok(_) => {
                let msg = if stderr_str.is_empty() {
                    last_msg
                } else {
                    stderr_str.chars().take(200).collect()
                };
                ProcessInfo {
                    name: pack.name.clone(),
                    progress: 0,
                    state: "error".into(),
                    msg,
                }
            }
            Err(e) => ProcessInfo {
                name: pack.name.clone(),
                progress: 0,
                state: "error".into(),
                msg: e.to_string(),
            },
        }
    }

    pub async fn handle_uninstall(&self, package_name: &str) -> bool {
        let python_path = self.inner.lock().await.python_path.clone();
        let _ = Command::new(&python_path)
            .args(["-m", "pip", "uninstall", "-y", package_name])
            .output()
            .await;
        self.delete_lib_dir(package_name).await;
        self.delete_lib_info(package_name).await;
        true
    }

    async fn delete_lib_dir(&self, package_name: &str) {
        let user_lib_path = self.inner.lock().await.user_lib_path.clone();
        let lib_path = user_lib_path.join(package_name);
        if lib_path.exists() {
            let _ = tokio::fs::remove_dir_all(&lib_path).await;
        }
    }

    async fn delete_lib_info(&self, package_name: &str) {
        let user_lib_path = self.inner.lock().await.user_lib_path.clone();
        let entries = match tokio::fs::read_dir(&user_lib_path).await {
            Ok(e) => e,
            Err(_) => return,
        };
        let pattern_name = package_name.replace('-', "_");
        let pattern = format!("^{}(.*)\\.dist-info$", regex::escape(&pattern_name));
        let re = match regex::Regex::new(&pattern) {
            Ok(r) => r,
            Err(_) => return,
        };
        let mut to_remove = Vec::new();
        let mut entries = entries;
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name().to_string_lossy().to_string();
            if re.is_match(&name) {
                to_remove.push(entry.path());
            }
        }
        for p in to_remove {
            let _ = tokio::fs::remove_dir_all(&p).await;
        }
    }

    pub async fn handle_search(&self, name: &str) -> Vec<PackageData> {
        let url = format!("https://pypi.org/search/?q={}", urlencoding(name));
        let html = match crate::utils::retry::retry_async(&crate::utils::retry::RetryPolicy::network(), || async {
            let r = reqwest::get(&url).await;
            match r {
                Ok(resp) if resp.status().is_success() => {
                    let text = resp.text().await.unwrap_or_default();
                    Ok(text)
                }
                Ok(resp) if crate::utils::retry::is_retryable_status(resp.status().as_u16()) => {
                    Err(format!("status {}", resp.status()))
                }
                Ok(resp) => {
                    let text = resp.text().await.unwrap_or_default();
                    Ok(text)
                }
                Err(e) => Err(e.to_string()),
            }
        })
        .await
        {
            Ok(t) => t,
            Err(e) => {
                tracing::error!("搜索错误: {e}");
                return Vec::new();
            }
        };

        let name_version_re = regex::Regex::new(
            r#"<span\s+class="package-snippet__name">([^<]+)</span>[\s\S]*?<span\s+class="package-snippet__version">([^<]+)</span>"#,
        ).unwrap();
        let desc_re = regex::Regex::new(
            r#"<span\s+class="package-snippet__name">([^<]+)</span>[\s\S]*?<p\s+class="package-snippet__description">([^<]*)"#,
        ).unwrap();

        let mut results: Vec<PackageData> = Vec::new();
        for cap in name_version_re.captures_iter(&html) {
            results.push(PackageData {
                name: cap[1].trim().to_string(),
                version: Some(cap[2].trim().to_string()),
                ..Default::default()
            });
        }
        let mut desc_map: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        for cap in desc_re.captures_iter(&html) {
            desc_map.insert(cap[1].trim().to_string(), cap[2].trim().to_string());
        }
        for r in &mut results {
            if let Some(d) = desc_map.get(&r.name) {
                r.desc = Some(d.clone());
            }
        }
        results
    }

    pub fn cancel_install(&self) {
        self.cancel.store(true, Ordering::SeqCst);
        let inner = self.inner.clone();
        tokio::spawn(async move {
            let mut inner = inner.lock().await;
            inner.current_installing = None;
        });
    }

    pub async fn get_process(&self) -> Option<ProcessInfo> {
        let mut inner = self.inner.lock().await;
        if let Some(info) = inner.last_completed_info.take() {
            return Some(info);
        }
        if let Some(name) = inner.current_installing.clone() {
            return Some(ProcessInfo {
                name,
                progress: 50,
                state: "installing".into(),
                msg: "安装中".into(),
            });
        }
        None
    }
}

async fn list_packages(python_path: &str, lib_path: &PathBuf) -> Vec<PipPackage> {
    let output = Command::new(python_path)
        .args([
            "-m",
            "pip",
            "list",
            "--path",
            lib_path.to_string_lossy().as_ref(),
            "--format",
            "json",
        ])
        .output()
        .await;
    match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            if stdout.trim().is_empty() {
                Vec::new()
            } else {
                serde_json::from_str::<Vec<PipPackage>>(&stdout).unwrap_or_default()
            }
        }
        Err(e) => {
            tracing::error!("列出包失败: {e}");
            Vec::new()
        }
    }
}

fn urlencoding(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' || c == '~' {
                c.to_string()
            } else {
                format!("%{:02X}", c as u8)
            }
        })
        .collect()
}
