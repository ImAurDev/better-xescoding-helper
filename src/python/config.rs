use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::config::config_path;
use crate::utils::executor::{append_common_paths, find_executable_paths, find_first_executable};

pub static PYTHON_CANDIDATES: &[&str] = if cfg!(windows) {
    &["python", "py", "python3"]
} else {
    &["python3", "python"]
};

pub static GOLANG_CANDIDATES: &[&str] = if cfg!(windows) {
    &["go", "go.exe"]
} else {
    &["go"]
};

pub static BUN_CANDIDATES: &[&str] = if cfg!(windows) {
    &["bun", "bun.exe"]
} else {
    &["bun"]
};

#[derive(Debug, Default, Deserialize, Serialize)]
struct Config {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    python_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    golang_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    bun_path: Option<String>,
}

async fn load_config() -> Config {
    let path = config_path();
    match tokio::fs::read_to_string(&path).await {
        Ok(content) if !content.trim().is_empty() => {
            serde_json::from_str(&content).unwrap_or_default()
        }
        _ => Config::default(),
    }
}

async fn save_config(cfg: &Config) {
    let path = config_path();
    if let Some(parent) = path.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    let data = serde_json::to_string_pretty(cfg).unwrap_or_default();
    let _ = tokio::fs::write(&path, data).await;
}

pub async fn get_saved_python_path() -> Option<String> {
    load_config().await.python_path
}

pub async fn save_python_path(p: &str) {
    let mut cfg = load_config().await;
    cfg.python_path = Some(p.to_string());
    save_config(&cfg).await;
}

pub async fn get_saved_golang_path() -> Option<String> {
    load_config().await.golang_path
}

pub async fn save_golang_path(p: &str) {
    let mut cfg = load_config().await;
    cfg.golang_path = Some(p.to_string());
    save_config(&cfg).await;
}

pub async fn get_saved_bun_path() -> Option<String> {
    load_config().await.bun_path
}

pub async fn save_bun_path(p: &str) {
    let mut cfg = load_config().await;
    cfg.bun_path = Some(p.to_string());
    save_config(&cfg).await;
}

pub async fn find_all_bun_paths() -> Vec<String> {
    find_executable_paths(BUN_CANDIDATES)
}

pub async fn find_all_golang_paths() -> Vec<String> {
    let mut paths = find_executable_paths(GOLANG_CANDIDATES);
    let mut seen: HashSet<String> = paths
        .iter()
        .map(|p| {
            if cfg!(windows) {
                p.to_lowercase()
            } else {
                p.clone()
            }
        })
        .collect();
    let common: &[&str] = if cfg!(windows) {
        &["C:\\Go\\bin\\go.exe", "C:\\Program Files\\Go\\bin\\go.exe"]
    } else {
        &["/usr/local/go/bin/go", "/usr/bin/go", "/usr/local/bin/go"]
    };
    append_common_paths(&mut paths, &mut seen, common);
    paths
}

pub async fn find_all_python_paths() -> Vec<String> {
    let mut paths = find_executable_paths(PYTHON_CANDIDATES);
    let mut seen: HashSet<String> = paths
        .iter()
        .map(|p| {
            if cfg!(windows) {
                p.to_lowercase()
            } else {
                p.clone()
            }
        })
        .collect();
    let common: &[&str] = if cfg!(windows) {
        &[
            "C:\\Python312\\python.exe",
            "C:\\Python311\\python.exe",
            "C:\\Python310\\python.exe",
            "C:\\Python39\\python.exe",
            "C:\\Program Files\\Python312\\python.exe",
            "C:\\Program Files\\Python311\\python.exe",
            "C:\\Program Files\\Python310\\python.exe",
        ]
    } else {
        &[
            "/usr/bin/python3",
            "/usr/local/bin/python3",
            "/opt/python3/bin/python3",
        ]
    };
    append_common_paths(&mut paths, &mut seen, common);
    paths
}

pub async fn find_golang_path() -> String {
    let saved = get_saved_golang_path().await;
    let found = find_first_executable(GOLANG_CANDIDATES, saved.as_deref());
    if !found.is_empty() {
        tracing::info!("找到Golang: {found}");
    }
    if found.is_empty() {
        "go".to_string()
    } else {
        found
    }
}

pub async fn find_bun_path() -> String {
    let saved = get_saved_bun_path().await;
    let found = find_first_executable(BUN_CANDIDATES, saved.as_deref());
    if found.is_empty() {
        "bun".to_string()
    } else {
        found
    }
}

pub async fn find_python_path() -> String {
    let saved = get_saved_python_path().await;
    let found = find_first_executable(PYTHON_CANDIDATES, saved.as_deref());
    if !found.is_empty() {
        tracing::info!("找到Python: {found}");
    }
    if found.is_empty() {
        "python".to_string()
    } else {
        found
    }
}

pub async fn current_python_path() -> String {
    let saved = get_saved_python_path().await;
    let found = find_first_executable(PYTHON_CANDIDATES, saved.as_deref());
    if found.is_empty() {
        "python".to_string()
    } else {
        found
    }
}

pub async fn current_golang_path() -> String {
    let saved = get_saved_golang_path().await;
    let found = find_first_executable(GOLANG_CANDIDATES, saved.as_deref());
    if found.is_empty() {
        "go".to_string()
    } else {
        found
    }
}

pub async fn current_bun_path() -> String {
    let saved = get_saved_bun_path().await;
    let found = find_first_executable(BUN_CANDIDATES, saved.as_deref());
    if found.is_empty() {
        "bun".to_string()
    } else {
        found
    }
}
