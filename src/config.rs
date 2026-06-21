use std::path::PathBuf;

pub fn cache_dir() -> PathBuf {
    if let Ok(p) = std::env::var("THONNY_CACHE") {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    let base = dirs::data_local_dir()
        .or_else(|| dirs::home_dir())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    base.join("xes-coding-helper")
}

pub fn cache_path() -> PathBuf {
    cache_dir()
}

pub fn asset_path() -> PathBuf {
    cache_dir().join("asset")
}

pub fn asset_pool_path() -> PathBuf {
    cache_dir().join("asset_pool")
}

pub fn config_path() -> PathBuf {
    cache_dir().join("config.json")
}

pub fn history_file() -> PathBuf {
    cache_dir().join("history.json")
}

pub const PORT_PAIRS: &[(u16, u16)] = &[
    (55820, 55821),
    (55825, 55826),
    (55830, 55831),
    (55835, 55836),
];

pub fn base_port() -> u16 {
    std::env::var("THONNY_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(8000)
}

pub fn file_server_port() -> u16 {
    base_port() + 4
}

pub const MAX_HISTORY_RECORDS: usize = 100;
