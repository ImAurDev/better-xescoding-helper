use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use tokio::sync::{oneshot, Mutex};

use crate::python::exec::{forward_stream, GO_RUN_TIMEOUT_SECS};
use crate::python::runner::Runner;
use crate::websocket::webtty::WsCmd;

impl Runner {
    pub(super) async fn run_golang(
        &self,
        go_files: HashMap<String, String>,
        work_dir: &Path,
    ) -> String {
        self.detect_golang().await;
        let golang_path = self.state.lock().await.golang_path.clone();
        for (file_name, content) in &go_files {
            let file_path = work_dir.join(file_name);
            if let Some(parent) = file_path.parent() {
                let _ = tokio::fs::create_dir_all(parent).await;
            }
            let _ = tokio::fs::write(&file_path, content).await;
        }

        let first_file = match go_files.keys().next() {
            Some(k) => k.clone(),
            None => return String::new(),
        };
        let main_pkg_file = go_files
            .iter()
            .find(|(_, content)| content.lines().any(|l| l.trim() == "package main"))
            .map(|(name, _)| name.clone())
            .unwrap_or_else(|| first_file.clone());
        let go_work_dir =
            if main_pkg_file.contains(std::path::MAIN_SEPARATOR) || main_pkg_file.contains('/') {
                work_dir.join(Path::new(&main_pkg_file).parent().unwrap_or(Path::new("")))
            } else {
                work_dir.to_path_buf()
            };
        let main_file =
            if main_pkg_file.contains(std::path::MAIN_SEPARATOR) || main_pkg_file.contains('/') {
                Path::new(&main_pkg_file)
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string()
            } else {
                main_pkg_file.clone()
            };

        let go_mod_path = go_work_dir.join("go.mod");
        if !go_mod_path.exists() {
            let _ = tokio::process::Command::new(&golang_path)
                .args(["mod", "init", "tempmodule"])
                .current_dir(&go_work_dir)
                .output()
                .await;
            let tidy = tokio::process::Command::new(&golang_path)
                .args(["mod", "tidy"])
                .current_dir(&go_work_dir)
                .output()
                .await;
            if let Ok(o) = tidy {
                let tidy_stderr = String::from_utf8_lossy(&o.stderr).to_string();
                let trimmed = tidy_stderr.trim();
                if !trimmed.is_empty() {
                    self.webtty
                        .lock()
                        .await
                        .send_msg(&WsCmd::BackendEvent {
                            data: format!(
                                "\x1b[41;37m[Go] {}\x1b[0m\r\n",
                                trimmed.replace('\n', "\r\n")
                            ),
                        })
                        .await;
                }
            }
        }

        let use_module_run = go_mod_path.exists() || go_files.len() > 1;
        let mut cmd = tokio::process::Command::new(&golang_path);
        cmd.kill_on_drop(true);
        if use_module_run {
            cmd.args(["run", "."]);
        } else {
            cmd.args(["run", &main_file]);
        }
        cmd.current_dir(&go_work_dir);
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());
        let project_id = self
            .state
            .lock()
            .await
            .current_project_id
            .clone()
            .unwrap_or_default();
        self.apply_env_to(&mut cmd, &project_id).await;
        for (k, v) in std::env::vars() {
            cmd.env(k, v);
        }

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                tracing::error!("Go运行失败: {e}");
                self.webtty
                    .lock()
                    .await
                    .send_msg(&WsCmd::BackendEvent {
                        data: format!(
                            "\x1b[41;37m[Go错误] {e} —— 请在管理页面设置 Golang 路径\x1b[0m\r\n"
                        ),
                    })
                    .await;
                return String::new();
            }
        };

        let (kill_tx, kill_rx) = oneshot::channel::<()>();
        self.state.lock().await.golang_kill = Some(kill_tx);

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let stdout_buf: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
        let webtty = self.webtty.clone();
        let stdout_clone = stdout_buf.clone();

        let stdout_task = tokio::spawn(async move {
            if let Some(out) = stdout {
                forward_stream(out, |text| {
                    let stdout_clone = stdout_clone.clone();
                    async move {
                        stdout_clone.lock().await.push_str(&text);
                    }
                })
                .await;
            }
        });
        let stderr_task = tokio::spawn(async move {
            if let Some(out) = stderr {
                forward_stream(out, |text| {
                    let webtty = webtty.clone();
                    async move {
                        let output = text.replace('\n', "\r\n");
                        webtty
                            .lock()
                            .await
                            .send_msg(&WsCmd::BackendEvent {
                                data: format!("\x1b[41;37m[Go] {}\x1b[0m", output),
                            })
                            .await;
                    }
                })
                .await;
            }
        });

        let status = tokio::select! {
            s = child.wait() => s,
            _ = kill_rx => {
                let _ = child.kill().await;
                child.wait().await
            }
            _ = tokio::time::sleep(std::time::Duration::from_secs(GO_RUN_TIMEOUT_SECS)) => {
                let _ = child.kill().await;
                self.webtty.lock().await.send_msg(&WsCmd::BackendEvent {
                    data: format!("\x1b[41;37m[Go] 执行超时(>{}秒),已强制终止\x1b[0m\r\n", GO_RUN_TIMEOUT_SECS),
                }).await;
                child.wait().await
            }
        };
        let _ = stdout_task.await;
        let _ = stderr_task.await;
        self.state.lock().await.golang_kill = None;
        tracing::info!(
            "Go程序退出: {}",
            status.map(|s| s.code().unwrap_or(-1)).unwrap_or(-1)
        );
        let result = stdout_buf.lock().await.clone();
        result
    }
}
