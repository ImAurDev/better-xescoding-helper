use std::collections::{HashMap, HashSet};

use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};
use tokio::process::Command;
use tokio::sync::Mutex;

use crate::history::{GraphEdge, RunGraph, RunPackage};
use crate::python::package_manager::PackageManager;
use crate::settings::Settings;

static IMPORT_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?m)^\s*(?:from\s+([A-Za-z_][\w.]*)|import\s+([A-Za-z_][\w.]*(?:\s*,\s*[A-Za-z_][\w.]*)*))"#)
        .unwrap()
});

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GraphBuildReport {
    pub run_id: String,
    pub imports: Vec<String>,
    pub resolved: usize,
    pub unresolved: Vec<String>,
    pub packages: Vec<RunPackage>,
    pub graph: RunGraph,
    pub duration_ms: i64,
}

pub fn extract_top_level_modules(code: &str) -> Vec<String> {
    let mut set: HashSet<String> = HashSet::new();
    for cap in IMPORT_RE.captures_iter(code) {
        if let Some(m) = cap.get(1) {
            set.insert(top_level(m.as_str()));
        }
        if let Some(m) = cap.get(2) {
            for part in m.as_str().split(',') {
                set.insert(top_level(part.trim()));
            }
        }
    }
    let stdlib = stdlib_modules();
    set.into_iter()
        .filter(|m| !stdlib.contains(m))
        .filter(|m| !m.is_empty())
        .collect()
}

fn top_level(module: &str) -> String {
    module.split('.').next().unwrap_or("").to_string()
}

fn stdlib_modules() -> HashSet<String> {
    [
        "os","sys","re","json","math","time","datetime","pathlib","collections","itertools",
        "functools","typing","io","string","random","statistics","threading","multiprocessing",
        "subprocess","shutil","glob","fnmatch","tempfile","uuid","hashlib","hmac","secrets",
        "logging","unittest","doctest","pdb","traceback","inspect","ast","dis","types","abc",
        "enum","dataclasses","copy","pickle","shelve","sqlite3","csv","configparser","argparse",
        "getopt","signal","socket","ssl","http","urllib","email","html","xml","asyncio","concurrent",
        "contextlib","warnings","weakref","gc","operator","pprint","textwrap","unicodedata",
        "stringprep","locale","gettext","zipfile","tarfile","gzip","bz2","lzma","zlib","base64",
        "binascii","struct","codecs","encodings","cgi","cgitb","wsgiref","pydoc","pyclbr",
        "tabnanny","code","codeop","profile","pstats","timeit","trace","platform","errno",
        "faulthandler","posixpath","ntpath","posix","nt","grp","pwd","resource","syslog",
        "pty","tty","fcntl","termios","readline","rlcompleter","imp","importlib",
        "pkgutil","modulefinder","runpy","py_compile","compileall","zipimport",
        "site","sysconfig","builtins","typing_extensions",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

pub async fn build(
    run_id: &str,
    code: &str,
    pkg_manager: &PackageManager,
    settings: &Settings,
) -> GraphBuildReport {
    let started = crate::history::now_millis();
    let imports = extract_top_level_modules(code);
    let local = pkg_manager.get_local_list().await;
    let installed_names: HashSet<String> = local.0.iter().map(|p| canonicalize(&p.name)).collect();
    let mut resolved: Vec<RunPackage> = Vec::new();
    let mut unresolved: Vec<String> = Vec::new();

    for m in &imports {
        if !installed_names.contains(m) {
            unresolved.push(m.clone());
        }
    }
    let candidates: Vec<String> = imports
        .iter()
        .filter(|m| installed_names.contains(*m))
        .cloned()
        .collect();

    if !candidates.is_empty() {
        let probed = probe_packages(&candidates, settings).await;
        for name in &candidates {
            if let Some(info) = probed.get(name) {
                let mut requires = Vec::new();
                for r in &info.requires {
                    let top = top_level(r);
                    if !top.is_empty() && installed_names.contains(&top) {
                        requires.push(top);
                    }
                }
                let required_by: Vec<String> = probed
                    .iter()
                    .filter(|(_, v)| v.requires.iter().any(|r| top_level(r) == *name))
                    .map(|(k, _)| k.clone())
                    .collect();
                resolved.push(RunPackage {
                    name: name.clone(),
                    version: Some(info.version.clone()),
                    requires,
                    required_by,
                });
            } else {
                resolved.push(RunPackage {
                    name: name.clone(),
                    version: None,
                    requires: Vec::new(),
                    required_by: Vec::new(),
                });
            }
        }
    }

    let mut edges: Vec<GraphEdge> = Vec::new();
    for p in &resolved {
        for r in &p.requires {
            edges.push(GraphEdge {
                from: p.name.clone(),
                to: r.clone(),
            });
        }
    }

    let graph = RunGraph {
        run_id: run_id.to_string(),
        nodes: resolved.clone(),
        edges,
    };

    GraphBuildReport {
        run_id: run_id.to_string(),
        imports,
        resolved: resolved.len(),
        unresolved,
        packages: resolved,
        graph,
        duration_ms: crate::history::now_millis() - started,
    }
}

fn canonicalize(name: &str) -> String {
    name.replace('-', "_").to_ascii_lowercase()
}

#[derive(Default, Debug, Clone)]
struct PackageInfo {
    name: String,
    version: String,
    requires: Vec<String>,
}

async fn probe_packages(
    names: &[String],
    _settings: &Settings,
) -> HashMap<String, PackageInfo> {
    let mut out: HashMap<String, PackageInfo> = HashMap::new();
    for name in names {
        if let Some(info) = probe_one(name).await {
            out.insert(name.clone(), info);
        }
    }
    out
}

static PROBE_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

async fn probe_one(name: &str) -> Option<PackageInfo> {
    let _guard = PROBE_LOCK.lock().await;
    let python = crate::python::current_python_path().await;
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        Command::new(&python)
            .args(["-c", &probe_script(name)])
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim()).ok()?;
    Some(PackageInfo {
        name: parsed
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or(name)
            .to_string(),
        version: parsed
            .get("version")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        requires: parsed
            .get("requires")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|x| x.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default(),
    })
}

fn probe_script(name: &str) -> String {
    format!(
        r#"
import importlib.metadata as md
import json, sys
try:
    dist = md.distribution({name_json})
except Exception as e:
    print(json.dumps({{'error': str(e)}}))
    sys.exit(1)
print(json.dumps({{
    'name': dist.metadata.get('Name') or {name_json},
    'version': dist.version,
    'requires': list(dist.requires or []),
}}))
"#,
        name_json = serde_json::to_string(name).unwrap_or_else(|_| "\"\"".to_string())
    )
}
