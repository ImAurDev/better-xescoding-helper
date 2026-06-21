use std::sync::Arc;

use serde::Serialize;
use tokio::sync::Mutex;

use crate::history::HistoryStore;
use crate::python::lib_list::PackageList;
use crate::python::package_manager::PackageManager;
use crate::websocket::webtty::Webtty;

#[derive(Clone, Serialize)]
pub struct ServerError {
    pub message: String,
    #[serde(rename = "type")]
    pub kind: String,
}

#[derive(Clone)]
#[allow(dead_code)]
pub struct AppState {
    pub history: Arc<Mutex<HistoryStore>>,
    pub pack_list: Arc<Mutex<PackageList>>,
    pub pkg_manager: PackageManager,
    pub server_error: Arc<Mutex<Option<ServerError>>>,
    pub webtty: Arc<Mutex<Webtty>>,
}
