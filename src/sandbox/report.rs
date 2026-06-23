use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;

use crate::sandbox::backend::{detect_backend, is_path_allowed, Backend};
use crate::settings::SandboxConfig;

pub struct SandboxCommand {
    pub inner: Command,
    pub backend: Backend,
    pub memory_limit: Option<u64>,
    pub cpu_time_limit: Option<Duration>,
    pub no_network: bool,
}

pub fn prepare(
    program: &str,
    cfg: &SandboxConfig,
    cwd: Option<PathBuf>,
    read_only_paths: &[PathBuf],
) -> Result<SandboxCommand, String> {
    let backend = detect_backend(&cfg.effective_mode());
    let enabled = cfg.enabled && backend.available;
    let mut cmd = match backend.name.as_str() {
        "bwrap" if enabled => bwrap_command(program, cfg, cwd, read_only_paths)?,
        "unshare" if enabled => unshare_command(program, cfg, cwd, read_only_paths)?,
        "sandbox-exec" if enabled => sandbox_exec_command(program, cfg, cwd, read_only_paths)?,
        _ => {
            let mut c = Command::new(program);
            if let Some(dir) = cwd.clone() {
                c.current_dir(dir);
            }
            c.kill_on_drop(true);
            #[cfg(windows)]
            {
                c.creation_flags(0x00000200); // CREATE_NEW_PROCESS_GROUP
            }
            c
        }
    };
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    Ok(SandboxCommand {
        inner: cmd,
        backend,
        memory_limit: if cfg.memory_limit_bytes > 0 {
            Some(cfg.memory_limit_bytes)
        } else {
            None
        },
        cpu_time_limit: if cfg.cpu_time_limit_secs > 0 {
            Some(Duration::from_secs(cfg.cpu_time_limit_secs))
        } else {
            None
        },
        no_network: cfg.no_network,
    })
}

#[cfg(target_os = "linux")]
fn bwrap_command(
    program: &str,
    cfg: &SandboxConfig,
    cwd: Option<PathBuf>,
    read_only_paths: &[PathBuf],
) -> Result<Command, String> {
    let mut cmd = Command::new("bwrap");
    cmd.arg("--unshare-user-try")
        .arg("--unshare-pid")
        .arg("--unshare-IPC")
        .arg("--unshare-uts")
        .arg("--die-with-parent")
        .arg("--new-session")
        .arg("--proc").arg("/proc")
        .arg("--dev").arg("/dev")
        .arg("--tmpfs").arg("/tmp")
        .arg("--bind").arg("/").arg("/")
        .arg("--tmpfs").arg("/run")
        .arg("--ro-bind").arg("/usr").arg("/usr");
    for p in read_only_paths {
        if p.exists() {
            let s = p.to_string_lossy();
            cmd.arg("--ro-bind").arg(s.as_ref()).arg(s.as_ref());
        }
    }
    for w in &cfg.writable_paths {
        if !w.is_empty() {
            cmd.arg("--bind").arg(w).arg(w);
        }
    }
    if let Some(dir) = cwd.clone() {
        let s = dir.to_string_lossy().to_string();
        cmd.arg("--chdir").arg(&s);
        cmd.arg("--bind").arg(&s).arg(&s);
    }
    if cfg.no_network {
        cmd.arg("--unshare-net");
    }
    cmd.arg("--").arg(program);
    Ok(cmd)
}

#[cfg(target_os = "linux")]
fn unshare_command(
    program: &str,
    cfg: &SandboxConfig,
    cwd: Option<PathBuf>,
    _read_only_paths: &[PathBuf],
) -> Result<Command, String> {
    let mut cmd = Command::new("unshare");
    cmd.arg("--user")
        .arg("--map-root-user")
        .arg("--pid")
        .arg("--fork")
        .arg("--mount-proc")
        .arg("--");
    if cfg.no_network {
        cmd.arg("--unshare-net");
    }
    let mut inner = Command::new(program);
    if let Some(dir) = cwd {
        inner.current_dir(dir);
    }
    inner.kill_on_drop(true);
    cmd.arg(program);
    Ok(cmd)
}

#[cfg(target_os = "linux")]
fn sandbox_exec_command(
    _program: &str,
    _cfg: &SandboxConfig,
    _cwd: Option<PathBuf>,
    _read_only_paths: &[PathBuf],
) -> Result<Command, String> {
    Err("sandbox-exec 仅在 macOS 可用".into())
}

#[cfg(target_os = "macos")]
fn bwrap_command(
    program: &str,
    _cfg: &SandboxConfig,
    cwd: Option<PathBuf>,
    _read_only_paths: &[PathBuf],
) -> Result<Command, String> {
    let mut cmd = Command::new(program);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    cmd.kill_on_drop(true);
    Ok(cmd)
}

#[cfg(target_os = "macos")]
fn unshare_command(
    program: &str,
    _cfg: &SandboxConfig,
    cwd: Option<PathBuf>,
    _read_only_paths: &[PathBuf],
) -> Result<Command, String> {
    let mut cmd = Command::new(program);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    cmd.kill_on_drop(true);
    Ok(cmd)
}

#[cfg(target_os = "macos")]
fn sandbox_exec_command(
    program: &str,
    cfg: &SandboxConfig,
    cwd: Option<PathBuf>,
    read_only_paths: &[PathBuf],
) -> Result<Command, String> {
    let profile = build_sandbox_profile(cfg, read_only_paths);
    let mut cmd = Command::new("sandbox-exec");
    cmd.arg("-p").arg(profile).arg(program);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    cmd.kill_on_drop(true);
    Ok(cmd)
}

#[cfg(target_os = "macos")]
fn build_sandbox_profile(cfg: &SandboxConfig, read_only_paths: &[PathBuf]) -> String {
    let mut s = String::from("(version 1)\n(deny default)\n");
    s.push_str("(allow process-fork)\n(allow process-exec)\n(allow sysctl-read)\n");
    s.push_str("(allow file-read* file-write* file-ioctl)\n");
    for ro in read_only_paths {
        s.push_str(&format!(
            "(deny file-write* (subpath \"{}\"))\n",
            ro.to_string_lossy()
        ));
    }
    s.push_str("(allow network*)\n");
    if cfg.no_network {
        s.push_str("(deny network*)\n");
    }
    s.push_str("(allow signal)\n");
    s
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn bwrap_command(
    program: &str,
    _cfg: &SandboxConfig,
    cwd: Option<PathBuf>,
    _read_only_paths: &[PathBuf],
) -> Result<Command, String> {
    let mut cmd = Command::new(program);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    cmd.kill_on_drop(true);
    Ok(cmd)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn unshare_command(
    program: &str,
    _cfg: &SandboxConfig,
    cwd: Option<PathBuf>,
    _read_only_paths: &[PathBuf],
) -> Result<Command, String> {
    let mut cmd = Command::new(program);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    cmd.kill_on_drop(true);
    Ok(cmd)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn sandbox_exec_command(
    program: &str,
    _cfg: &SandboxConfig,
    cwd: Option<PathBuf>,
    _read_only_paths: &[PathBuf],
) -> Result<Command, String> {
    let mut cmd = Command::new(program);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    cmd.kill_on_drop(true);
    Ok(cmd)
}

pub fn check_path(cfg: &SandboxConfig, path: &std::path::Path) -> Result<(), String> {
    if !is_path_allowed(path, &cfg.writable_paths) {
        return Err(format!(
            "沙箱策略禁止访问: {}",
            path.to_string_lossy()
        ));
    }
    Ok(())
}
