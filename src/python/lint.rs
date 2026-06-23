use std::time::Duration;

use serde::Serialize;

use crate::python::config::find_python_path;
use crate::websocket::webtty::WsCmd;

#[derive(Debug, Clone, Serialize)]
pub struct LintResult {
    pub tool: String,
    pub available: bool,
    pub issues: Vec<LintIssue>,
    pub raw_output: String,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct LintIssue {
    pub line: usize,
    pub column: Option<usize>,
    pub code: Option<String>,
    pub message: String,
    pub severity: String,
}

pub fn is_lint_enabled() -> bool {
    std::env::var("XES_HELPER_LINT").ok().as_deref() == Some("1")
}

pub async fn run_lint(
    file_path: &std::path::Path,
    webtty: std::sync::Arc<tokio::sync::Mutex<crate::websocket::webtty::Webtty>>,
    strict: bool,
) -> Option<LintResult> {
    let tool = pick_lint_tool().await?;
    if !tool.found {
        if strict {
            let mut wt = webtty.lock().await;
            wt.send_msg(&WsCmd::BackendEvent {
                data: format!(
                    "\x1b[43;30m[检查] 未检测到任何 lint 工具(ruff/flake8/pyflakes),已跳过\x1b[0m\r\n"
                ),
            })
            .await;
        }
        return Some(LintResult {
            tool: tool.name,
            available: false,
            issues: Vec::new(),
            raw_output: String::new(),
            duration_ms: 0,
        });
    }

    let start = std::time::Instant::now();
    let mut cmd = tokio::process::Command::new(&tool.bin);
    cmd.args(tool.args.iter().map(|s| s.as_str()));
    cmd.arg(file_path);
    cmd.env("PYTHONIOENCODING", "utf-8");
    cmd.env("PYTHONUTF8", "1");
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    cmd.kill_on_drop(true);
    let child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("启动 lint 工具失败: {e}");
            return Some(LintResult {
                tool: tool.name,
                available: false,
                issues: Vec::new(),
                raw_output: String::new(),
                duration_ms: 0,
            });
        }
    };
    let output = tokio::time::timeout(Duration::from_secs(20), child.wait_with_output()).await;
    let duration_ms = start.elapsed().as_millis() as u64;
    match output {
        Ok(Ok(out)) => {
            let raw = format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            let issues = parse_issues(&raw, &tool.name);
            Some(LintResult {
                tool: tool.name,
                available: true,
                issues,
                raw_output: raw,
                duration_ms,
            })
        }
        _ => Some(LintResult {
            tool: tool.name,
            available: true,
            issues: Vec::new(),
            raw_output: String::new(),
            duration_ms,
        }),
    }
}

struct LintTool {
    name: String,
    bin: String,
    args: Vec<String>,
    found: bool,
}

async fn pick_lint_tool() -> Option<LintTool> {
    let python = find_python_path().await;
    let candidates: &[(&str, &str, &[&str])] = &[
        ("ruff", "ruff", &["-m", "ruff", "check", "--output-format=concise"]),
        ("flake8", "flake8", &["-m", "flake8", "--format=default"]),
        ("pyflakes", "pyflakes", &["-m", "pyflakes"]),
    ];
    for (name, _alias, args) in candidates {
        let probe = tokio::time::timeout(
            Duration::from_secs(3),
            tokio::process::Command::new(&python)
                .arg("-c")
                .arg(format!("import {}; print(getattr({}, '__version__', '?'))", name, name))
                .output(),
        )
        .await;
        if let Ok(Ok(out)) = probe {
            if out.status.success() {
                return Some(LintTool {
                    name: name.to_string(),
                    bin: python.clone(),
                    args: args.iter().map(|s| s.to_string()).collect(),
                    found: true,
                });
            }
        }
    }
    Some(LintTool {
        name: "none".into(),
        bin: String::new(),
        args: Vec::new(),
        found: false,
    })
}

fn parse_issues(raw: &str, tool: &str) -> Vec<LintIssue> {
    let mut issues = Vec::new();
    match tool {
        "ruff" | "flake8" | "pyflakes" => {
            for line in raw.lines() {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let parts: Vec<&str> = trimmed.splitn(4, ':').collect();
                if parts.len() >= 4 {
                    let line_no = parts[1].trim().parse::<usize>().unwrap_or(0);
                    let col_no = parts[2].trim().parse::<usize>().ok();
                    let rest = parts[3].trim();
                    let (code, msg) = match rest.find(' ') {
                        Some(idx) => (Some(rest[..idx].trim().to_string()), rest[idx + 1..].trim().to_string()),
                        None => (None, rest.to_string()),
                    };
                    issues.push(LintIssue {
                        line: line_no,
                        column: col_no,
                        code,
                        message: msg.to_string(),
                        severity: "warning".to_string(),
                    });
                } else {
                    issues.push(LintIssue {
                        line: 0,
                        column: None,
                        code: None,
                        message: trimmed.to_string(),
                        severity: "warning".to_string(),
                    });
                }
            }
        }
        _ => {}
    }
    issues
}
