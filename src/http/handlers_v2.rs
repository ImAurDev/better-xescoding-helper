use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;
use tokio::sync::Mutex;

use crate::ai::explain::AiService;
use crate::dep_graph;
use crate::history::HistoryStore;
use crate::python::package_manager::PackageManager;
use crate::sandbox::backend;
use crate::settings::{
    mutate_arc, persist_arc, AiConfig, SandboxConfig, Settings, UpdaterConfig,
};
use crate::state::AppState;
use crate::updater::self_update::UpdaterService;

fn json_error(status: u16, message: &str) -> Response {
    let code = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_REQUEST);
    (code, Json(json!({ "code": status, "message": message }))).into_response()
}

pub async fn ai_status(State(app): State<AppState>) -> Response {
    let status = app.ai.status().await;
    Json(status).into_response()
}

#[derive(Deserialize)]
pub struct AiExplainPayload {
    pub run_id: Option<String>,
    pub code: Option<String>,
    pub error: Option<String>,
    #[serde(default)]
    pub force: bool,
}

pub async fn ai_explain(
    State(app): State<AppState>,
    Json(body): Json<AiExplainPayload>,
) -> Response {
    if let Some(rid) = body.run_id.as_deref().filter(|s| !s.is_empty()) {
        match app.ai.explain_run(rid).await {
            Ok(res) => {
                return Json(json!({
                    "data": res,
                    "error": null,
                }))
                .into_response()
            }
            Err(e) => return json_error(400, &e),
        }
    }
    if let (Some(code), Some(error)) = (body.code.as_ref(), body.error.as_ref()) {
        match app.ai.explain_text(code, error).await {
            Ok(res) => {
                return Json(json!({ "data": res })).into_response();
            }
            Err(e) => return json_error(400, &e),
        }
    }
    json_error(400, "需要提供 run_id 或 code+error")
}

pub async fn updater_status(State(app): State<AppState>) -> Response {
    Json(app.updater.status().await).into_response()
}

#[derive(Deserialize, Default)]
pub struct UpdaterCheckPayload {
    #[serde(default)]
    pub force: bool,
}

pub async fn updater_check(
    State(app): State<AppState>,
    body: Option<Json<UpdaterCheckPayload>>,
) -> Response {
    let force = body.map(|b| b.0.force).unwrap_or(false);
    match app.updater.check(force).await {
        Ok(r) => Json(json!({ "data": r })).into_response(),
        Err(e) => json_error(400, &e),
    }
}

pub async fn updater_apply(State(app): State<AppState>) -> Response {
    match app.updater.perform_update().await {
        Ok(report) => Json(json!({ "data": report })).into_response(),
        Err(e) => json_error(400, &e),
    }
}

pub async fn sandbox_status(State(app): State<AppState>) -> Response {
    let cfg = app.settings.lock().await.sandbox.clone();
    let report = backend::describe(&cfg);
    Json(json!({
        "config": cfg,
        "report": report,
    }))
    .into_response()
}

#[derive(Deserialize)]
pub struct GraphQuery {
    pub run_id: Option<String>,
    pub project_id: Option<String>,
}

pub async fn dependency_graph(
    State(app): State<AppState>,
    Query(q): Query<GraphQuery>,
) -> Response {
    if let Some(rid) = q.run_id.clone().filter(|s| !s.is_empty()) {
        let graph = app.history.lock().await.get_run_graph(&rid).await;
        if !graph.nodes.is_empty() || !graph.edges.is_empty() {
            return Json(json!({ "data": graph })).into_response();
        }
        return json_error(404, &format!("未找到运行 {rid} 的依赖图"));
    }
    if let Some(pid) = q.project_id.clone().filter(|s| !s.is_empty()) {
        return Json(json!({
            "data": { "run_id": "", "nodes": [], "edges": [], "project_id": pid },
            "note": "需要先提供 run_id;可在历史记录中点击某次运行后获取。"
        }))
        .into_response();
    }
    json_error(400, "需要 run_id 或 project_id")
}

#[derive(Deserialize)]
pub struct DepBuildPayload {
    pub run_id: String,
    pub code: String,
}

pub async fn dependency_graph_build(
    State(app): State<AppState>,
    Json(body): Json<DepBuildPayload>,
) -> Response {
    let settings = app.settings.lock().await.clone();
    let report = dep_graph::build(&body.run_id, &body.code, &app.pkg_manager, &settings).await;
    Json(json!({ "data": report })).into_response()
}

#[derive(Deserialize)]
pub struct HistoryGetParams {
    pub id: Option<String>,
}

pub async fn get_history_one(
    State(app): State<AppState>,
    Query(p): Query<HistoryGetParams>,
) -> Response {
    let id = match p.id {
        Some(s) if !s.is_empty() => s,
        _ => return json_error(400, "缺少 id"),
    };
    let h = app.history.lock().await;
    match h.get(&id).await {
        Some(rec) => Json(json!({ "data": rec })).into_response(),
        None => json_error(404, &format!("未找到运行 {id}")),
    }
}

pub async fn metrics_dashboard(State(_app): State<AppState>) -> Response {
    let summary = crate::python::metrics::summary().await;
    let recent = crate::python::metrics::recent(50).await;
    let history = _app.history.lock().await.recent(200).await;
    let (ok, fail) = history
        .iter()
        .fold((0u32, 0u32), |(ok, fail), r| {
            if r.success {
                (ok + 1, fail)
            } else {
                (ok, fail + 1)
            }
        });
    let total_history = history.len();
    let success_rate = if total_history == 0 {
        0.0
    } else {
        ok as f64 / total_history as f64
    };
    let top_failing = {
        let mut m: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
        for r in history.iter().filter(|r| !r.success) {
            for im in &r.imports {
                *m.entry(im.clone()).or_insert(0) += 1;
            }
        }
        let mut v: Vec<_> = m.into_iter().collect();
        v.sort_by(|a, b| b.1.cmp(&a.1));
        v.into_iter().take(5).collect::<Vec<_>>()
    };
    let last_24h = {
        let now = crate::history::now_millis();
        let cutoff = now - 24 * 3600 * 1000;
        history.iter().filter(|r| r.timestamp >= cutoff).count()
    };
    Json(json!({
        "summary": summary,
        "recent_runs": recent,
        "history_total": total_history,
        "history_success": ok,
        "history_failed": fail,
        "success_rate": success_rate,
        "last_24h_runs": last_24h,
        "top_failing_imports": top_failing,
        "server_version": env!("CARGO_PKG_VERSION"),
    }))
    .into_response()
}

#[derive(Deserialize)]
pub struct AiSettingsPayload {
    pub enabled: Option<bool>,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub model: Option<String>,
    pub timeout_secs: Option<u64>,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub system_prompt: Option<String>,
    pub auto_explain_on_error: Option<bool>,
    #[serde(default)]
    pub clear_api_key: bool,
}

pub async fn get_ai_settings(State(app): State<AppState>) -> Response {
    let cfg = app.settings.lock().await.ai.clone();
    let mut sanitized = cfg.clone();
    sanitized.api_key = sanitized.api_key.as_ref().map(|_| "***".into());
    Json(sanitized).into_response()
}

pub async fn set_ai_settings(
    State(app): State<AppState>,
    Json(body): Json<AiSettingsPayload>,
) -> Response {
    mutate_arc(&app.settings, |s| {
        apply_ai_payload(&mut s.ai, &body);
    })
    .await;
    persist_arc(&app.settings).await;
    Json(json!({ "data": "ok" })).into_response()
}

fn apply_ai_payload(cfg: &mut AiConfig, p: &AiSettingsPayload) {
    if let Some(v) = p.enabled {
        cfg.enabled = v;
    }
    if let Some(v) = p.base_url.clone() {
        cfg.base_url = Some(v);
    }
    if p.clear_api_key {
        cfg.api_key = None;
    } else if let Some(v) = p.api_key.clone() {
        if !v.is_empty() && v != "***" {
            cfg.api_key = Some(v);
        }
    }
    if let Some(v) = p.model.clone() {
        cfg.model = Some(v);
    }
    if let Some(v) = p.timeout_secs {
        cfg.timeout_secs = v;
    }
    if let Some(v) = p.max_tokens {
        cfg.max_tokens = v;
    }
    if let Some(v) = p.temperature {
        cfg.temperature = v;
    }
    if let Some(v) = p.system_prompt.clone() {
        cfg.system_prompt = Some(v);
    }
    if let Some(v) = p.auto_explain_on_error {
        cfg.auto_explain_on_error = v;
    }
}

#[derive(Deserialize)]
pub struct UpdaterSettingsPayload {
    pub enabled: Option<bool>,
    pub repo: Option<String>,
    pub channel: Option<String>,
    pub auto_check: Option<bool>,
    pub check_interval_hours: Option<u64>,
    pub include_prerelease: Option<bool>,
}

pub async fn get_updater_settings(State(app): State<AppState>) -> Response {
    let cfg = app.settings.lock().await.updater.clone();
    Json(cfg).into_response()
}

pub async fn set_updater_settings(
    State(app): State<AppState>,
    Json(body): Json<UpdaterSettingsPayload>,
) -> Response {
    mutate_arc(&app.settings, |s| {
        apply_updater_payload(&mut s.updater, &body);
    })
    .await;
    persist_arc(&app.settings).await;
    Json(json!({ "data": "ok" })).into_response()
}

fn apply_updater_payload(cfg: &mut UpdaterConfig, p: &UpdaterSettingsPayload) {
    if let Some(v) = p.enabled {
        cfg.enabled = v;
    }
    if let Some(v) = p.repo.clone() {
        cfg.repo = Some(v);
    }
    if let Some(v) = p.channel.clone() {
        cfg.channel = Some(v);
    }
    if let Some(v) = p.auto_check {
        cfg.auto_check = v;
    }
    if let Some(v) = p.check_interval_hours {
        cfg.check_interval_hours = v;
    }
    if let Some(v) = p.include_prerelease {
        cfg.include_prerelease = v;
    }
}

#[derive(Deserialize)]
pub struct SandboxSettingsPayload {
    pub enabled: Option<bool>,
    pub mode: Option<String>,
    pub memory_limit_bytes: Option<u64>,
    pub cpu_time_limit_secs: Option<u64>,
    pub no_network: Option<bool>,
    pub read_only_paths: Option<Vec<String>>,
    pub writable_paths: Option<Vec<String>>,
    pub drop_capabilities: Option<bool>,
}

pub async fn get_sandbox_settings(State(app): State<AppState>) -> Response {
    let cfg = app.settings.lock().await.sandbox.clone();
    Json(cfg).into_response()
}

pub async fn set_sandbox_settings(
    State(app): State<AppState>,
    Json(body): Json<SandboxSettingsPayload>,
) -> Response {
    mutate_arc(&app.settings, |s| {
        apply_sandbox_payload(&mut s.sandbox, &body);
    })
    .await;
    persist_arc(&app.settings).await;
    Json(json!({ "data": "ok" })).into_response()
}

fn apply_sandbox_payload(cfg: &mut SandboxConfig, p: &SandboxSettingsPayload) {
    if let Some(v) = p.enabled {
        cfg.enabled = v;
    }
    if let Some(v) = p.mode.clone() {
        cfg.mode = Some(v);
    }
    if let Some(v) = p.memory_limit_bytes {
        cfg.memory_limit_bytes = v;
    }
    if let Some(v) = p.cpu_time_limit_secs {
        cfg.cpu_time_limit_secs = v;
    }
    if let Some(v) = p.no_network {
        cfg.no_network = v;
    }
    if let Some(v) = p.read_only_paths.clone() {
        cfg.read_only_paths = v;
    }
    if let Some(v) = p.writable_paths.clone() {
        cfg.writable_paths = v;
    }
    if let Some(v) = p.drop_capabilities {
        cfg.drop_capabilities = v;
    }
}

#[allow(dead_code)]
pub type SharedHistory = Arc<Mutex<HistoryStore>>;
#[allow(dead_code)]
pub type SharedPkgManager = Arc<Mutex<PackageManager>>;
#[allow(dead_code)]
pub type SharedSettings = Arc<Mutex<Settings>>;
#[allow(dead_code)]
pub type SharedAi = Arc<AiService>;
#[allow(dead_code)]
pub type SharedUpdater = Arc<UpdaterService>;
