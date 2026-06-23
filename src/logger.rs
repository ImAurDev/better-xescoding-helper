use tracing_subscriber::{fmt, prelude::*, EnvFilter};

use crate::utils::log_buffer::{record_log, LogEntry};

pub fn init() {
    let filter = EnvFilter::try_from_env("LOG_LEVEL").unwrap_or_else(|_| EnvFilter::new("debug"));
    tracing_subscriber::registry()
        .with(filter)
        .with(
            fmt::layer()
                .with_target(false)
                .without_time()
                .with_level(true),
        )
        .init();

    let prev_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let loc = info.location().map(|l| l.to_string()).unwrap_or_default();
        let msg_static = info.payload().downcast_ref::<String>().cloned();
        let msg = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or(msg_static)
            .unwrap_or_default();
        let entry = LogEntry {
            timestamp: crate::history::now_millis(),
            level: "ERROR".to_string(),
            target: "panic".to_string(),
            message: format!("panic at {loc}: {msg}"),
        };
        record_log(entry);
        prev_hook(info);
    }));
}

pub fn record_event(_level: &str, _target: &str, _message: &str) {}
