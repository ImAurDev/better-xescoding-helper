use tokio::net::TcpListener;

pub async fn is_port_available(port: u16) -> bool {
    match TcpListener::bind(("0.0.0.0", port)).await {
        Ok(_) => true,
        Err(e) => {
            tracing::warn!("端口 {port} 不可用: {e}");
            false
        }
    }
}
