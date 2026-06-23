use std::sync::Arc;

use serde::Serialize;
use tokio::sync::Mutex;

use crate::ai::explain::AiService;
use crate::history::HistoryStore;
use crate::python::lib_list::PackageList;
use crate::python::package_manager::PackageManager;
use crate::python::runner_pool::RunnerPool;
use crate::settings::Settings;
use crate::updater::self_update::UpdaterService;
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
    pub settings: Arc<Mutex<Settings>>,
    pub runner_pool: Arc<RunnerPool>,
    pub started_at: std::time::Instant,
    pub ai: Arc<AiService>,
    pub updater: Arc<UpdaterService>,
}
