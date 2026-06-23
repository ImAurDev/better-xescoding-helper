use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::settings::SandboxConfig;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxReport {
    pub backend: String,
    pub effective: bool,
    pub memory_limit_bytes: u64,
    pub cpu_time_limit_secs: u64,
    pub no_network: bool,
    pub notes: Vec<String>,
}

pub fn describe(cfg: &SandboxConfig) -> SandboxReport {
    let backend = detect_backend(cfg.effective_mode().as_str());
    let effective = cfg.enabled && backend.available;
    SandboxReport {
        backend: backend.name,
        effective,
        memory_limit_bytes: cfg.memory_limit_bytes,
        cpu_time_limit_secs: cfg.cpu_time_limit_secs,
        no_network: cfg.no_network,
        notes: backend.notes,
    }
}

#[derive(Debug, Clone)]
pub struct Backend {
    pub name: String,
    pub available: bool,
    pub notes: Vec<String>,
}

pub fn detect_backend(mode: &str) -> Backend {
    let mut notes = Vec::new();
    if !cfg!(target_os = "linux") && !cfg!(target_os = "macos") && !cfg!(windows) {
        return Backend {
            name: "none".into(),
            available: false,
            notes: vec!["当前平台不支持进程沙箱".into()],
        };
    }
    let mode = mode.to_ascii_lowercase();
    let chosen = match mode.as_str() {
        "auto" | "preferred" => None,
        "bwrap" => Some("bwrap"),
        "unshare" => Some("unshare"),
        "sandbox-exec" => Some("sandbox-exec"),
        "process-group" | "pg" => Some("process-group"),
        "off" | "none" => return Backend {
            name: "off".into(),
            available: false,
            notes: vec!["沙箱已通过配置关闭".into()],
        },
        other => {
            notes.push(format!("未知模式 '{other}',回退为 auto"));
            None
        }
    };

    if cfg!(target_os = "linux") {
        let bwrap = chosen == Some("bwrap") || chosen.is_none();
        let unshare = chosen == Some("unshare");
        if bwrap && which_exists("bwrap") {
            return Backend {
                name: "bwrap".into(),
                available: true,
                notes,
            };
        }
        if bwrap {
            notes.push("未检测到 bwrap (apt install bubblewrap)".into());
        }
        if (unshare || chosen.is_none()) && which_exists("unshare") {
            return Backend {
                name: "unshare".into(),
                available: true,
                notes,
            };
        }
        if chosen.is_none() {
            notes.push("将回退到 process-group (仅隔离进程组,不限制资源)".into());
        }
        return Backend {
            name: "process-group".into(),
            available: chosen.is_none() || chosen == Some("process-group"),
            notes,
        };
    }
    if cfg!(target_os = "macos") {
        if chosen == Some("process-group") {
            return Backend {
                name: "process-group".into(),
                available: true,
                notes,
            };
        }
        if which_exists("sandbox-exec") {
            return Backend {
                name: "sandbox-exec".into(),
                available: true,
                notes,
            };
        }
        notes.push("未检测到 sandbox-exec,使用 process-group 兜底".into());
        return Backend {
            name: "process-group".into(),
            available: true,
            notes,
        };
    }
    if cfg!(windows) {
        notes.push("Windows 使用 Job Object (kill_on_drop + CREATE_NEW_PROCESS_GROUP)".into());
        return Backend {
            name: "process-group".into(),
            available: true,
            notes,
        };
    }
    Backend {
        name: "none".into(),
        available: false,
        notes,
    }
}

fn which_exists(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    which::which(name).is_ok()
}

pub fn is_path_allowed(path: &Path, allowed: &[String]) -> bool {
    if allowed.is_empty() {
        return true;
    }
    let p = path.to_string_lossy();
    allowed.iter().any(|a| p.starts_with(a))
}
