mod config;
mod download;
mod frontend;
mod history;
mod http;
mod logger;
mod python;
mod state;
mod utils;
mod websocket;

use std::sync::Arc;

use tokio::net::TcpListener;
use tokio::sync::Mutex;

use config::PORT_PAIRS;
use history::HistoryStore;
use http::port::is_port_available;
use http::router;
use python::lib_list::{run_auto_state, PackageList};
use python::package_manager::PackageManager;
use python::runner::Runner;
use state::{AppState, ServerError};
use utils::cache_cleaner;
use websocket::webtty::Webtty;
use websocket::{build_router, WsState};

#[tokio::main]
async fn main() {
    logger::init();
    tracing::info!("欢迎使用 更好的学而思编程助手 v1 | 作者: 极光");

    cache_cleaner::start();

    let history = Arc::new(Mutex::new(HistoryStore::new()));
    history.lock().await.init().await;

    let pkg_manager = PackageManager::new();
    pkg_manager.init().await;

    let pack_list = Arc::new(Mutex::new(PackageList::new(pkg_manager.clone())));

    let webtty = Arc::new(Mutex::new(Webtty::new()));

    let server_error: Arc<Mutex<Option<ServerError>>> = Arc::new(Mutex::new(None));

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

    let app = AppState {
        history: history.clone(),
        pack_list: pack_list.clone(),
        pkg_manager: pkg_manager.clone(),
        server_error: server_error.clone(),
        webtty: webtty.clone(),
    };

    if http_port > 0 {
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

    let runner = Arc::new(Runner::new(
        webtty.clone(),
        history.clone(),
        pkg_manager.clone(),
    ));
    runner.start();

    if ws_port > 0 {
        let ws_state = WsState {
            webtty: webtty.clone(),
        };
        let ws_router = build_router(ws_state);
        let listener = match TcpListener::bind(("0.0.0.0", ws_port)).await {
            Ok(l) => l,
            Err(e) => {
                tracing::error!("WebSocket 监听失败: {e}");
                return;
            }
        };
        tokio::spawn(async move {
            let _ = axum::serve(listener, ws_router).await;
        });
    }

    let go_result = detect_golang().await;
    let bun_result = detect_bun().await;

    if let Some(p) = go_result {
        tracing::info!("Golang 可用: path={p}");
    } else {
        tracing::warn!("Golang 未检测到");
    }
    if let Some(p) = bun_result {
        tracing::info!("Bun 可用: path={p}");
    } else {
        tracing::warn!("Bun 未检测到");
    }

    if http_port > 0 {
        tracing::info!("HTTP 服务已启动: httpPort={http_port}");
        tracing::info!("WebSocket 服务已启动: wsPort={ws_port}");
    } else {
        tracing::error!("HTTP 服务启动失败: 未找到可用端口");
    }

    tokio::spawn(run_auto_state(pack_list));

    tokio::signal::ctrl_c().await.ok();
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
