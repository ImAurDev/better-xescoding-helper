use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::sync::Arc;

use tokio::sync::Mutex;

use crate::python::exec::{forward_stream, TS_RUN_TIMEOUT_SECS};
use crate::python::runner::Runner;
use crate::websocket::webtty::WsCmd;

impl Runner {
    pub(super) async fn run_typescript(
        &self,
        ts_files: HashMap<String, String>,
        work_dir: &Path,
    ) -> String {
        self.detect_bun().await;
        let bun_path = self.state.lock().await.bun_path.clone();
        for (file_name, content) in &ts_files {
            let file_path = work_dir.join(file_name);
            if let Some(parent) = file_path.parent() {
                let _ = tokio::fs::create_dir_all(parent).await;
            }
            let _ = tokio::fs::write(&file_path, content).await;
        }

        let first_file = match ts_files.keys().next() {
            Some(k) => k.clone(),
            None => return String::new(),
        };
        let ts_work_dir =
            if first_file.contains(std::path::MAIN_SEPARATOR) || first_file.contains('/') {
                work_dir.join(Path::new(&first_file).parent().unwrap_or(Path::new("")))
            } else {
                work_dir.to_path_buf()
            };
        let main_file =
            if first_file.contains(std::path::MAIN_SEPARATOR) || first_file.contains('/') {
                Path::new(&first_file)
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string()
            } else {
                first_file.clone()
            };

        let pkg_json_path = ts_work_dir.join("package.json");
        if !pkg_json_path.exists() {
            let pkg = serde_json::json!({
                "name": "tmp",
                "module": "index.ts",
                "type": "module",
                "private": true,
            });
            let _ = tokio::fs::write(&pkg_json_path, pkg.to_string()).await;
        }

        let all_content: String = ts_files.values().cloned().collect::<Vec<_>>().join("\n");
        let import_re = regex::Regex::new(
            r#"(?:import\s+.*?\s+from\s+['"]([^'"]+)['"]|require\s*\(\s*['"]([^'"]+)['"]\))"#,
        )
        .unwrap();
        let mut external_pkgs: BTreeSet<String> = BTreeSet::new();
        for cap in import_re.captures_iter(&all_content) {
            let pkg_name = cap.get(1).or(cap.get(2)).map(|m| m.as_str().to_string());
            if let Some(pn) = pkg_name {
                if !pn.starts_with('.') && !pn.starts_with("node:") && !pn.starts_with("bun:") {
                    external_pkgs.insert(pn);
                }
            }
        }

        if !external_pkgs.is_empty() {
            let _ = tokio::process::Command::new(&bun_path)
                .arg("add")
                .args(external_pkgs.iter().collect::<Vec<_>>())
                .current_dir(&ts_work_dir)
                .output()
                .await;
        }

        let mut cmd = tokio::process::Command::new(&bun_path);
        cmd.kill_on_drop(true);
        cmd.args(["run", &main_file]);
        cmd.current_dir(&ts_work_dir);
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
                tracing::error!("TypeScript运行失败: {e}");
                self.webtty
                    .lock()
                    .await
                    .send_msg(&WsCmd::BackendEvent {
                        data: format!("\x1b[41;37m[TS错误] {e}\x1b[0m\r\n"),
                    })
                    .await;
                return String::new();
            }
        };

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
                                data: format!("\x1b[41;37m[TS] {}\x1b[0m", output),
                            })
                            .await;
                    }
                })
                .await;
            }
        });

        let status = tokio::select! {
            s = child.wait() => s,
            _ = tokio::time::sleep(std::time::Duration::from_secs(TS_RUN_TIMEOUT_SECS)) => {
                let _ = child.kill().await;
                self.webtty.lock().await.send_msg(&WsCmd::BackendEvent {
                    data: format!("\x1b[41;37m[TS] 执行超时(>{}秒),已强制终止\x1b[0m\r\n", TS_RUN_TIMEOUT_SECS),
                }).await;
                child.wait().await
            }
        };
        let _ = stdout_task.await;
        let _ = stderr_task.await;
        tracing::info!(
            "TypeScript程序退出: {}",
            status.map(|s| s.code().unwrap_or(-1)).unwrap_or(-1)
        );
        let result = stdout_buf.lock().await.clone();
        result
    }
}
