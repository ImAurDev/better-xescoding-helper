use std::collections::HashMap;

use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use crate::health;
use crate::python::metrics;
use crate::python::prewarm::{self, PrewarmReport};
use crate::settings::{mutate_arc, persist_arc, ProxyConfig, VenvConfig};
use crate::state::AppState;
use crate::utils::log_buffer;

fn json_error(status: u16, message: &str) -> Response {
    let code = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_REQUEST);
    (code, Json(json!({ "code": status, "message": message }))).into_response()
}

#[derive(Deserialize)]
pub struct ExportParams {
    pub format: Option<String>,
}

pub async fn export_history(
    State(app): State<AppState>,
    axum::extract::Query(p): axum::extract::Query<ExportParams>,
) -> Response {
    let format = p.format.unwrap_or_else(|| "json".into());
    let history = app.history.lock().await;
    match history.export(&format).await {
        Ok(content) => {
            let content_type = match format.as_str() {
                "csv" => "text/csv; charset=utf-8",
                "md" | "markdown" => "text/markdown; charset=utf-8",
                _ => "application/json; charset=utf-8",
            };
            let filename = format!("history.{}", match format.as_str() {
                "md" | "markdown" => "md",
                "csv" => "csv",
                _ => "json",
            });
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, content_type)
                .header(
                    header::CONTENT_DISPOSITION,
                    format!("attachment; filename=\"{}\"", filename),
                )
                .body(axum::body::Body::from(content))
                .unwrap_or_else(|e| {
                    tracing::warn!("history export builder error: {e}");
                    json_error(500, "导出失败")
                })
        }
        Err(e) => json_error(400, &e),
    }
}

pub async fn get_health(State(app): State<AppState>) -> Response {
    let server_error = app.server_error.lock().await.clone();
    let settings = app.settings.lock().await.clone();
    let report =
        health::build_report(app.started_at, &settings, &app.history, &app.webtty, server_error.map(|e| json!(e))).await;
    Json(report).into_response()
}

pub async fn get_metrics(State(_app): State<AppState>) -> Response {
    let summary = metrics::summary().await;
    let recent = metrics::recent(20).await;
    Json(json!({
        "summary": summary,
        "recent": recent,
    }))
    .into_response()
}

pub async fn get_metrics_recent(
    State(_app): State<AppState>,
    axum::extract::Query(p): axum::extract::Query<HashMap<String, String>>,
) -> Response {
    let limit = p
        .get("limit")
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(20);
    let items = metrics::recent(limit).await;
    Json(json!({ "items": items })).into_response()
}

pub async fn get_logs(
    State(_app): State<AppState>,
    axum::extract::Query(p): axum::extract::Query<HashMap<String, String>>,
) -> Response {
    let level = p.get("level").map(|s| s.as_str());
    let limit = p
        .get("limit")
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(50);
    let items = log_buffer::recent_logs(level, limit).await;
    let panics = log_buffer::recent_panics().await;
    Json(json!({ "items": items, "panics": panics })).into_response()
}

pub async fn get_settings(State(app): State<AppState>) -> Response {
    let snapshot = app.settings.lock().await.clone();
    Json(snapshot).into_response()
}

#[derive(Deserialize)]
pub struct ProxyPayload {
    pub http: Option<String>,
    pub https: Option<String>,
    pub no_proxy: Option<String>,
    pub pip_index: Option<String>,
    pub asset_cdn_override: Option<String>,
}

pub async fn set_proxy(State(app): State<AppState>, Json(body): Json<ProxyPayload>) -> Response {
    mutate_arc(&app.settings, |s| {
        s.proxy = ProxyConfig {
            http: body.http.clone(),
            https: body.https.clone(),
            no_proxy: body.no_proxy.clone(),
            pip_index: body.pip_index.clone(),
            asset_cdn_override: body.asset_cdn_override.clone(),
        };
    })
    .await;
    persist_arc(&app.settings).await;
    crate::settings::set_global_proxy(app.settings.lock().await.proxy.clone());
    Json(json!({ "data": "ok" })).into_response()
}

pub async fn get_proxy(State(app): State<AppState>) -> Response {
    let p = app.settings.lock().await.proxy.clone();
    Json(p).into_response()
}

#[derive(Deserialize)]
pub struct EnvPayload {
    pub project_id: String,
    pub vars: HashMap<String, String>,
    pub merge: Option<bool>,
}

pub async fn get_env(
    State(app): State<AppState>,
    axum::extract::Query(p): axum::extract::Query<HashMap<String, String>>,
) -> Response {
    let pid = p.get("project_id").cloned().unwrap_or_default();
    let s = app.settings.lock().await;
    let vars = s.env_vars.get(&pid).cloned().unwrap_or_default();
    Json(json!({ "project_id": pid, "vars": vars })).into_response()
}

pub async fn set_env(State(app): State<AppState>, Json(body): Json<EnvPayload>) -> Response {
    if body.project_id.is_empty() {
        return json_error(400, "缺少 project_id");
    }
    let merge = body.merge.unwrap_or(false);
    mutate_arc(&app.settings, |s| {
        if merge {
            let entry = s
                .env_vars
                .entry(body.project_id.clone())
                .or_insert_with(HashMap::new);
            for (k, v) in body.vars {
                entry.insert(k, v);
            }
        } else {
            s.env_vars.insert(body.project_id.clone(), body.vars.clone());
        }
    })
    .await;
    persist_arc(&app.settings).await;
    Json(json!({ "data": "ok" })).into_response()
}

pub async fn delete_env(
    State(app): State<AppState>,
    axum::extract::Query(p): axum::extract::Query<HashMap<String, String>>,
) -> Response {
    let pid = p.get("project_id").cloned().unwrap_or_default();
    if pid.is_empty() {
        return json_error(400, "缺少 project_id");
    }
    let key = p.get("key").cloned();
    mutate_arc(&app.settings, |s| {
        if let Some(k) = key {
            if let Some(map) = s.env_vars.get_mut(&pid) {
                map.remove(&k);
            }
        } else {
            s.env_vars.remove(&pid);
        }
    })
    .await;
    persist_arc(&app.settings).await;
    Json(json!({ "data": "ok" })).into_response()
}

pub async fn get_cleanup_policy(State(app): State<AppState>) -> Response {
    let policy = app.settings.lock().await.cleanup.clone();
    Json(policy).into_response()
}

#[derive(Deserialize)]
pub struct CleanupPayload {
    pub max_cache_bytes: Option<u64>,
    pub max_asset_pool_bytes: Option<u64>,
    pub max_snapshot_bytes: Option<u64>,
    pub max_snapshot_count: Option<usize>,
    pub run_metrics_history: Option<usize>,
}

pub async fn set_cleanup_policy(
    State(app): State<AppState>,
    Json(body): Json<CleanupPayload>,
) -> Response {
    let new_policy = mutate_arc(&app.settings, |s| {
        let mut p = s.cleanup.clone();
        if let Some(v) = body.max_cache_bytes {
            p.max_cache_bytes = v;
        }
        if let Some(v) = body.max_asset_pool_bytes {
            p.max_asset_pool_bytes = v;
        }
        if let Some(v) = body.max_snapshot_bytes {
            p.max_snapshot_bytes = v;
        }
        if let Some(v) = body.max_snapshot_count {
            p.max_snapshot_count = v;
        }
        if let Some(v) = body.run_metrics_history {
            p.run_metrics_history = v;
        }
        s.cleanup = p.clone();
        p
    })
    .await;
    persist_arc(&app.settings).await;
    metrics::configure_capacity(new_policy.run_metrics_history).await;
    Json(json!({ "data": "ok" })).into_response()
}

pub async fn get_venv_settings(State(app): State<AppState>) -> Response {
    let v = app.settings.lock().await.venv.clone();
    Json(v).into_response()
}

#[derive(Deserialize)]
pub struct VenvPayload {
    pub enabled: Option<bool>,
    pub inherit_base_packages: Option<bool>,
    pub pinned_packages: Option<Vec<String>>,
}

pub async fn set_venv_settings(State(app): State<AppState>, Json(body): Json<VenvPayload>) -> Response {
    mutate_arc(&app.settings, |s| {
        if let Some(v) = body.enabled {
            s.venv.enabled = v;
        }
        if let Some(v) = body.inherit_base_packages {
            s.venv.inherit_base_packages = v;
        }
        if let Some(v) = body.pinned_packages {
            s.venv.pinned_packages = v;
        }
        s.venv = VenvConfig {
            enabled: s.venv.enabled,
            inherit_base_packages: s.venv.inherit_base_packages,
            pinned_packages: s.venv.pinned_packages.clone(),
        };
    })
    .await;
    persist_arc(&app.settings).await;
    Json(json!({ "data": "ok" })).into_response()
}

pub async fn get_prewarm_settings(State(app): State<AppState>) -> Response {
    let p = app.settings.lock().await.prewarm.clone();
    Json(p).into_response()
}

#[derive(Deserialize)]
pub struct PrewarmSettingsPayload {
    pub enabled: Option<bool>,
    pub packages: Option<Vec<String>>,
}

pub async fn set_prewarm_settings(
    State(app): State<AppState>,
    Json(body): Json<PrewarmSettingsPayload>,
) -> Response {
    mutate_arc(&app.settings, |s| {
        if let Some(v) = body.enabled {
            s.prewarm.enabled = v;
        }
        if let Some(v) = body.packages {
            s.prewarm.packages = v;
        }
    })
    .await;
    persist_arc(&app.settings).await;
    Json(json!({ "data": "ok" })).into_response()
}

#[derive(Deserialize)]
pub struct PrewarmRunPayload {
    pub packages: Option<Vec<String>>,
    pub force: Option<bool>,
}

pub async fn run_prewarm(
    State(app): State<AppState>,
    Json(body): Json<PrewarmRunPayload>,
) -> Response {
    let (config, settings, force) = {
        let s = app.settings.lock().await;
        let mut cfg = s.prewarm.clone();
        if let Some(packages) = body.packages.clone() {
            cfg.packages = packages;
        }
        (cfg, s.clone(), body.force.unwrap_or(false))
    };
    if !force && prewarm::is_already_warmed(24) && config.packages.len() <= 1 {
        return Json(json!({
            "data": "skipped",
            "reason": "24 小时内已执行过"
        }))
        .into_response();
    }
    let report: PrewarmReport =
        prewarm::run_prewarm(&config, &settings, Some(app.webtty.clone())).await;
    Json(json!({ "data": report })).into_response()
}

pub async fn get_run_queue(State(app): State<AppState>) -> Response {
    let snap = app.runner_pool.snapshot().await;
    Json(snap).into_response()
}

pub async fn get_registered_projects(State(app): State<AppState>) -> Response {
    let list = app.runner_pool.registered_projects().await;
    Json(json!({ "data": list })).into_response()
}

#[derive(Deserialize)]
pub struct QueueSubmitPayload {
    pub project_id: String,
    pub code: String,
    pub path: Option<String>,
}

pub async fn submit_run(State(app): State<AppState>, Json(body): Json<QueueSubmitPayload>) -> Response {
    if body.project_id.is_empty() {
        return json_error(400, "缺少 project_id");
    }
    if body.code.is_empty() {
        return json_error(400, "缺少 code");
    }
    let path = body.path.unwrap_or_else(|| body.project_id.clone());
    let sender = app.runner_pool.sender();
    if let Err(e) = sender
        .send(crate::python::runner_pool::RunRequest::Run {
            project_id: body.project_id,
            code: body.code,
            path,
        })
        .await
    {
        return json_error(503, &format!("队列已关闭: {e}"));
    }
    Json(json!({ "data": "queued" })).into_response()
}

pub async fn get_run_limits(State(app): State<AppState>) -> Response {
    let limits = app.settings.lock().await.run_limits.clone();
    Json(limits).into_response()
}

#[derive(Deserialize)]
pub struct RunLimitsPayload {
    pub python_timeout_secs: Option<u64>,
    pub go_timeout_secs: Option<u64>,
    pub ts_timeout_secs: Option<u64>,
    pub auto_install_on_missing: Option<bool>,
    pub lint_before_run: Option<bool>,
    pub detect_missing_imports: Option<bool>,
}

pub async fn set_run_limits(
    State(app): State<AppState>,
    Json(body): Json<RunLimitsPayload>,
) -> Response {
    mutate_arc(&app.settings, |s| {
        if let Some(v) = body.python_timeout_secs {
            s.run_limits.python_timeout_secs = v;
        }
        if let Some(v) = body.go_timeout_secs {
            s.run_limits.go_timeout_secs = v;
        }
        if let Some(v) = body.ts_timeout_secs {
            s.run_limits.ts_timeout_secs = v;
        }
        if let Some(v) = body.auto_install_on_missing {
            s.run_limits.auto_install_on_missing = v;
        }
        if let Some(v) = body.lint_before_run {
            s.run_limits.lint_before_run = v;
        }
        if let Some(v) = body.detect_missing_imports {
            s.run_limits.detect_missing_imports = v;
        }
    })
    .await;
    persist_arc(&app.settings).await;
    Json(json!({ "data": "ok" })).into_response()
}

pub async fn get_max_concurrent(State(app): State<AppState>) -> Response {
    let n = app.settings.lock().await.max_concurrent_runners;
    Json(json!({ "max_concurrent": n })).into_response()
}

#[derive(Deserialize)]
pub struct MaxConcurrentPayload {
    pub max_concurrent: usize,
    pub max_queue: Option<usize>,
}

pub async fn set_max_concurrent(
    State(app): State<AppState>,
    Json(body): Json<MaxConcurrentPayload>,
) -> Response {
    if body.max_concurrent == 0 {
        return json_error(400, "max_concurrent 必须大于 0");
    }
    mutate_arc(&app.settings, |s| {
        s.max_concurrent_runners = body.max_concurrent;
    })
    .await;
    persist_arc(&app.settings).await;
    app.runner_pool
        .configure(
            body.max_concurrent,
            body.max_queue.unwrap_or(32),
        )
        .await;
    Json(json!({ "data": "ok" })).into_response()
}

pub async fn trigger_cache_cleanup(State(app): State<AppState>) -> Response {
    let cleanup_settings = app.settings.lock().await.cleanup.clone();
    let result = crate::utils::cache_cleaner::run_cleanup_with_policy(&cleanup_settings).await;
    Json(json!({ "data": result })).into_response()
}

pub async fn list_caches(State(_app): State<AppState>) -> Response {
    let info = crate::utils::cache_cleaner::list_cache_info().await;
    Json(info).into_response()
}
