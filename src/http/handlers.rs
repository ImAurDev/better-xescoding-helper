use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::config::file_server_port;
use crate::download::assets::{get_local_path, AssetJson, AssetManage};
use crate::python::{
    current_bun_path, current_golang_path, current_python_path, find_all_bun_paths,
    find_all_golang_paths, find_all_python_paths, get_saved_bun_path, get_saved_golang_path,
    get_saved_python_path, save_bun_path, save_golang_path, save_python_path,
};
use crate::state::AppState;

fn json_error(status: u16, message: &str) -> Response {
    let code = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_REQUEST);
    (code, Json(json!({ "code": status, "message": message }))).into_response()
}

fn locked_response() -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({ "status_code": 1001, "message": "该服务被锁定" })),
    )
        .into_response()
}

fn is_locked(v: &Value) -> bool {
    matches!(v, Value::Bool(false))
}

fn value_to_id(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => v.to_string(),
    }
}

pub async fn ping() -> Response {
    Json(json!({"auto": true})).into_response()
}

pub async fn version() -> Response {
    Json(json!({"version": "2.13"})).into_response()
}

#[derive(Deserialize)]
pub struct PathPayload {
    pub id: Option<String>,
    pub message: Option<AssetJson>,
    pub project_id: Option<Value>,
}

pub async fn path(State(_app): State<AppState>, Json(payload): Json<PathPayload>) -> Response {
    let id = payload.id.unwrap_or_default();
    if id.is_empty() {
        return json_error(404, "资源不存在");
    }
    let message = match payload.message {
        Some(m) => m,
        None => return json_error(400, "缺少资源信息"),
    };
    let mut am = AssetManage::new();
    let result = am.handle_assets_json(message).await;
    if !result.ok {
        return json_error(400, "资源处理失败");
    }
    let pid = payload
        .project_id
        .as_ref()
        .map(value_to_id)
        .unwrap_or_else(|| "6".to_string());
    match get_local_path(&pid, &id).await {
        Some(local) => Json(json!({
            "code": 0,
            "data": { "path": format!("http://127.0.0.1:{}/{}", file_server_port(), local) }
        }))
        .into_response(),
        None => json_error(404, "资源不存在"),
    }
}

#[derive(Deserialize)]
pub struct SearchParams {
    pub name: Option<String>,
    pub exact_flag: Option<String>,
}

pub async fn search_pkg(State(app): State<AppState>, Query(p): Query<SearchParams>) -> Response {
    let name = match p.name {
        Some(n) if !n.is_empty() => n,
        _ => return json_error(400, "缺少查询名称"),
    };
    let flag = p.exact_flag.as_deref() == Some("true");
    let mut pl = app.pack_list.lock().await;
    match pl.search_handler(&name, flag).await {
        v => Json(json!({ "status_code": 200, "data": v })).into_response(),
    }
}

#[derive(Deserialize)]
pub struct PageIdParam {
    pub page_id: Option<String>,
}

pub async fn get_list(State(app): State<AppState>, Query(p): Query<PageIdParam>) -> Response {
    let page_id = p.page_id.unwrap_or_default();
    let mut pl = app.pack_list.lock().await;
    let msg = pl.get_pack_list(&page_id).await;
    if is_locked(&msg) {
        return locked_response();
    }
    Json(json!({ "data": msg })).into_response()
}

pub async fn get_err_list(State(app): State<AppState>) -> Response {
    let pl = app.pack_list.lock().await;
    let data = pl.get_err_list();
    Json(json!({ "data": data })).into_response()
}

#[derive(Deserialize)]
pub struct NamePayload {
    pub name: Option<String>,
}

pub async fn remove_pkg(State(app): State<AppState>, Json(body): Json<NamePayload>) -> Response {
    let name = match body.name {
        Some(n) if !n.is_empty() => n,
        _ => return json_error(400, "缺少参数"),
    };
    let mut pl = app.pack_list.lock().await;
    pl.remove_err_pack(&name);
    Json(json!({ "data": "Delete Success" })).into_response()
}

#[derive(Deserialize)]
pub struct InstallPayload {
    pub name: Option<String>,
    pub version: Option<String>,
    pub desc: Option<String>,
    pub page_id: Option<String>,
}

pub async fn install_pkg(
    State(app): State<AppState>,
    Json(body): Json<InstallPayload>,
) -> Response {
    let name = match body.name {
        Some(n) if !n.is_empty() => n,
        _ => return json_error(400, "缺少参数"),
    };
    let version = body.version.unwrap_or_default();
    let desc = body.desc.unwrap_or_default();
    let page_id = body.page_id;
    let mut pl = app.pack_list.lock().await;
    let res = pl
        .install_handler(&name, &version, &desc, page_id.as_deref())
        .await;
    match res {
        Some(state) => Json(json!({ "data": { "state": state } })).into_response(),
        None => locked_response(),
    }
}

pub async fn uninstall_pkg(State(app): State<AppState>, Json(body): Json<NamePayload>) -> Response {
    let name = match body.name {
        Some(n) if !n.is_empty() => n,
        _ => return json_error(400, "缺少参数"),
    };
    let mut pl = app.pack_list.lock().await;
    pl.uninstall_handler(&name).await;
    Json(json!({ "data": "Uninstall Success" })).into_response()
}

pub async fn cancel_install_pkg(
    State(app): State<AppState>,
    Json(body): Json<NamePayload>,
) -> Response {
    let name = match body.name {
        Some(n) if !n.is_empty() => n,
        _ => return json_error(400, "缺少参数"),
    };
    let mut pl = app.pack_list.lock().await;
    pl.cancel_install_handler(&name).await;
    Json(json!({ "data": { "state": "waiting" } })).into_response()
}

#[derive(Deserialize)]
pub struct PreParam {
    pub pre: Option<String>,
}

pub async fn get_state(State(app): State<AppState>, Query(p): Query<PreParam>) -> Response {
    let pre = p.pre.unwrap_or_default();
    let mut pl = app.pack_list.lock().await;
    let data = pl.get_state(&pre, true).await;
    Json(json!({ "data": data })).into_response()
}

pub async fn get_all_state(State(app): State<AppState>, Query(p): Query<PageIdParam>) -> Response {
    let page_id = p.page_id.unwrap_or_default();
    let mut pl = app.pack_list.lock().await;
    let data = pl.get_all_state(&page_id).await;
    if is_locked(&data) {
        return locked_response();
    }
    Json(json!({ "data": data })).into_response()
}

#[derive(Deserialize)]
pub struct UnlockPayload {
    pub page_id: Option<String>,
}

pub async fn unlock(State(app): State<AppState>, Json(body): Json<UnlockPayload>) -> Response {
    let page_id = body.page_id.unwrap_or_default();
    let mut pl = app.pack_list.lock().await;
    pl.unlock(&page_id);
    Json(json!({ "data": "ok" })).into_response()
}

pub async fn get_mirrors(State(app): State<AppState>) -> Response {
    let (mirrors, current_index) = app.pkg_manager.get_mirrors().await;
    Json(json!({ "data": { "mirrors": mirrors, "currentIndex": current_index } })).into_response()
}

#[derive(Deserialize)]
pub struct MirrorPayload {
    pub index: Option<usize>,
}

pub async fn choose_mirror(
    State(app): State<AppState>,
    Json(body): Json<MirrorPayload>,
) -> Response {
    match body.index {
        Some(i) => {
            app.pkg_manager.set_mirror_index(i).await;
            Json(json!({ "data": "ok" })).into_response()
        }
        None => json_error(400, "缺少参数"),
    }
}

pub async fn get_history(State(app): State<AppState>) -> Response {
    let h = app.history.lock().await;
    let records = h.list();
    Json(json!({ "records": records })).into_response()
}

pub async fn clear_history(State(app): State<AppState>) -> Response {
    let mut h = app.history.lock().await;
    h.clear().await;
    Json(json!({ "success": true })).into_response()
}

#[derive(Deserialize)]
pub struct IdParam {
    pub id: Option<String>,
}

pub async fn delete_history(State(app): State<AppState>, Query(p): Query<IdParam>) -> Response {
    let id = match p.id {
        Some(i) if !i.is_empty() => i,
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "success": false, "error": "缺少 id" })),
            )
                .into_response()
        }
    };
    let mut h = app.history.lock().await;
    let ok = h.delete(&id).await;
    Json(json!({ "success": ok })).into_response()
}

async fn paths_response(paths: Vec<String>, saved: Option<String>, current: String) -> Response {
    Json(json!({ "paths": paths, "savedPath": saved, "currentPath": current })).into_response()
}

pub async fn get_python_paths() -> Response {
    let paths = find_all_python_paths().await;
    let saved = get_saved_python_path().await;
    let current = current_python_path().await;
    paths_response(paths, saved, current).await
}

#[derive(Deserialize)]
pub struct PathBody {
    pub path: Option<String>,
}

pub async fn set_python_path(Json(body): Json<PathBody>) -> Response {
    match body.path {
        Some(p) if !p.is_empty() => {
            save_python_path(&p).await;
            Json(json!({ "success": true })).into_response()
        }
        _ => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "success": false, "error": "路径不能为空" })),
        )
            .into_response(),
    }
}

pub async fn get_golang_paths() -> Response {
    let paths = find_all_golang_paths().await;
    let saved = get_saved_golang_path().await;
    let current = current_golang_path().await;
    paths_response(paths, saved, current).await
}

pub async fn set_golang_path(Json(body): Json<PathBody>) -> Response {
    match body.path {
        Some(p) if !p.is_empty() => {
            save_golang_path(&p).await;
            Json(json!({ "success": true })).into_response()
        }
        _ => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "success": false, "error": "路径不能为空" })),
        )
            .into_response(),
    }
}

pub async fn get_bun_paths() -> Response {
    let paths = find_all_bun_paths().await;
    let saved = get_saved_bun_path().await;
    let current = current_bun_path().await;
    paths_response(paths, saved, current).await
}

pub async fn set_bun_path(Json(body): Json<PathBody>) -> Response {
    match body.path {
        Some(p) if !p.is_empty() => {
            save_bun_path(&p).await;
            Json(json!({ "success": true })).into_response()
        }
        _ => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "success": false, "error": "路径不能为空" })),
        )
            .into_response(),
    }
}

pub async fn get_status(State(app): State<AppState>) -> Response {
    let err = app.server_error.lock().await.clone();
    Json(json!({ "error": err })).into_response()
}
