use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::io::AsyncWriteExt;
use tokio::process::ChildStdin;
use tokio::sync::{oneshot, Mutex};

use crate::ai::explain::AiService;
use crate::config::{asset_path, cache_path};
use crate::dep_graph;
use crate::history::{gen_id, now_millis, HistoryStore, RunRecord};
use crate::python::check_dangerous_code;
use crate::python::code_blocks::{parse_blocks, parse_go_output, GoOutput};
use crate::python::config::{find_bun_path, find_golang_path, find_python_path, PYTHON_CANDIDATES};
use crate::python::exec::{run_with_timeout, warmup_runtimes};
use crate::python::imports::{auto_install_missing, build_report as build_import_report, classify_against_local, ImportReport};
use crate::python::package_manager::PackageManager;
use crate::settings::Settings;
use crate::utils::cache_cleaner;
use crate::websocket::webtty::{State, Webtty, WsCmd};
use crate::websocket::DANGER_CONFIRM_TIMEOUT_SECS;

pub struct RunnerState {
    pub(crate) python_path: String,
    pub(crate) golang_path: String,
    pub(crate) bun_path: String,
    pub(crate) python_detected: bool,
    pub(crate) golang_detected: bool,
    pub(crate) bun_detected: bool,
    pub(crate) main_is_running: bool,
    pub(crate) process_ready: bool,
    pub(crate) pending_inputs: Vec<String>,
    pub(crate) run_output_buffer: String,
    pub(crate) run_start_time: i64,
    pub(crate) run_code: String,
    pub(crate) run_has_go_blocks: bool,
    pub(crate) python_stdin: Option<Arc<Mutex<Option<ChildStdin>>>>,
    pub(crate) python_kill: Option<oneshot::Sender<()>>,
    pub(crate) golang_kill: Option<oneshot::Sender<()>>,
    pub(crate) python_pid: Option<u32>,
    pub(crate) active_python: Option<PathBuf>,
    pub(crate) current_project_id: Option<String>,
    pub(crate) last_peak_rss: u64,
    pub(crate) last_lint_issues: u32,
    pub(crate) last_auto_installs: u32,
    pub(crate) last_missing_resolved: u32,
    pub(crate) last_exit_code: Option<i32>,
    pub(crate) last_sandboxed: bool,
}

impl RunnerState {
    pub fn new() -> Self {
        Self {
            python_path: PYTHON_CANDIDATES[0].to_string(),
            golang_path: "go".into(),
            bun_path: "bun".into(),
            python_detected: false,
            golang_detected: false,
            bun_detected: false,
            main_is_running: false,
            process_ready: false,
            pending_inputs: Vec::new(),
            run_output_buffer: String::new(),
            run_start_time: 0,
            run_code: String::new(),
            run_has_go_blocks: false,
            python_stdin: None,
            python_kill: None,
            golang_kill: None,
            python_pid: None,
            active_python: None,
            current_project_id: None,
            last_peak_rss: 0,
            last_lint_issues: 0,
            last_auto_installs: 0,
            last_missing_resolved: 0,
            last_exit_code: None,
            last_sandboxed: false,
        }
    }
}

#[allow(dead_code)]
pub struct Runner {
    pub(crate) webtty: Arc<Mutex<Webtty>>,
    pub(crate) history: Arc<Mutex<HistoryStore>>,
    pkg_manager: PackageManager,
    cache_path: PathBuf,
    pub(crate) state: Arc<Mutex<RunnerState>>,
    last_state: Arc<Mutex<State>>,
    pub(crate) settings: Arc<Mutex<Settings>>,
    pub(crate) ai: Arc<AiService>,
}

impl Runner {
    pub fn new(
        webtty: Arc<Mutex<Webtty>>,
        history: Arc<Mutex<HistoryStore>>,
        pkg_manager: PackageManager,
        settings: Arc<Mutex<Settings>>,
        ai: Arc<AiService>,
    ) -> Self {
        Self {
            webtty,
            history,
            pkg_manager,
            cache_path: cache_path(),
            state: Arc::new(Mutex::new(RunnerState::new())),
            last_state: Arc::new(Mutex::new(State::Wait)),
            settings,
            ai,
        }
    }

    pub async fn detect_python(&self) -> String {
        let s = self.state.lock().await;
        if s.python_detected {
            return s.python_path.clone();
        }
        drop(s);
        let p = find_python_path().await;
        let mut s = self.state.lock().await;
        s.python_path = p.clone();
        s.python_detected = true;
        p
    }

    pub async fn detect_golang(&self) -> String {
        let s = self.state.lock().await;
        if s.golang_detected {
            return s.golang_path.clone();
        }
        drop(s);
        let p = find_golang_path().await;
        let mut s = self.state.lock().await;
        s.golang_path = p.clone();
        s.golang_detected = true;
        p
    }

    pub async fn detect_bun(&self) -> String {
        let s = self.state.lock().await;
        if s.bun_detected {
            return s.bun_path.clone();
        }
        drop(s);
        let p = find_bun_path().await;
        let mut s = self.state.lock().await;
        s.bun_path = p.clone();
        s.bun_detected = true;
        p
    }

    pub fn start(self: &Arc<Self>) {
        let this = self.clone();
        let warmup_state = self.state.clone();
        tokio::spawn(async move {
            warmup_runtimes(warmup_state).await;
            let mut interval = tokio::time::interval(std::time::Duration::from_millis(500));
            interval.tick().await;
            loop {
                interval.tick().await;
                this.check_state().await;
            }
        });
    }

    async fn check_state(self: &Arc<Self>) {
        let wt_state = {
            let mut wt = self.webtty.lock().await;
            wt.get_state()
        };

        if wt_state == State::Ready {
            let main_running = self.state.lock().await.main_is_running;
            if !main_running {
                self.state.lock().await.main_is_running = true;
                self.state.lock().await.process_ready = false;
                let this = self.clone();
                tokio::spawn(async move {
                    this.recv_and_run().await;
                });
            } else {
                let process_ready = self.state.lock().await.process_ready;
                if process_ready {
                    loop {
                        let input = self.webtty.lock().await.fetch_next_input();
                        if let Some(inp) = input {
                            self.send_program_input(&inp).await;
                        } else {
                            break;
                        }
                    }
                }
            }
        } else {
            let mut last = self.last_state.lock().await;
            let was_ready = *last == State::Ready;
            *last = wt_state;
            drop(last);
            if was_ready {
                self.restart_backend().await;
                self.state.lock().await.main_is_running = false;
                self.state.lock().await.process_ready = false;
            }
            self.webtty.lock().await.poxy_ready();
        }
        let mut last = self.last_state.lock().await;
        *last = wt_state;
    }

    async fn send_program_input(&self, data: &str) {
        let stdin = self.state.lock().await.python_stdin.clone();
        if let Some(stdin) = stdin {
            if let Some(s) = stdin.lock().await.as_mut() {
                let _ = s.write_all(format!("{}\n", data).as_bytes()).await;
            }
        } else {
            let mut s = self.state.lock().await;
            if !s.process_ready {
                s.pending_inputs.push(data.to_string());
            }
        }
    }

    async fn restart_backend(&self) {
        let py_kill = self.state.lock().await.python_kill.take();
        let go_kill = self.state.lock().await.golang_kill.take();
        if let Some(k) = py_kill {
            let _ = k.send(());
        }
        if let Some(k) = go_kill {
            let _ = k.send(());
        }
    }

    async fn recv_and_run(&self) {
        let wt_state = self.webtty.lock().await.get_state();
        if wt_state != State::Ready {
            self.state.lock().await.main_is_running = false;
            return;
        }

        let (code, path_id, _first_msg) = self.webtty.lock().await.get_code_and_path();
        let (code, path_id) = match (code, path_id) {
            (Some(c), Some(p)) if !c.is_empty() && !p.is_empty() => (c, p),
            _ => {
                self.state.lock().await.main_is_running = false;
                return;
            }
        };

        {
            let mut s = self.state.lock().await;
            s.run_output_buffer.clear();
            s.run_start_time = now_millis();
            s.run_code = code.clone();
            s.run_has_go_blocks = false;
            s.current_project_id = Some(path_id.clone());
            s.last_peak_rss = 0;
            s.last_lint_issues = 0;
            s.last_auto_installs = 0;
            s.last_missing_resolved = 0;
            s.last_exit_code = None;
            s.last_sandboxed = false;
            s.python_pid = None;
            s.active_python = None;
        }

        let _run_metric_id = crate::python::metrics::begin_run(&path_id, false).await;
        crate::python::metrics::track_pid(0).await;

        if let Some(novel) = super::exec_novel::parse_novel(&code) {
            self.webtty
                .lock()
                .await
                .send_msg(&WsCmd::BackendEvent {
                    data: "\x1b[3J\x1b[H\x1b[2J".into(),
                })
                .await;
            self.run_novel(novel).await;
            let _ = crate::python::metrics::end_run(None, true).await;
            return;
        }

        let run_id = gen_id();
        let _ = cache_cleaner::save_code_snapshot(&code, &run_id).await;

        let danger_hits = check_dangerous_code(&code);
        if !danger_hits.is_empty() {
            let rx = {
                let mut wt = self.webtty.lock().await;
                wt.begin_danger_confirm(&danger_hits, DANGER_CONFIRM_TIMEOUT_SECS)
                    .await
            };
            let allow = if let Some(rx) = rx {
                match tokio::time::timeout(
                    std::time::Duration::from_secs(DANGER_CONFIRM_TIMEOUT_SECS),
                    rx,
                )
                .await
                {
                    Ok(Ok(allow)) => {
                        let mut wt = self.webtty.lock().await;
                        wt.finish_danger_confirm(allow, DANGER_CONFIRM_TIMEOUT_SECS, false)
                            .await;
                        allow
                    }
                    _ => {
                        let mut wt = self.webtty.lock().await;
                        wt.clear_danger_tx();
                        wt.finish_danger_confirm(false, DANGER_CONFIRM_TIMEOUT_SECS, true)
                            .await;
                        false
                    }
                }
            } else {
                false
            };
            if !allow {
                self.webtty
                    .lock()
                    .await
                    .send_msg(&WsCmd::InnerErr {
                        inner_err: "代码包含危险操作,已被取消".into(),
                    })
                    .await;
                self.webtty.lock().await.send_msg(&WsCmd::CommandRun).await;
                self.state.lock().await.main_is_running = false;
                let _ = crate::python::metrics::end_run(None, false).await;
                return;
            }
            self.webtty
                .lock()
                .await
                .send_msg(&WsCmd::BackendEvent {
                    data: "\x1b[3J\x1b[H\x1b[2J".into(),
                })
                .await;
        }

        let project_path = asset_path().join(&path_id);
        let file_path = project_path.join("main.py");

        if !self.create_file(&file_path, code.as_bytes()).await {
            self.webtty
                .lock()
                .await
                .send_msg(&WsCmd::InnerErr {
                    inner_err: "资源创建失败，请刷新后页面".into(),
                })
                .await;
            self.state.lock().await.main_is_running = false;
            let _ = crate::python::metrics::end_run(None, false).await;
            return;
        }

        let _ = self.analyze_imports(&code).await;
        let _ = self.run_lint_check(&file_path).await;
        let _ = self.setup_venv(&path_id).await;

        self.detect_python().await;
        let python_path = self.state.lock().await.python_path.clone();
        let check_stderr = {
            let mut cmd = tokio::process::Command::new(&python_path);
            cmd.kill_on_drop(true);
            cmd.args([
                "-c",
                &format!(
                    "compile(open({}, encoding='utf-8').read(), 'main.py', 'exec')",
                    serde_json::to_string(&file_path.to_string_lossy().to_string()).unwrap()
                ),
            ]);
            cmd.current_dir(&project_path);
            cmd.stdout(std::process::Stdio::piped());
            cmd.stderr(std::process::Stdio::piped());
            cmd.env("PYTHONIOENCODING", "utf-8");
            cmd.env("PYTHONUTF8", "1");
            self.apply_env_to(&mut cmd, &path_id).await;
            for (k, v) in std::env::vars() {
                cmd.env(k, v);
            }
            match cmd.spawn() {
                Ok(child) => {
                    match run_with_timeout(10, "Python 语法检查", child.wait_with_output()).await
                    {
                        Ok(Ok(o)) => (
                            o.status.code().unwrap_or(-1),
                            String::from_utf8_lossy(&o.stderr).to_string(),
                        ),
                        Ok(Err(_)) => (-1, String::new()),
                        Err(_) => (-1, "Python 语法检查超时".into()),
                    }
                }
                Err(_) => (-1, String::new()),
            }
        };

        let (exit_code, stderr) = check_stderr;
        if exit_code != 0 {
            let lines: Vec<&str> = stderr.lines().collect();
            let start_idx = lines.iter().position(|l| l.contains("File \"main.py\""));
            let filtered = match start_idx {
                Some(i) => lines[i..].join("\n"),
                None => stderr.clone(),
            };
            {
                let mut s = self.state.lock().await;
                s.run_output_buffer = filtered.clone();
                s.last_exit_code = Some(exit_code);
            }
            self.save_run_history(false).await;
            let _ = crate::python::metrics::end_run(Some(exit_code), false).await;
            let data = format!("\x1b[41;37m[Err] {}\x1b[0m", filtered.replace('\n', "\r\n"));
            self.webtty
                .lock()
                .await
                .send_msg(&WsCmd::BackendEvent { data })
                .await;
            self.webtty.lock().await.send_msg(&WsCmd::CommandRun).await;
            self.state.lock().await.main_is_running = false;
            return;
        }

        let go_blocks = parse_blocks(&code, "go");
        let ts_blocks = parse_blocks(&code, "ts");

        let mut replacements: Vec<(usize, usize, Vec<String>)> = Vec::new();
        let mut has_any_go = false;
        let mut has_any_ts = false;

        if !go_blocks.is_empty() {
            self.state.lock().await.run_has_go_blocks = true;
            tracing::info!("检测到Go代码块: {}", go_blocks.len());
            has_any_go = true;
            let mut go_files: HashMap<String, String> = HashMap::new();
            for b in &go_blocks {
                go_files.insert(b.file_name.clone(), b.content.clone());
            }
            let go_stdout = self.run_golang(go_files, &project_path).await;
            let go_out = if go_stdout.is_empty() {
                GoOutput {
                    print_output: String::new(),
                    return_value: String::new(),
                    return_is_json: false,
                }
            } else {
                parse_go_output(&go_stdout)
            };
            let mut go_replace_lines: Vec<String> = Vec::new();
            if !go_out.print_output.is_empty() {
                go_replace_lines.push(format!(
                    "print({})",
                    serde_json::to_string(&go_out.print_output).unwrap()
                ));
            }
            if !go_out.return_value.is_empty() {
                if go_out.return_is_json {
                    go_replace_lines.push(format!("__GO_OUTPUT__ = {}", go_out.return_value));
                } else {
                    go_replace_lines.push(format!(
                        "__GO_OUTPUT__ = {}",
                        serde_json::to_string(&go_out.return_value).unwrap()
                    ));
                }
            }
            for b in &go_blocks {
                replacements.push((b.start_line, b.end_line, go_replace_lines.clone()));
            }
        }

        if !ts_blocks.is_empty() {
            has_any_ts = true;
            tracing::info!("检测到TypeScript代码块: {}", ts_blocks.len());
            let mut ts_files: HashMap<String, String> = HashMap::new();
            for b in &ts_blocks {
                ts_files.insert(b.file_name.clone(), b.content.clone());
            }
            let ts_stdout = self.run_typescript(ts_files, &project_path).await;
            let ts_out = if ts_stdout.is_empty() {
                GoOutput {
                    print_output: String::new(),
                    return_value: String::new(),
                    return_is_json: false,
                }
            } else {
                parse_go_output(&ts_stdout)
            };
            let mut ts_replace_lines: Vec<String> = Vec::new();
            if !ts_out.print_output.is_empty() {
                ts_replace_lines.push(format!(
                    "print({})",
                    serde_json::to_string(&ts_out.print_output).unwrap()
                ));
            }
            if !ts_out.return_value.is_empty() {
                if ts_out.return_is_json {
                    ts_replace_lines.push(format!("__TS_OUTPUT__ = {}", ts_out.return_value));
                } else {
                    ts_replace_lines.push(format!(
                        "__TS_OUTPUT__ = {}",
                        serde_json::to_string(&ts_out.return_value).unwrap()
                    ));
                }
            }
            for b in &ts_blocks {
                replacements.push((b.start_line, b.end_line, ts_replace_lines.clone()));
            }
        }

        if !replacements.is_empty() {
            replacements.sort_by_key(|r| r.0);
            let lines: Vec<&str> = code.split('\n').collect();
            let mut result_lines: Vec<String> = Vec::new();
            let mut cursor = 0usize;
            for (start, end, rep_lines) in &replacements {
                while cursor < *start {
                    if let Some(l) = lines.get(cursor) {
                        result_lines.push(l.to_string());
                    }
                    cursor += 1;
                }
                cursor = *end + 1;
                for l in rep_lines {
                    result_lines.push(l.clone());
                }
            }
            while cursor < lines.len() {
                if let Some(l) = lines.get(cursor) {
                    result_lines.push(l.to_string());
                }
                cursor += 1;
            }

            if has_any_go {
                let has_go_return = result_lines
                    .iter()
                    .any(|l| l.starts_with("__GO_OUTPUT__ ="));
                if !has_go_return {
                    result_lines.insert(0, "__GO_OUTPUT__ = \"\"".to_string());
                }
            }
            if has_any_ts {
                let has_ts_return = result_lines
                    .iter()
                    .any(|l| l.starts_with("__TS_OUTPUT__ ="));
                if !has_ts_return {
                    result_lines.insert(0, "__TS_OUTPUT__ = \"\"".to_string());
                }
            }

            let joined = result_lines.join("\n");
            self.create_file(&file_path, joined.as_bytes()).await;
        }

        self.run_python(&file_path, &project_path).await;
    }

    async fn create_file(&self, file_name: &Path, content: &[u8]) -> bool {
        if let Some(parent) = file_name.parent() {
            if !parent.exists() {
                if let Err(e) = tokio::fs::create_dir_all(parent).await {
                    tracing::error!("创建文件错误: {e}");
                    return false;
                }
            }
        }
        match tokio::fs::write(file_name, content).await {
            Ok(_) => true,
            Err(e) => {
                tracing::error!("创建文件错误: {e}");
                false
            }
        }
    }

    pub(crate) async fn save_run_history(&self, success: bool) {
        let (record, code, project_id) = {
            let s = self.state.lock().await;
            let record = RunRecord {
                id: gen_id(),
                timestamp: s.run_start_time,
                code: s.run_code.clone(),
                output: s.run_output_buffer.trim_end().to_string(),
                has_go_blocks: s.run_has_go_blocks,
                success,
                duration: now_millis() - s.run_start_time,
                project_id: s.current_project_id.clone(),
                peak_rss_bytes: s.last_peak_rss,
                auto_installs: s.last_auto_installs,
                lint_issues: s.last_lint_issues,
                missing_imports_resolved: s.last_missing_resolved,
                exit_code: s.last_exit_code,
                env_overrides: None,
                imports: Vec::new(),
                packages: Vec::new(),
                ai_explanation: None,
                sandboxed: s.last_sandboxed,
            };
            (record, s.run_code.clone(), s.current_project_id.clone())
        };
        let imports = dep_graph::extract_top_level_modules(&code);
        let run_id_for_graph = record.id.clone();
        let code_for_graph = code.clone();
        let pkg_manager = self.pkg_manager.clone();
        let settings = self.settings.clone();
        let history_for_graph = self.history.clone();
        let _ = project_id;
        let mut h = self.history.lock().await;
        h.add_with_imports(record.clone(), imports.clone(), Vec::new())
            .await;
        drop(h);
        if !imports.is_empty() {
            tokio::spawn(async move {
                let snap = settings.lock().await.clone();
                let report = dep_graph::build(
                    &run_id_for_graph,
                    &code_for_graph,
                    &pkg_manager,
                    &snap,
                )
                .await;
                let mut h = history_for_graph.lock().await;
                h.add_with_imports(
                    RunRecord {
                        id: run_id_for_graph.clone(),
                        timestamp: now_millis(),
                        code: String::new(),
                        output: String::new(),
                        has_go_blocks: false,
                        success: true,
                        duration: 0,
                        project_id: None,
                        peak_rss_bytes: 0,
                        auto_installs: 0,
                        lint_issues: 0,
                        missing_imports_resolved: 0,
                        exit_code: None,
                        env_overrides: None,
                        imports: report.imports.clone(),
                        packages: report.packages.clone(),
                        ai_explanation: None,
                        sandboxed: false,
                    },
                    report.imports,
                    report.packages,
                )
                .await;
            });
        }
        if !success {
            let ai_enabled = self.settings.lock().await.ai.auto_explain_on_error;
            if ai_enabled {
                let ai = self.ai.clone();
                let history = self.history.clone();
                let rid = record.id.clone();
                let code_snapshot = code.clone();
                let output_snapshot = record.output.clone();
                tokio::spawn(async move {
                    let res = ai
                        .explain_text(&code_snapshot, &output_snapshot)
                        .await
                        .ok()
                        .map(|r| r.explanation);
                    if let Some(text) = res {
                        let mut h = history.lock().await;
                        h.attach_ai_explanation(&rid, &text).await;
                    }
                });
            }
        }
    }

    pub(crate) async fn analyze_imports(&self, code: &str) -> Option<ImportReport> {
        let settings = self.settings.lock().await;
        if !settings.run_limits.detect_missing_imports {
            return None;
        }
        drop(settings);
        let mut report = build_import_report(code);
        if report.install_names.is_empty() {
            return Some(report);
        }
        let local = self.pkg_manager.get_local_list().await;
        let set: std::collections::HashSet<String> = local
            .0
            .iter()
            .map(|p| p.name.replace('-', "_").to_ascii_lowercase())
            .collect();
        let set_normal: std::collections::HashSet<String> =
            local.0.iter().map(|p| p.name.clone()).collect();
        let mut combined = set.clone();
        combined.extend(set_normal);
        classify_against_local(&mut report, &combined).await;
        if !report.missing.is_empty() {
            let _ = auto_install_missing(
                &report,
                &self.state.lock().await.python_path,
                self.webtty.clone(),
            )
            .await;
            crate::python::metrics::record_auto_install(true).await;
            crate::python::metrics::record_missing_imports_resolved(report.missing.len() as u32)
                .await;
            {
                let mut s = self.state.lock().await;
                s.last_auto_installs = s.last_auto_installs.saturating_add(report.missing.len() as u32);
                s.last_missing_resolved = s.last_missing_resolved.saturating_add(report.missing.len() as u32);
            }
        }
        Some(report)
    }

    pub(crate) async fn run_lint_check(
        &self,
        file_path: &Path,
    ) -> Option<crate::python::lint::LintResult> {
        let settings = self.settings.lock().await;
        if !settings.run_limits.lint_before_run {
            return None;
        }
        drop(settings);
        let result = crate::python::lint::run_lint(file_path, self.webtty.clone(), false).await;
        if let Some(ref r) = result {
            if r.available {
                {
                    let mut s = self.state.lock().await;
                    s.last_lint_issues = r.issues.len() as u32;
                }
                crate::python::metrics::record_lint(r.issues.len() as u32).await;
                if !r.issues.is_empty() {
                    let preview: String = r
                        .issues
                        .iter()
                        .take(5)
                        .map(|i| format!("L{}: {}", i.line, i.message))
                        .collect::<Vec<_>>()
                        .join(" | ");
                    let mut wt = self.webtty.lock().await;
                    wt.send_msg(&WsCmd::BackendEvent {
                        data: format!(
                            "\x1b[43;30m[lint-{}] {} 项(展示前 5 条): {}\x1b[0m\r\n",
                            r.tool,
                            r.issues.len(),
                            preview
                        ),
                    })
                    .await;
                }
            }
        }
        result
    }

    pub(crate) async fn setup_venv(&self, project_id: &str) -> Option<PathBuf> {
        let settings_snapshot = self.settings.lock().await.clone();
        if !settings_snapshot.venv.enabled {
            return None;
        }
        let venv_root =
            crate::python::venv::ensure_project_venv(project_id, &settings_snapshot, &self.webtty)
                .await?;
        let venv_python = crate::python::venv::venv_python(&venv_root);
        if venv_python.exists() {
            {
                let mut s = self.state.lock().await;
                s.python_path = venv_python.to_string_lossy().to_string();
                s.active_python = Some(venv_python);
            }
        }
        Some(venv_root)
    }

    pub(crate) async fn env_overrides_for(
        &self,
        project_id: &str,
    ) -> HashMap<String, String> {
        let settings = self.settings.lock().await;
        let mut overrides = HashMap::new();
        if let Some(map) = settings.env_vars.get(project_id) {
            for (k, v) in map {
                overrides.insert(k.clone(), v.clone());
            }
        }
        if let Some(venv) = self.state.lock().await.active_python.as_ref() {
            if let Some(parent) = venv.parent().and_then(|p| p.parent()) {
                overrides.insert(
                    "VIRTUAL_ENV".to_string(),
                    parent.to_string_lossy().to_string(),
                );
            }
        }
        if let Some(proxy) = settings.proxy.https.as_ref().or(settings.proxy.http.as_ref()) {
            overrides.insert("HTTPS_PROXY".to_string(), proxy.clone());
            overrides.insert("HTTP_PROXY".to_string(), proxy.clone());
        }
        overrides
    }

    pub(crate) async fn apply_env_to(&self, cmd: &mut tokio::process::Command, project_id: &str) {
        let overrides = self.env_overrides_for(project_id).await;
        for (k, v) in overrides {
            cmd.env(k, v);
        }
    }
}
