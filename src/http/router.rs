use axum::routing::{get, post};
use axum::Router;

use crate::frontend;
use crate::http::cors::cors_layer;
use crate::http::handlers::*;
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
        .route("/api/history/clear", post(clear_history))
        .route("/api/history/delete", get(delete_history))
        .layer(cors_layer())
        .with_state(app)
}
