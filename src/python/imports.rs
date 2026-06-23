use std::collections::{BTreeSet, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use once_cell::sync::Lazy;
use regex::Regex;
use tokio::sync::Mutex;

use crate::python::code_blocks::module_install_name;
use crate::websocket::webtty::WsCmd;

static IMPORT_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?m)(?:^|\n)\s*(?:import\s+([\w\.]+)|from\s+([\w\.]+)\s+import\s+[^#\n]+)",
    )
    .unwrap()
});

static FROM_IMPORT_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^\s*import\s+([\w\.]+)(?:\s+as\s+\w+)?\s*(?:#.*)?$").unwrap());

static CONDITIONAL_HINTS: &[&str] = &[
    "try:",
    "except ",
    "if ",
    "TYPE_CHECKING",
    "pytest",
    "unittest",
    "test_",
];

const STDLIB: &[&str] = &[
    "__future__", "_thread", "abc", "aifc", "argparse", "array", "ast", "asynchat", "asyncio",
    "asyncore", "atexit", "audioop", "base64", "bdb", "binascii", "binhex", "bisect", "builtins",
    "bz2", "calendar", "cgi", "cgitb", "chunk", "cmath", "cmd", "code", "codecs", "codeop",
    "collections", "colorsys", "compileall", "concurrent", "configparser", "contextlib", "contextvars",
    "copy", "copyreg", "cProfile", "crypt", "csv", "ctypes", "curses", "dataclasses", "datetime",
    "dbm", "decimal", "difflib", "dis", "distutils", "doctest", "email", "encodings", "enum",
    "errno", "faulthandler", "fcntl", "filecmp", "fileinput", "fnmatch", "formatter", "fpectl",
    "fractions", "ftplib", "functools", "gc", "getopt", "getpass", "gettext", "glob", "grp",
    "gzip", "hashlib", "heapq", "hmac", "html", "http", "idlelib", "imaplib", "imghdr", "imp",
    "importlib", "inspect", "io", "ipaddress", "itertools", "json", "keyword", "lib2to3",
    "linecache", "locale", "logging", "lzma", "mailbox", "mailcap", "marshal", "math", "mimetypes",
    "mmap", "modulefinder", "multiprocessing", "netrc", "nis", "nntplib", "numbers", "operator",
    "optparse", "os", "ossaudiodev", "parser", "pathlib", "pdb", "pickle", "pickletools", "pipes",
    "pkgutil", "platform", "plistlib", "poplib", "posix", "posixpath", "pprint", "profile",
    "pstats", "pty", "pwd", "py_compile", "pyclbr", "pydoc", "queue", "quopri", "random", "re",
    "readline", "reprlib", "resource", "rlcompleter", "runpy", "sched", "secrets", "select",
    "selectors", "shelve", "shlex", "shutil", "signal", "site", "smtpd", "smtplib", "sndhdr",
    "socket", "socketserver", "spwd", "sqlite3", "ssl", "stat", "statistics", "string", "stringprep",
    "struct", "subprocess", "sunau", "symtable", "sys", "sysconfig", "syslog", "tabnanny",
    "tarfile", "telnetlib", "tempfile", "termios", "test", "textwrap", "threading", "time",
    "timeit", "tkinter", "token", "tokenize", "trace", "traceback", "tracemalloc", "tty", "turtle",
    "turtledemo", "types", "typing", "unicodedata", "unittest", "urllib", "uu", "uuid", "venv",
    "warnings", "wave", "weakref", "webbrowser", "winreg", "winsound", "wsgiref", "xdrlib", "xml",
    "xmlrpc", "zipapp", "zipfile", "zipimport", "zlib", "_abc", "_collections_abc", "posixpath",
    "ntpath", "nturl2path", "stat", "grp", "pwd", "resource", "sys", "tty", "pty",
];

fn stdlib_set() -> &'static HashSet<&'static str> {
    static SET: Lazy<HashSet<&'static str>> = Lazy::new(|| STDLIB.iter().copied().collect());
    &SET
}

#[derive(Debug, Clone, Default)]
pub struct ImportReport {
    pub modules: BTreeSet<String>,
    pub install_names: BTreeSet<String>,
    pub missing: Vec<String>,
    pub installed: Vec<String>,
}

impl ImportReport {
    pub fn is_empty(&self) -> bool {
        self.modules.is_empty()
    }
}

pub fn extract_imports(code: &str) -> BTreeSet<String> {
    let mut set: BTreeSet<String> = BTreeSet::new();
    for cap in IMPORT_RE.captures_iter(code) {
        let raw = cap.get(1).or_else(|| cap.get(2));
        if let Some(m) = raw {
            let s = m.as_str();
            let top = s.split('.').next().unwrap_or(s);
            if !top.is_empty() {
                set.insert(top.to_string());
            }
        }
    }
    for cap in FROM_IMPORT_RE.captures_iter(code) {
        let raw = cap.get(1);
        if let Some(m) = raw {
            let s = m.as_str();
            let top = s.split('.').next().unwrap_or(s);
            if !top.is_empty() {
                set.insert(top.to_string());
            }
        }
    }
    set
}

pub fn filter_imports(modules: &BTreeSet<String>) -> BTreeSet<String> {
    modules
        .iter()
        .filter(|m| !stdlib_set().contains(m.as_str()))
        .cloned()
        .collect()
}

pub fn is_conditional_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    CONDITIONAL_HINTS
        .iter()
        .any(|h| trimmed.starts_with(h) || trimmed.contains(h))
}

pub fn build_report(code: &str) -> ImportReport {
    let modules = extract_imports(code);
    let filtered = filter_imports(&modules);
    let install_names: BTreeSet<String> = filtered
        .iter()
        .map(|m| module_install_name(m).to_string())
        .collect();
    ImportReport {
        modules,
        install_names,
        missing: Vec::new(),
        installed: Vec::new(),
    }
}

pub fn join_with_quotes(items: &[String]) -> String {
    items
        .iter()
        .map(|s| format!("\"{}\"", s))
        .collect::<Vec<_>>()
        .join(", ")
}

#[derive(Debug, Clone)]
pub struct MissingCheckResult {
    pub missing: Vec<String>,
    pub installed: Vec<String>,
}

pub async fn classify_against_local(
    report: &mut ImportReport,
    local_packages: &HashSet<String>,
) {
    let mut missing = Vec::new();
    let mut installed = Vec::new();
    for module in &report.install_names {
        let root = module.split(|c: char| c == '[' || c == '<' || c == ' ').next().unwrap_or(module);
        let key = root.replace('-', "_").to_ascii_lowercase();
        if local_packages.contains(&key) || local_packages.contains(root) {
            installed.push(module.clone());
        } else {
            missing.push(module.clone());
        }
    }
    report.missing = missing;
    report.installed = installed;
}

pub async fn auto_install_missing(
    report: &ImportReport,
    python_path: &str,
    webtty: Arc<Mutex<crate::websocket::webtty::Webtty>>,
) -> Vec<String> {
    let mut installed = Vec::new();
    for module in &report.missing {
        {
            let mut wt = webtty.lock().await;
            wt.send_msg(&WsCmd::BackendEvent {
                data: format!(
                    "\x1b[44;37m[依赖] 自动安装缺失模块: {}\x1b[0m\r\n",
                    module
                ),
            })
            .await;
        }
        let ok = run_pip_install(python_path, module).await;
        if ok {
            installed.push(module.clone());
            let mut wt = webtty.lock().await;
            wt.send_msg(&WsCmd::BackendEvent {
                data: format!("\x1b[42;30m[依赖] {} 安装成功\x1b[0m\r\n", module),
            })
            .await;
        } else {
            let mut wt = webtty.lock().await;
            wt.send_msg(&WsCmd::BackendEvent {
                data: format!("\x1b[41;37m[依赖] {} 安装失败\x1b[0m\r\n", module),
            })
            .await;
        }
    }
    installed
}

async fn run_pip_install(python_path: &str, pkg: &str) -> bool {
    let res = tokio::time::timeout(
        Duration::from_secs(120),
        tokio::process::Command::new(python_path)
            .args([
                "-m",
                "pip",
                "install",
                pkg,
                "--no-cache-dir",
                "--no-warn-script-location",
                "--disable-pip-version-check",
            ])
            .output(),
    )
    .await;
    matches!(res, Ok(Ok(o)) if o.status.success())
}

pub fn map_module_to_package(module: &str) -> &str {
    module_install_name(module)
}

pub fn venv_python_path(venv_root: &std::path::Path) -> PathBuf {
    if cfg!(windows) {
        venv_root.join("Scripts").join("python.exe")
    } else {
        venv_root.join("bin").join("python")
    }
}

pub fn venv_exists(venv_root: &std::path::Path) -> bool {
    venv_python_path(venv_root).exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_basic_imports() {
        let code = "import os\nimport sys\nimport numpy as np\nfrom flask import Flask\nfrom PIL import Image\n";
        let mods = extract_imports(code);
        assert!(mods.contains("os"));
        assert!(mods.contains("sys"));
        assert!(mods.contains("numpy"));
        assert!(mods.contains("flask"));
        assert!(mods.contains("PIL"));
    }

    #[test]
    fn filters_stdlib() {
        let code = "import os\nimport sys\nimport numpy\n";
        let mods = extract_imports(code);
        let f = filter_imports(&mods);
        assert!(f.contains("numpy"));
        assert!(!f.contains("os"));
    }
}
