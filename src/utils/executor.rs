use std::collections::HashSet;
use std::path::Path;

pub fn find_executable_paths(candidates: &[&str]) -> Vec<String> {
    let mut paths = Vec::new();
    let mut seen = HashSet::new();
    let normalize: fn(&str) -> String = if cfg!(windows) {
        |p| p.to_lowercase()
    } else {
        |p| p.to_string()
    };
    for &cand in candidates {
        if let Ok(iter) = which::which_all(cand) {
            for p in iter {
                let s = p.to_string_lossy().to_string();
                let key = normalize(&s);
                if seen.insert(key) {
                    paths.push(s);
                }
            }
        }
    }
    paths
}

pub fn find_first_executable(candidates: &[&str], saved: Option<&str>) -> String {
    if let Some(s) = saved {
        if !s.is_empty() && Path::new(s).exists() {
            return s.to_string();
        }
    }
    for &cand in candidates {
        if let Ok(p) = which::which(cand) {
            return p.to_string_lossy().to_string();
        }
    }
    candidates.first().copied().unwrap_or("").to_string()
}

pub fn append_common_paths(paths: &mut Vec<String>, seen: &mut HashSet<String>, common: &[&str]) {
    let normalize: fn(&str) -> String = if cfg!(windows) {
        |p| p.to_lowercase()
    } else {
        |p| p.to_string()
    };
    for &p in common {
        let key = normalize(p);
        if Path::new(p).exists() && seen.insert(key) {
            paths.push(p.to_string());
        }
    }
}
