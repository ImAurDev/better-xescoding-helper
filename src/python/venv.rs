use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;

use crate::config::cache_dir;
use crate::python::config::find_python_path;
use crate::settings::{Settings, VenvConfig};
use crate::websocket::webtty::{Webtty, WsCmd};

pub fn venv_root_for(project_id: &str) -> PathBuf {
    cache_dir().join("venvs").join(project_id)
}

pub fn venv_python(venv_root: &Path) -> PathBuf {
    if cfg!(windows) {
        venv_root.join("Scripts").join("python.exe")
    } else {
        venv_root.join("bin").join("python")
    }
}

pub fn venv_pip(venv_root: &Path) -> PathBuf {
    if cfg!(windows) {
        venv_root.join("Scripts").join("pip.exe")
    } else {
        venv_root.join("bin").join("pip")
    }
}

pub fn venv_exists(venv_root: &Path) -> bool {
    venv_python(venv_root).exists()
}

pub async fn create_venv(venv_root: &Path, webtty: &Arc<Mutex<Webtty>>) -> bool {
    if venv_exists(venv_root) {
        return true;
    }
    let python = find_python_path().await;
    if let Some(parent) = venv_root.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    let mut cmd = tokio::process::Command::new(&python);
    cmd.args(["-m", "venv", &venv_root.to_string_lossy()])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("创建 venv 失败: {e}");
            let mut wt = webtty.lock().await;
            wt.send_msg(&WsCmd::BackendEvent {
                data: format!(
                    "\x1b[41;37m[venv] 创建失败: {}\x1b[0m\r\n",
                    e
                ),
            })
            .await;
            return false;
        }
    };
    let res = tokio::time::timeout(Duration::from_secs(120), child.wait_with_output()).await;
    match res {
        Ok(Ok(o)) if o.status.success() => {
            let mut wt = webtty.lock().await;
            wt.send_msg(&WsCmd::BackendEvent {
                data: format!(
                    "\x1b[42;30m[venv] 已创建: {}\x1b[0m\r\n",
                    venv_root.display()
                ),
            })
            .await;
            true
        }
        Ok(Ok(o)) => {
            let err = String::from_utf8_lossy(&o.stderr);
            let mut wt = webtty.lock().await;
            wt.send_msg(&WsCmd::BackendEvent {
                data: format!(
                    "\x1b[41;37m[venv] 创建失败: {}\x1b[0m\r\n",
                    err.trim()
                ),
            })
            .await;
            false
        }
        Ok(Err(e)) => {
            tracing::warn!("等待 venv 创建失败: {e}");
            false
        }
        Err(_) => {
            let mut wt = webtty.lock().await;
            wt.send_msg(&WsCmd::BackendEvent {
                data: "\x1b[41;37m[venv] 创建超时\x1b[0m\r\n".to_string(),
            })
            .await;
            false
        }
    }
}

pub async fn install_pinned(venv_root: &Path, config: &VenvConfig, webtty: &Arc<Mutex<Webtty>>) -> bool {
    if config.pinned_packages.is_empty() {
        return true;
    }
    let pip = venv_pip(venv_root);
    if !pip.exists() {
        return false;
    }
    let mut cmd = tokio::process::Command::new(&pip);
    cmd.arg("install")
        .arg("--disable-pip-version-check")
        .arg("--no-warn-script-location")
        .args(&config.pinned_packages)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("pip 启动失败: {e}");
            return false;
        }
    };
    let res = tokio::time::timeout(Duration::from_secs(180), child.wait_with_output()).await;
    let ok = matches!(res, Ok(Ok(o)) if o.status.success());
    if !ok {
        let mut wt = webtty.lock().await;
        wt.send_msg(&WsCmd::BackendEvent {
            data: "\x1b[41;37m[venv] 预装包失败\x1b[0m\r\n".to_string(),
        })
        .await;
    } else {
        let mut wt = webtty.lock().await;
        wt.send_msg(&WsCmd::BackendEvent {
            data: "\x1b[42;30m[venv] 预装包完成\x1b[0m\r\n".to_string(),
        })
        .await;
    }
    ok
}

pub async fn ensure_project_venv(
    project_id: &str,
    settings: &Settings,
    webtty: &Arc<Mutex<Webtty>>,
) -> Option<PathBuf> {
    if !settings.venv.enabled {
        return None;
    }
    let root = venv_root_for(project_id);
    if !venv_exists(&root) {
        if !create_venv(&root, webtty).await {
            return None;
        }
        if settings.venv.inherit_base_packages {
            let _ = install_pinned(&root, &settings.venv, webtty).await;
        }
    }
    Some(root)
}

pub fn venv_env_overrides(venv_root: &Path) -> Vec<(String, String)> {
    let bin_dir = if cfg!(windows) {
        venv_root.join("Scripts")
    } else {
        venv_root.join("bin")
    };
    let path_sep = if cfg!(windows) { ";" } else { ":" };
    let new_path = format!(
        "{}{}{}",
        bin_dir.to_string_lossy(),
        path_sep,
        std::env::var("PATH").unwrap_or_default()
    );
    vec![
        ("VIRTUAL_ENV".to_string(), venv_root.to_string_lossy().to_string()),
        ("PATH".to_string(), new_path),
    ]
}
