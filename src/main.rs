mod ai;
mod config;
mod dep_graph;
mod download;
mod frontend;
mod health;
mod history;
mod http;
mod logger;
mod python;
mod sandbox;
mod settings;
mod state;
mod updater;
mod utils;
mod websocket;

use std::sync::Arc;
use std::time::Instant;

use tokio::net::TcpListener;
use tokio::sync::Mutex;

use ai::explain::AiService;
use config::PORT_PAIRS;
use history::HistoryStore;
use http::port::is_port_available;
use http::router;
use python::lib_list::{PackageList, run_auto_state};
use python::metrics as run_metrics;
use python::package_manager::PackageManager;
use python::prewarm;
use python::runner::Runner;
use python::runner_pool::RunnerPool;
use settings::Settings;
use state::{AppState, ServerError};
use updater::self_update::UpdaterService;
use utils::cache_cleaner;
use websocket::webtty::Webtty;
use websocket::{WsState, build_router};

#[tokio::main]
async fn main() {
    logger::init();
    tracing::info!("欢迎使用 更好的学而思编程助手 v2 | 作者: 极光");

    cache_cleaner::start();

    let started_at = Instant::now();

    let settings_store = settings::SettingsStore::load().await;
    let settings_arc = settings_store.shared();

    {
        let s = settings_arc.lock().await;
        settings::set_global_proxy(s.proxy.clone());
    }

    let history = Arc::new(Mutex::new(HistoryStore::new()));
    history.lock().await.init().await;

    let pkg_manager = PackageManager::new();
    pkg_manager.init().await;

    let pack_list = Arc::new(Mutex::new(PackageList::new(pkg_manager.clone())));

    let webtty = Arc::new(Mutex::new(Webtty::new()));

    let server_error: Arc<Mutex<Option<ServerError>>> = Arc::new(Mutex::new(None));

    let ai = Arc::new(AiService::new(settings_arc.clone(), history.clone()));
    let updater = Arc::new(UpdaterService::new(settings_arc.clone()));

    let result = find_available_ports().await;
    let (http_port, ws_port) = match result {
        Some(ports) => {
            server_error.lock().await.take();
            ports
        }
        None => {
            server_error.lock().await.replace(ServerError {
                message: "未找到可用端口，请关闭其他占用端口的程序".into(),
                kind: "port".into(),
            });
            (0u16, 0u16)
        }
    };

    let pool = Arc::new(RunnerPool::new(
        webtty.clone(),
        history.clone(),
        pkg_manager.clone(),
        settings_arc.clone(),
    ));
    pool.start();

    {
        let s = settings_arc.lock().await;
        pool.configure(s.max_concurrent_runners.max(1), 32).await;
    }

    let runner = Arc::new(Runner::new(
        webtty.clone(),
        history.clone(),
        pkg_manager.clone(),
        settings_arc.clone(),
        ai.clone(),
    ));
    runner.start();

    {
        let h = settings_arc.lock().await;
        run_metrics::configure_capacity(h.cleanup.run_metrics_history).await;
    }

    if http_port > 0 {
        let app = AppState {
            history: history.clone(),
            pack_list: pack_list.clone(),
            pkg_manager: pkg_manager.clone(),
            server_error: server_error.clone(),
            webtty: webtty.clone(),
            settings: settings_arc.clone(),
            runner_pool: pool.clone(),
            started_at,
            ai: ai.clone(),
            updater: updater.clone(),
        };
        let router = router::build(app);
        let listener = match TcpListener::bind(("0.0.0.0", http_port)).await {
            Ok(l) => l,
            Err(e) => {
                tracing::error!("发生错误: {e}");
                return;
            }
        };
        tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
    }

    if ws_port > 0 {
        let ws_state = WsState {
            webtty: webtty.clone(),
        };
        let ws_router = build_router(ws_state);
        let listener = match TcpListener::bind(("0.0.0.0", ws_port)).await {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!("WebSocket 监听失败: {e}");
                return;
            }
        };
        tokio::spawn(async move {
            let _ = axum::serve(listener, ws_router).await;
        });
    }

    let go_result = detect_golang().await;
    let bun_result = detect_bun().await;

    if go_result.is_none() {
        tracing::warn!("Golang 未检测到");
    }
    if bun_result.is_none() {
        tracing::warn!("Bun 未检测到");
    }

    if http_port > 0 {
        tracing::info!("HTTP 服务已启动: httpPort={http_port}");
        tracing::info!("WebSocket 服务已启动: wsPort={ws_port}");
    } else {
        tracing::error!("HTTP 服务启动失败: 未找到可用端口");
    }

    tokio::spawn(run_auto_state(pack_list));

    spawn_prewarm_if_enabled(settings_arc.clone(), webtty.clone()).await;

    spawn_updater_check_if_enabled(updater.clone()).await;

    tokio::signal::ctrl_c().await.ok();
}

async fn spawn_prewarm_if_enabled(
    settings: Arc<Mutex<Settings>>,
    webtty: Arc<Mutex<Webtty>>,
) {
    let (config, full_settings) = {
        let s = settings.lock().await;
        (s.prewarm.clone(), s.clone())
    };
    if !config.enabled {
        return;
    }
    if prewarm::is_already_warmed(24) {
        tracing::info!("预热标记在 24 小时内已存在,跳过");
        return;
    }
    tokio::spawn(async move {
        let report = prewarm::run_prewarm(&config, &full_settings, Some(webtty)).await;
        tracing::info!(
            "预热完成: 成功 {} 失败 {} 耗时 {}ms",
            report.installed.len(),
            report.failed.len(),
            report.duration_ms
        );
    });
}

async fn spawn_updater_check_if_enabled(updater: Arc<UpdaterService>) {
    tokio::spawn(async move {
        updater.auto_check().await;
    });
}

async fn find_available_ports() -> Option<(u16, u16)> {
    for &(port, port2) in PORT_PAIRS {
        let (a, b) = tokio::join!(is_port_available(port), is_port_available(port2));
        if a && b {
            return Some((port, port2));
        }
    }
    None
}

async fn detect_golang() -> Option<String> {
    python::find_all_golang_paths().await.into_iter().next()
}

async fn detect_bun() -> Option<String> {
    python::find_all_bun_paths().await.into_iter().next()
}
