use axum::routing::{get, post};
use axum::Router;

use crate::frontend;
use crate::http::cors::cors_layer;
use crate::http::handlers::*;
use crate::http::handlers_extra::*;
use crate::http::handlers_v2::*;
use crate::state::AppState;

pub fn build(app: AppState) -> Router {
    Router::new()
        .route("/", get(frontend::index))
        .route(
            "/frontend.js",
            get(|| async { frontend::serve("frontend.js").await }),
        )
        .route(
            "/styles.gen.css",
            get(|| async { frontend::serve("styles.gen.css").await }),
        )
        .route("/ping", get(ping))
        .route("/version", get(version))
        .route("/path", post(path))
        .route("/package/search", get(search_pkg))
        .route("/package/local", get(get_list))
        .route("/package/err", get(get_err_list))
        .route("/package/err/delete", post(remove_pkg))
        .route("/package/install", post(install_pkg))
        .route("/package/uninstall", post(uninstall_pkg))
        .route("/package/cancel", post(cancel_install_pkg))
        .route("/package/state", get(get_state))
        .route("/package/all_state", get(get_all_state))
        .route("/package/unlock", post(unlock))
        .route("/package/mirrors", get(get_mirrors))
        .route("/package/mirrors/choose", post(choose_mirror))
        .route("/api/python-paths", get(get_python_paths))
        .route("/api/python-path", post(set_python_path))
        .route("/api/golang-paths", get(get_golang_paths))
        .route("/api/golang-path", post(set_golang_path))
        .route("/api/bun-paths", get(get_bun_paths))
        .route("/api/bun-path", post(set_bun_path))
        .route("/api/status", get(get_status))
        .route("/api/history", get(get_history))
        .route("/api/history/get", get(get_history_one))
        .route("/api/history/clear", post(clear_history))
        .route("/api/history/delete", get(delete_history))
        .route("/api/health", get(get_health))
        .route("/api/metrics", get(get_metrics))
        .route("/api/metrics/runs", get(get_metrics_recent))
        .route("/api/metrics/dashboard", get(metrics_dashboard))
        .route("/api/logs", get(get_logs))
        .route("/api/settings", get(get_settings))
        .route("/api/settings/proxy", get(get_proxy))
        .route("/api/settings/proxy", post(set_proxy))
        .route("/api/settings/env", get(get_env))
        .route("/api/settings/env", post(set_env))
        .route("/api/settings/env/delete", get(delete_env))
        .route("/api/settings/cleanup", get(get_cleanup_policy))
        .route("/api/settings/cleanup", post(set_cleanup_policy))
        .route("/api/settings/venv", get(get_venv_settings))
        .route("/api/settings/venv", post(set_venv_settings))
        .route("/api/settings/prewarm", get(get_prewarm_settings))
        .route("/api/settings/prewarm", post(set_prewarm_settings))
        .route("/api/settings/run-limits", get(get_run_limits))
        .route("/api/settings/run-limits", post(set_run_limits))
        .route("/api/settings/max-concurrent", get(get_max_concurrent))
        .route("/api/settings/max-concurrent", post(set_max_concurrent))
        .route("/api/settings/ai", get(get_ai_settings))
        .route("/api/settings/ai", post(set_ai_settings))
        .route("/api/settings/updater", get(get_updater_settings))
        .route("/api/settings/updater", post(set_updater_settings))
        .route("/api/settings/sandbox", get(get_sandbox_settings))
        .route("/api/settings/sandbox", post(set_sandbox_settings))
        .route("/api/prewarm", post(run_prewarm))
        .route("/api/queue", get(get_run_queue))
        .route("/api/queue/submit", post(submit_run))
        .route("/api/projects", get(get_registered_projects))
        .route("/api/cache/list", get(list_caches))
        .route("/api/cache/cleanup", post(trigger_cache_cleanup))
        .route("/api/history/export", get(export_history))
        .route("/api/ai/status", get(ai_status))
        .route("/api/ai/explain", post(ai_explain))
        .route("/api/updater/status", get(updater_status))
        .route("/api/updater/check", post(updater_check))
        .route("/api/updater/apply", post(updater_apply))
        .route("/api/sandbox/status", get(sandbox_status))
        .route("/api/dependency-graph", get(dependency_graph))
        .route("/api/dependency-graph/build", post(dependency_graph_build))
        .layer(cors_layer())
        .with_state(app)
}
