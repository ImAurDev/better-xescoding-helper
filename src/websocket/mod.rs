pub mod danger;
pub mod webtty;

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::Mutex;

use webtty::{MsgAction, Webtty, WsOut, WS_CHANNEL_CAPACITY};

pub use danger::{DangerHit, DANGER_CONFIRM_TIMEOUT_SECS};

#[derive(Clone)]
pub struct WsState {
    pub webtty: Arc<Mutex<Webtty>>,
}

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<WsState>) -> Response {
    ws.max_message_size(256 * 1024 * 1024)
        .on_upgrade(move |socket| handle_ws(socket, state.webtty.clone()))
}

async fn handle_ws(socket: WebSocket, webtty: Arc<Mutex<Webtty>>) {
    let (mut sender, mut receiver) = socket.split();
    let client_id = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let (tx, mut rx) = tokio::sync::mpsc::channel::<WsOut>(WS_CHANNEL_CAPACITY);

    {
        let mut wt = webtty.lock().await;
        wt.new_client(client_id, tx);
    }

    let send_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            let out = match msg {
                WsOut::Text(s) => Message::Text(s.into()),
                WsOut::Close => Message::Close(None),
            };
            if sender.send(out).await.is_err() {
                tracing::warn!("WebSocket连接已关闭,停止发送");
                break;
            }
        }
    });

    let wt = webtty.clone();
    let receive_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = receiver.next().await {
            let text = match msg {
                Message::Text(t) => t.to_string(),
                Message::Binary(b) => String::from_utf8_lossy(&b).to_string(),
                Message::Close(_) => break,
                _ => continue,
            };
            let action = {
                let mut wt = wt.lock().await;
                wt.message_received(client_id, &text).await
            };
            match action {
                MsgAction::None => {}
                MsgAction::HandleAssets(json) => {
                    let _ = webtty::handle_assets(wt.clone(), json).await;
                }
                MsgAction::WaitForAssets(json, aid) => {
                    webtty::wait_for_assets(wt.clone(), json, aid).await;
                }
                MsgAction::ConnClose { was_ready } => {
                    let mut w = wt.lock().await;
                    w.handle_conn_close(was_ready).await;
                }
            }
        }
        let mut w = wt.lock().await;
        w.client_left(client_id);
    });

    let _ = tokio::join!(send_task, receive_task);
}

pub fn build_router(state: WsState) -> Router {
    Router::new().route("/", get(ws_handler).with_state(state))
}
