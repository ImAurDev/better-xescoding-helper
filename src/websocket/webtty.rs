use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use base64::Engine;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot, Mutex};

use crate::download::assets::{AssetJson, AssetManage, CompareTag};
use crate::utils::flex::flex_string_opt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Wait = 1,
    Ready = 2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetState {
    Checking = 0,
    Ready = 1,
    Error = 2,
}

pub enum WsCmd {
    BackendEvent { data: String },
    InnerErr { inner_err: String },
    CommandRun,
    DangerRequest { payload: String },
}

pub enum WsOut {
    Text(String),
    Close,
}

pub enum MsgAction {
    None,
    HandleAssets(AssetJson),
    WaitForAssets(AssetJson, u64),
    ConnClose { was_ready: bool },
}

#[derive(Deserialize)]
struct ClientMessage {
    #[serde(rename = "type")]
    msg_type: Option<String>,
    handle: Option<String>,
    #[serde(rename = "projectId", default, deserialize_with = "flex_string_opt")]
    project_id: Option<String>,
    xml: Option<String>,
    assets: Option<Vec<crate::download::assets::AssetInfo>>,
    preload: Option<String>,
    #[serde(default, deserialize_with = "flex_string_opt")]
    cols: Option<String>,
    #[serde(default, deserialize_with = "flex_string_opt")]
    rows: Option<String>,
}

fn asset_json_from(d: ClientMessage) -> AssetJson {
    AssetJson {
        project_id: d.project_id.unwrap_or_default(),
        assets: d.assets.unwrap_or_default(),
        xml: d.xml,
        preload: d.preload,
        extra: HashMap::new(),
    }
}

fn is_chinese(ch: &str) -> bool {
    if let Some(c) = ch.chars().next() {
        let code = c as u32;
        code >= 0x4e00 && code <= 0x9fa5
    } else {
        false
    }
}

pub const WS_CHANNEL_CAPACITY: usize = 128;

#[allow(dead_code)]
pub struct Webtty {
    client_tx: Option<mpsc::Sender<WsOut>>,
    client_id: u64,
    state: State,
    path: Option<String>,
    code: Option<String>,
    enable: bool,
    inputs: VecDeque<String>,
    tmp_inputs: Vec<String>,
    first_msg: Option<String>,
    load_flag: bool,
    has_run: bool,
    wait_to_close: bool,
    am: Option<AssetManage>,
    asset_states: HashMap<String, AssetState>,
    pub(crate) danger_tx: Option<oneshot::Sender<bool>>,
    pub(crate) danger_buf: String,
    pub(crate) terminal_cols: u16,
    pub(crate) terminal_rows: u16,
}

impl Webtty {
    pub fn new() -> Self {
        Self {
            client_tx: None,
            client_id: 0,
            state: State::Wait,
            path: None,
            code: None,
            enable: true,
            inputs: VecDeque::new(),
            tmp_inputs: Vec::new(),
            first_msg: None,
            load_flag: true,
            has_run: false,
            wait_to_close: false,
            am: None,
            asset_states: HashMap::new(),
            danger_tx: None,
            danger_buf: String::new(),
            terminal_cols: 80,
            terminal_rows: 24,
        }
    }

    pub fn new_client(&mut self, client_id: u64, tx: mpsc::Sender<WsOut>) {
        if let Some(old) = self.client_tx.take() {
            self.enable = false;
            let _ = old.try_send(WsOut::Text(format!("7{}", base64::engine::general_purpose::STANDARD.encode(serde_json::json!({"Type":"compileFail","Info":"\r\n\r\n新的连接建立,自动断开"}).to_string()))));
            let _ = old.try_send(WsOut::Close);
        } else {
            self.enable = true;
        }
        self.client_tx = Some(tx);
        self.client_id = client_id;
        self.first_msg = None;
    }

    pub fn client_left(&mut self, client_id: u64) {
        if self.client_id == client_id {
            self.client_tx = None;
            self.state_to_wait();
        }
    }

    pub async fn message_received(&mut self, client_id: u64, msg: &str) -> MsgAction {
        if msg.is_empty() {
            return MsgAction::None;
        }
        let first = msg.chars().next();
        let first_len = first.map(|c| c.len_utf8()).unwrap_or(0);
        let rest = &msg[first_len..];

        if self.danger_tx.is_some() {
            return self.try_resolve_danger(first, rest, client_id).await;
        }

        match first {
            Some('7') => {
                let data: ClientMessage = match serde_json::from_str(rest) {
                    Ok(d) => d,
                    Err(e) => {
                        tracing::error!("无法转译数据: {e}");
                        return MsgAction::None;
                    }
                };
                if let Some(c) = data.cols.as_ref().and_then(|s| s.parse::<u16>().ok()) {
                    if c >= 20 && c <= 1000 {
                        self.terminal_cols = c;
                    }
                }
                if let Some(r) = data.rows.as_ref().and_then(|s| s.parse::<u16>().ok()) {
                    if r >= 5 && r <= 200 {
                        self.terminal_rows = r;
                    }
                }
                if data.msg_type.as_deref() == Some("assets") {
                    return MsgAction::HandleAssets(asset_json_from(data));
                }
                if data.msg_type.as_deref() == Some("conn")
                    && data.handle.as_deref() == Some("close")
                {
                    self.wait_to_close = true;
                    let was_ready = self.state == State::Ready;
                    self.state_to_wait();
                    return MsgAction::ConnClose { was_ready };
                }
                if self.client_id == client_id {
                    self.path = Some(String::new());
                    if let Some(pid) = &data.project_id {
                        if !pid.is_empty() {
                            self.path = Some(pid.clone());
                            return MsgAction::WaitForAssets(asset_json_from(data), self.client_id);
                        }
                    }
                }
                MsgAction::None
            }
            Some('1') if self.state == State::Ready => {
                if self.client_id == client_id {
                    if rest == "\r" || rest == "\n" {
                        let whole: String = self.tmp_inputs.concat();
                        self.inputs.push_back(whole);
                        self.tmp_inputs.clear();
                        self.send_to_web("1", "\r\n").await;
                    } else if rest == "\u{007F}" {
                        if let Some(last) = self.tmp_inputs.pop() {
                            let is_ch = is_chinese(&last);
                            self.send_mv_msg(is_ch).await;
                        }
                    } else {
                        for ch in rest.chars() {
                            self.tmp_inputs.push(ch.to_string());
                        }
                        self.send_to_web("1", rest).await;
                    }
                }
                MsgAction::None
            }
            _ => {
                if self.first_msg.is_none() {
                    self.first_msg = Some(msg.to_string());
                    let _: Result<Value, _> = serde_json::from_str(msg);
                }
                MsgAction::None
            }
        }
    }

    pub async fn send_msg(&mut self, cmd: &WsCmd) {
        if self.state != State::Ready && !self.wait_to_close {
            return;
        }
        match cmd {
            WsCmd::BackendEvent { data } => {
                let pre = data.replace('\n', "\r\n");
                self.send_to_web("1", &pre).await;
                if pre.contains(" * Running on") {
                    if let Some(host) = extract_flask_host(&pre) {
                        let signal = json!({"type":"flask","host":host}).to_string();
                        self.form_msg_send("signal", &signal).await;
                    }
                }
            }
            WsCmd::InnerErr { inner_err } => {
                self.form_msg_send("runInfo", &format!("\r\n{}", inner_err))
                    .await;
                self.state_to_wait();
                self.close_cur_client();
            }
            WsCmd::CommandRun => {
                self.handle_close().await;
                self.form_msg_send("runInfo", "\r\n\r\n程序运行结束").await;
                self.close_cur_client();
                self.enable = true;
                self.state_to_wait();
            }
            WsCmd::DangerRequest { payload } => {
                self.form_msg_send("dangerConfirm", &payload).await;
            }
        }
    }

    pub async fn handle_close(&mut self) {
        let msg = match self.am.as_mut() {
            None => json!({"type":"changed"}),
            Some(am) => {
                let tag = am.compare_assets();
                match tag {
                    CompareTag::Oversize => json!({"type":"file_err","reason":"oversize"}),
                    CompareTag::Count => json!({"type":"file_err","reason":"count"}),
                    CompareTag::Changed(obj) => {
                        let mut m = obj;
                        if let Value::Object(ref mut map) = m {
                            map.insert("type".into(), Value::String("changed".into()));
                        } else {
                            m = json!({"type":"changed"});
                        }
                        m
                    }
                }
            }
        };
        self.form_msg_send("signal", &msg.to_string()).await;
    }

    pub fn close_cur_client(&mut self) {
        if let Some(tx) = self.client_tx.take() {
            let _ = tx.try_send(WsOut::Close);
        }
    }

    async fn send_mv_msg(&mut self, is_ch: bool) {
        let tag = if is_ch { "CCAICCAI" } else { "CCAI" };
        if let Some(tx) = &self.client_tx {
            if let Err(e) = tx.send(WsOut::Text(format!("1{}", tag))).await {
                tracing::warn!("WebSocket发送失败: {e}");
            }
        }
    }

    fn set_state(&mut self, new_state: State) {
        self.state = new_state;
    }

    pub fn get_state(&mut self) -> State {
        if self.state == State::Wait
            && self.code.is_some()
            && self.path.is_some()
            && self.enable
            && self.client_tx.is_some()
        {
            self.set_state(State::Ready);
        }
        self.state
    }

    pub fn get_code_and_path(&self) -> (Option<String>, Option<String>, Option<String>) {
        (self.code.clone(), self.path.clone(), self.first_msg.clone())
    }

    pub async fn form_msg_send(&self, com_type: &str, msg: &str) {
        let obj = json!({"Type": com_type, "Info": msg});
        self.send_to_web("7", &obj.to_string()).await;
    }

    pub async fn send_to_web(&self, msg_type: &str, msg: &str) {
        if let Some(tx) = &self.client_tx {
            let encoded = base64::engine::general_purpose::STANDARD.encode(msg);
            if let Err(e) = tx
                .send(WsOut::Text(format!("{}{}", msg_type, encoded)))
                .await
            {
                tracing::warn!("WebSocket发送失败: {e}");
            }
        }
    }

    pub fn poxy_ready(&mut self) {
        self.enable = true;
    }

    pub fn fetch_next_input(&mut self) -> Option<String> {
        self.inputs.pop_front()
    }

    pub fn state_to_wait(&mut self) {
        self.set_state(State::Wait);
        self.path = None;
        self.code = None;
    }

    pub fn set_asset_state(&mut self, pid: &str, state: AssetState) {
        self.asset_states.insert(pid.to_string(), state);
    }

    pub fn asset_state(&self, pid: &str) -> Option<AssetState> {
        self.asset_states.get(pid).copied()
    }

    pub fn set_code(&mut self, code: Option<String>) {
        self.code = code;
    }

    pub fn get_code(&self) -> Option<String> {
        self.code.clone()
    }

    pub fn set_has_run(&mut self, v: bool) {
        self.has_run = v;
    }

    pub fn set_am(&mut self, am: AssetManage) {
        self.am = Some(am);
    }

    #[allow(dead_code)]
    pub fn am(&mut self) -> Option<&mut AssetManage> {
        self.am.as_mut()
    }

    pub fn client_id(&self) -> u64 {
        self.client_id
    }

    pub fn client_tx_is_some(&self) -> bool {
        self.client_tx.is_some()
    }

    pub async fn handle_conn_close(&mut self, was_ready: bool) {
        self.wait_to_close = false;
        if was_ready {
            self.handle_close().await;
        }
        self.form_msg_send("compileFail", "连接终止").await;
        self.close_cur_client();
        self.enable = true;
    }
}

fn extract_flask_host(text: &str) -> Option<String> {
    let re = regex::Regex::new(r"Running on (.+?) ").ok()?;
    let cap = re.captures(text)?;
    let host = cap.get(1)?.as_str().replace("0.0.0.0", "127.0.0.1");
    Some(host)
}

pub async fn handle_assets(webtty: Arc<Mutex<Webtty>>, message: AssetJson) -> bool {
    let pid = message.project_id.clone();
    {
        let mut wt = webtty.lock().await;
        wt.set_asset_state(&pid, AssetState::Checking);
    }
    let mut am = AssetManage::new();
    let res = am.handle_assets_json(message).await;
    let ok = res.ok;
    {
        let mut wt = webtty.lock().await;
        if ok {
            wt.set_asset_state(&pid, AssetState::Ready);
        } else {
            wt.set_asset_state(&pid, AssetState::Error);
            wt.form_msg_send("assets", "err").await;
        }
        wt.set_am(am);
    }
    ok
}

pub async fn wait_for_assets(webtty: Arc<Mutex<Webtty>>, message: AssetJson, aid: u64) {
    let pid = message.project_id.clone();
    let mut cnt = 0u32;
    let mut load_flag = true;

    loop {
        let (state, client_ok) = {
            let wt = webtty.lock().await;
            (
                wt.asset_state(&pid),
                wt.client_id() == aid && wt.client_tx_is_some(),
            )
        };
        if state != Some(AssetState::Checking) || !client_ok {
            break;
        }
        cnt += 1;
        if cnt == 100 {
            let wt = webtty.lock().await;
            wt.form_msg_send("assets", "start").await;
            load_flag = false;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }

    {
        let wt = webtty.lock().await;
        if !(wt.client_id() == aid && wt.client_tx_is_some()) {
            return;
        }
    }

    if load_flag {
        let res = handle_assets(webtty.clone(), message.clone()).await;
        let mut wt = webtty.lock().await;
        wt.set_asset_state(
            &pid,
            if res {
                AssetState::Ready
            } else {
                AssetState::Error
            },
        );
        if res {
            if wt.get_code().is_none() {
                wt.form_msg_send("assets", "end").await;
            }
            wt.set_code(message.xml.clone());
            wt.set_has_run(true);
        } else {
            wt.state_to_wait();
            wt.close_cur_client();
        }
    } else {
        let res = handle_assets(webtty.clone(), message.clone()).await;
        let mut wt = webtty.lock().await;
        wt.set_asset_state(
            &pid,
            if res {
                AssetState::Ready
            } else {
                AssetState::Error
            },
        );
        if wt.client_id() == aid && wt.client_tx_is_some() {
            if res {
                if wt.get_code().is_none() {
                    wt.form_msg_send("assets", "end").await;
                }
                wt.set_code(message.xml.clone());
                wt.set_has_run(true);
            } else {
                wt.state_to_wait();
                wt.close_cur_client();
            }
        }
    }
}
