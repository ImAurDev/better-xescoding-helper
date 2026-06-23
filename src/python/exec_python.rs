use std::path::Path;
use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::sync::{oneshot, Mutex};

use crate::python::code_blocks::module_install_name;
use crate::python::exec::{forward_stream, is_debug_info, DEFAULT_RUN_TIMEOUT_SECS};
use crate::python::runner::Runner;
use crate::sandbox::{backend, report};
use crate::utils::cache_cleaner;
use crate::websocket::webtty::WsCmd;

impl Runner {
    pub(super) async fn run_python(&self, file_path: &Path, work_dir: &Path) {
        let mut retry_count = 0u32;
        let max_retries = 1;
        let project_id = self
            .state
            .lock()
            .await
            .current_project_id
            .clone()
            .unwrap_or_default();
        let sandbox_cfg = self.settings.lock().await.sandbox.clone();
        let sandbox_active = sandbox_cfg.enabled
            && backend::describe(&sandbox_cfg).effective;
        {
            let mut s = self.state.lock().await;
            s.last_sandboxed = sandbox_active;
        }
        loop {
            self.detect_python().await;
            let python_path = self.state.lock().await.python_path.clone();
            let file_name = file_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();

            let mut cmd = if sandbox_active {
                match report::prepare(
                    &python_path,
                    &sandbox_cfg,
                    Some(work_dir.to_path_buf()),
                    &[file_path.to_path_buf()],
                ) {
                    Ok(c) => c.inner,
                    Err(e) => {
                        tracing::warn!("沙箱准备失败,回退普通执行: {e}");
                        let mut c = tokio::process::Command::new(&python_path);
                        c.kill_on_drop(true);
                        c
                    }
                }
            } else {
                let mut c = tokio::process::Command::new(&python_path);
                c.kill_on_drop(true);
                c
            };
            cmd.args(["-u", &file_name]);
            if !sandbox_active {
                cmd.current_dir(work_dir);
            }
            cmd.stdin(std::process::Stdio::piped());
            cmd.stdout(std::process::Stdio::piped());
            cmd.stderr(std::process::Stdio::piped());
            cmd.env("PYTHONIOENCODING", "utf-8");
            cmd.env("PYTHONUTF8", "1");
            if !sandbox_active {
                self.apply_env_to(&mut cmd, &project_id).await;
                for (k, v) in std::env::vars() {
                    cmd.env(k, v);
                }
            }

            let mut child = match cmd.spawn() {
                Ok(c) => c,
                Err(e) => {
                    tracing::error!("Python启动失败: {e}");
                    self.save_run_history(false).await;
                    let _ = crate::python::metrics::end_run(None, false).await;
                    self.webtty
                        .lock()
                        .await
                        .send_msg(&WsCmd::InnerErr {
                            inner_err: format!("运行错误: {e}"),
                        })
                        .await;
                    let mut s = self.state.lock().await;
                    s.main_is_running = false;
                    s.process_ready = false;
                    return;
                }
            };
            {
                let mut s = self.state.lock().await;
                s.python_pid = child.id();
            }
            if let Some(pid) = child.id() {
                crate::python::metrics::track_pid(pid).await;
            }

            let stdout = child.stdout.take();
            let stderr = child.stderr.take();
            let stdin = child.stdin.take();

            let stdin_arc = Arc::new(Mutex::new(stdin));
            {
                let mut s = self.state.lock().await;
                s.python_stdin = Some(stdin_arc.clone());
                while let Some(inp) = s.pending_inputs.first().cloned() {
                    s.pending_inputs.remove(0);
                    if let Some(sin) = stdin_arc.lock().await.as_mut() {
                        let _ = sin.write_all(format!("{}\n", inp).as_bytes()).await;
                    }
                }
                s.process_ready = true;
            }

            let (kill_tx, kill_rx) = oneshot::channel::<()>();
            self.state.lock().await.python_kill = Some(kill_tx);

            let stderr_buffer: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
            let run_buffer: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));

            let stdout_task = {
                let webtty = self.webtty.clone();
                let run_buffer = run_buffer.clone();
                let out = stdout;
                async move {
                    if let Some(out) = out {
                        forward_stream(out, |text| {
                            let webtty = webtty.clone();
                            let run_buffer = run_buffer.clone();
                            async move {
                                let cap = cache_cleaner::max_output_bytes() as usize;
                                {
                                    let mut buf = run_buffer.lock().await;
                                    if buf.len() < cap {
                                        let remaining = cap - buf.len();
                                        if text.len() > remaining {
                                            buf.push_str(&text[..remaining]);
                                        } else {
                                            buf.push_str(&text);
                                        }
                                    }
                                }
                                let output = text.replace('\n', "\r\n");
                                webtty
                                    .lock()
                                    .await
                                    .send_msg(&WsCmd::BackendEvent { data: output })
                                    .await;
                            }
                        })
                        .await;
                    }
                }
            };

            let stderr_task = {
                let webtty = self.webtty.clone();
                let stderr_buffer = stderr_buffer.clone();
                let run_buffer = run_buffer.clone();
                let out = stderr;
                async move {
                    if let Some(out) = out {
                        forward_stream(out, |text| {
                            let webtty = webtty.clone();
                            let stderr_buffer = stderr_buffer.clone();
                            let run_buffer = run_buffer.clone();
                            async move {
                                let cap = cache_cleaner::max_output_bytes() as usize;
                                {
                                    let mut buf = stderr_buffer.lock().await;
                                    if buf.len() < cap {
                                        let remaining = cap - buf.len();
                                        if text.len() > remaining {
                                            buf.push_str(&text[..remaining]);
                                        } else {
                                            buf.push_str(&text);
                                        }
                                    }
                                }
                                {
                                    let mut buf = run_buffer.lock().await;
                                    if buf.len() < cap {
                                        let remaining = cap - buf.len();
                                        if text.len() > remaining {
                                            buf.push_str(&text[..remaining]);
                                        } else {
                                            buf.push_str(&text);
                                        }
                                    }
                                }
                                let output = text.replace('\n', "\r\n");
                                let should_err = !is_debug_info(&text);
                                let data = if should_err {
                                    format!("\x1b[41;37m[Err] {}\x1b[0m", output)
                                } else {
                                    output
                                };
                                webtty
                                    .lock()
                                    .await
                                    .send_msg(&WsCmd::BackendEvent { data })
                                    .await;
                            }
                        })
                        .await;
                    }
                }
            };

            let (stdout_handle, stderr_handle) =
                (tokio::spawn(stdout_task), tokio::spawn(stderr_task));

            let timeout_secs = DEFAULT_RUN_TIMEOUT_SECS;
            let timeout_at =
                tokio::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
            let status = tokio::select! {
                s = child.wait() => Ok(s),
                _ = kill_rx => {
                    let _ = child.kill().await;
                    Ok(child.wait().await)
                }
                _ = tokio::time::sleep_until(timeout_at) => {
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                    self.webtty.lock().await.send_msg(&WsCmd::BackendEvent {
                        data: format!("\x1b[41;37m[Err] Python 执行超时(>{}秒),已强制终止\x1b[0m\r\n", timeout_secs),
                    }).await;
                    Err(())
                }
            };

            let _ = stdout_handle.await;
            let _ = stderr_handle.await;

            if status.is_err() {
                self.save_run_history(false).await;
                let _ = crate::python::metrics::end_run(None, false).await;
                self.webtty.lock().await.send_msg(&WsCmd::CommandRun).await;
                let mut s = self.state.lock().await;
                s.main_is_running = false;
                s.process_ready = false;
                s.python_pid = None;
                return;
            }

            {
                let mut s = self.state.lock().await;
                s.python_stdin = None;
                s.python_kill = None;
                let buf = run_buffer.lock().await.clone();
                s.run_output_buffer = buf;
            }

            let exit_code = status
                .ok()
                .and_then(|s| s.ok())
                .and_then(|s| s.code())
                .unwrap_or(-1);
            tracing::info!("Python程序退出: {exit_code}");

            if exit_code != 0 && retry_count < max_retries {
                let stderr_text = stderr_buffer.lock().await.clone();
                let module_re =
                    regex::Regex::new(r"ModuleNotFoundError: No module named '([^']+)'").unwrap();
                if let Some(cap) = module_re.captures(&stderr_text) {
                    let module_name = cap[1].to_string();
                    let install_name = module_install_name(&module_name).to_string();
                    self.webtty
                        .lock()
                        .await
                        .send_msg(&WsCmd::BackendEvent {
                            data: format!(
                                "\x1b[41;37m[Err] 正在自动安装缺失模块: {}...\x1b[0m\r\n",
                                install_name
                            ),
                        })
                        .await;

                    let install_ok = self.auto_install_module(&install_name).await;
                    crate::python::metrics::record_auto_install(install_ok).await;
                    {
                        let mut s = self.state.lock().await;
                        s.last_auto_installs = s.last_auto_installs.saturating_add(1);
                    }
                    if install_ok {
                        self.webtty
                            .lock()
                            .await
                            .send_msg(&WsCmd::BackendEvent {
                                data: format!(
                                    "\x1b[41;37m[Err] 模块 {} 安装完成，正在重新运行...\x1b[0m\r\n",
                                    install_name
                                ),
                            })
                            .await;
                        self.state.lock().await.process_ready = false;
                        retry_count += 1;
                        continue;
                    } else {
                        self.webtty
                            .lock()
                            .await
                            .send_msg(&WsCmd::BackendEvent {
                                data: format!(
                                    "\x1b[41;37m[Err] 自动安装 {} 失败\x1b[0m\r\n",
                                    install_name
                                ),
                            })
                            .await;
                    }
                }
            }

            {
                let mut s = self.state.lock().await;
                s.last_exit_code = Some(exit_code);
                s.python_pid = None;
            }
            self.save_run_history(exit_code == 0).await;
            let _ = crate::python::metrics::end_run(Some(exit_code), exit_code == 0).await;
            self.webtty.lock().await.send_msg(&WsCmd::CommandRun).await;
            let mut s = self.state.lock().await;
            s.main_is_running = false;
            s.process_ready = false;
            break;
        }
    }

    pub(super) async fn auto_install_module(&self, install_name: &str) -> bool {
        let python_path = self.state.lock().await.python_path.clone();
        let mut cmd = tokio::process::Command::new(&python_path);
        cmd.args([
            "-m",
            "pip",
            "install",
            install_name,
            "--no-cache-dir",
            "--no-warn-script-location",
            "--only-binary",
            ":all:",
        ]);
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());
        match cmd.spawn() {
            Ok(mut child) => {
                let stdout = child.stdout.take();
                let stderr = child.stderr.take();
                let webtty = self.webtty.clone();
                let out_task = tokio::spawn(async move {
                    if let Some(out) = stdout {
                        let reader = tokio::io::BufReader::new(out);
                        let mut lines = reader.lines();
                        while let Ok(Some(line)) = lines.next_line().await {
                            let l = line.trim();
                            if !l.is_empty() {
                                webtty
                                    .lock()
                                    .await
                                    .send_msg(&WsCmd::BackendEvent {
                                        data: format!("\x1b[41;37m[Err] {}\x1b[0m\r\n", l),
                                    })
                                    .await;
                            }
                        }
                    }
                });
                let webtty2 = self.webtty.clone();
                let err_task = tokio::spawn(async move {
                    if let Some(out) = stderr {
                        let reader = tokio::io::BufReader::new(out);
                        let mut lines = reader.lines();
                        while let Ok(Some(line)) = lines.next_line().await {
                            let l = line.trim();
                            if !l.is_empty() {
                                webtty2
                                    .lock()
                                    .await
                                    .send_msg(&WsCmd::BackendEvent {
                                        data: format!("\x1b[41;37m[Err] {}\x1b[0m\r\n", l),
                                    })
                                    .await;
                            }
                        }
                    }
                });
                let status = child.wait().await;
                let _ = out_task.await;
                let _ = err_task.await;
                status.map(|s| s.success()).unwrap_or(false)
            }
            Err(_) => false,
        }
    }
}
