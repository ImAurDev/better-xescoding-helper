use serde_json::{json, Value};
use tokio::sync::oneshot;

use super::webtty::{MsgAction, Webtty, WsCmd};

pub const DANGER_CONFIRM_TIMEOUT_SECS: u64 = 300;

#[derive(Debug, Clone)]
pub struct DangerHit {
    pub label: String,
    pub hint: String,
    pub line: usize,
    pub code: String,
}

pub fn is_danger_junk(s: &str) -> bool {
    if s.is_empty() {
        return true;
    }
    let lower = s.to_ascii_lowercase();
    if lower == "undefined"
        || lower == "null"
        || lower == "nan"
        || lower == "none"
        || lower == "[object object]"
        || lower == "[object undefined]"
        || lower == "false"
        || lower == "true"
    {
        return true;
    }
    if lower.contains("undefined")
        || lower.contains("[object")
        || lower.starts_with("nan")
        || lower.starts_with("null")
    {
        return true;
    }
    false
}

pub fn build_danger_prompt(hits: &[DangerHit], timeout_secs: u64, cols: usize) -> String {
    let width = cols.min(120).max(40);
    let mut s = String::new();
    s.push_str("\r\n");
    s.push_str(&format!(
        "\x1b[1;33m⚠  \x1b[1;97m检测到 \x1b[1;91m{}\x1b[1;97m 处危险代码\x1b[0m\r\n",
        hits.len()
    ));
    for (i, h) in hits.iter().enumerate() {
        let code_line = h.code.trim();
        let max_code_len = width.saturating_sub(14);
        let display_code = if code_line.chars().count() > max_code_len {
            let truncated: String = code_line
                .chars()
                .take(max_code_len.saturating_sub(3))
                .collect();
            format!("{truncated}...")
        } else {
            code_line.to_string()
        };
        s.push_str(&format!(
            "  \x1b[1;97m{}.\x1b[0m \x1b[1;96mL{:>3}\x1b[0m  \x1b[1;91m{}\x1b[0m\r\n",
            i + 1,
            h.line,
            display_code
        ));
        s.push_str(&format!(
            "        \x1b[37m[{}]\x1b[0m \x1b[97m{}\x1b[0m\r\n",
            h.label, h.hint
        ));
    }
    s.push_str(&format!(
        "\x1b[1;97m继续运行?\x1b[0m \x1b[1;97m(\x1b[1;92my\x1b[1;97m/\x1b[1;91mN\x1b[1;97m, \x1b[1;93m{}秒\x1b[1;97m后取消)\x1b[0m: ",
        timeout_secs
    ));
    s
}

pub fn build_danger_signal(hits: &[DangerHit]) -> String {
    let items: Vec<Value> = hits
        .iter()
        .map(|h| {
            json!({
                "label": h.label,
                "hint": h.hint,
                "line": h.line,
                "code": h.code,
            })
        })
        .collect();
    let payload = json!({
        "type": "dangerConfirm",
        "count": hits.len(),
        "timeoutSecs": DANGER_CONFIRM_TIMEOUT_SECS,
        "items": items,
    });
    payload.to_string()
}

impl Webtty {
    #[allow(dead_code)]
    pub fn terminal_cols(&self) -> u16 {
        self.terminal_cols
    }

    pub fn take_danger_tx(&mut self) -> Option<oneshot::Sender<bool>> {
        self.danger_tx.take()
    }

    pub fn clear_danger_tx(&mut self) {
        self.danger_tx = None;
        self.danger_buf.clear();
    }

    pub async fn begin_danger_confirm(
        &mut self,
        hits: &[DangerHit],
        timeout_secs: u64,
    ) -> Option<oneshot::Receiver<bool>> {
        if !self.client_tx_is_some() || hits.is_empty() {
            return None;
        }
        if self.danger_tx.is_some() {
            return None;
        }
        let (tx, rx) = oneshot::channel::<bool>();
        self.danger_tx = Some(tx);
        self.danger_buf.clear();

        let cols = self.terminal_cols.max(40) as usize;
        let payload = build_danger_prompt(hits, timeout_secs, cols);
        self.send_msg(&WsCmd::BackendEvent { data: payload }).await;
        self.send_msg(&WsCmd::DangerRequest {
            payload: build_danger_signal(hits),
        })
        .await;
        Some(rx)
    }

    pub async fn finish_danger_confirm(&mut self, allow: bool, timeout_secs: u64, timed_out: bool) {
        self.danger_buf.clear();
        let notice = if timed_out {
            format!(
                "\r\n\x1b[1;93m⚠ [安全] 等待确认超时(>{}秒),已自动取消\x1b[0m\r\n",
                timeout_secs
            )
        } else if allow {
            "\r\n\x1b[1;92m[安全] 已确认,继续执行\x1b[0m\r\n".to_string()
        } else {
            "\r\n\x1b[1;91m[安全] 已取消,本次运行被中止\x1b[0m\r\n".to_string()
        };
        self.send_msg(&WsCmd::BackendEvent { data: notice }).await;
    }

    pub async fn try_resolve_danger(
        &mut self,
        first: Option<char>,
        rest: &str,
        client_id: u64,
    ) -> MsgAction {
        if self.client_id() != client_id {
            return MsgAction::None;
        }
        match first {
            Some('9') => {
                if let Some(tx) = self.take_danger_tx() {
                    self.danger_buf.clear();
                    let _ = tx.send(false);
                }
            }
            Some('1') => {
                if rest == "\r" || rest == "\n" {
                    self.danger_buf.push('\n');
                    self.send_to_web("1", "\r\n").await;
                    let line = std::mem::take(&mut self.danger_buf);
                    let lower = line.trim().to_ascii_lowercase();
                    if is_danger_junk(&lower) {
                        return MsgAction::None;
                    }
                    let allow = matches!(
                        lower.as_str(),
                        "y" | "yes"
                            | "y\r"
                            | "y\n"
                            | "allow"
                            | "ok"
                            | "1"
                            | "确认"
                            | "是"
                            | "运行"
                            | "继续"
                            | "继续运行"
                    );
                    if let Some(tx) = self.take_danger_tx() {
                        let _ = tx.send(allow);
                    }
                } else if rest == "\u{007F}" {
                    if !self.danger_buf.is_empty() {
                        self.danger_buf.pop();
                        self.send_to_web("1", "\u{0008} \u{0008}").await;
                    }
                } else if rest.chars().all(|c| c == '\u{0008}' || c == '\u{007F}') {
                    if !self.danger_buf.is_empty() {
                        self.danger_buf.pop();
                        self.send_to_web("1", "\u{0008} \u{0008}").await;
                    }
                } else if is_danger_junk(&rest.to_ascii_lowercase()) {
                    return MsgAction::None;
                } else if rest.to_ascii_lowercase().contains("undefined")
                    || rest.to_ascii_lowercase().contains("[object")
                    || rest.to_ascii_lowercase().starts_with("nan")
                {
                    return MsgAction::None;
                } else {
                    for ch in rest.chars() {
                        if !ch.is_control() {
                            self.danger_buf.push(ch);
                            let mut buf = [0u8; 4];
                            let s = ch.encode_utf8(&mut buf);
                            self.send_to_web("1", s).await;
                        }
                    }
                }
            }
            Some('7') => {
                if let Ok(v) = serde_json::from_str::<Value>(rest) {
                    let allow_opt = v
                        .get("allow")
                        .and_then(|x| x.as_bool())
                        .or_else(|| v.get("Allow").and_then(|x| x.as_bool()));
                    if let Some(allow) = allow_opt {
                        if let Some(tx) = self.take_danger_tx() {
                            self.danger_buf.clear();
                            let _ = tx.send(allow);
                        }
                    }
                }
            }
            _ => {}
        }
        MsgAction::None
    }
}
