use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::settings::AiConfig;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub stream: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatChoice {
    pub index: i64,
    pub message: ChatMessage,
    #[serde(default)]
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatUsage {
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub total_tokens: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatResponse {
    pub id: String,
    pub model: String,
    pub choices: Vec<ChatChoice>,
    #[serde(default)]
    pub usage: Option<ChatUsage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiError {
    pub message: String,
    #[serde(default)]
    pub r#type: Option<String>,
    #[serde(default)]
    pub code: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiErrorEnvelope {
    pub error: ApiError,
}

pub struct Client {
    cfg: AiConfig,
    http: reqwest::Client,
}

impl Client {
    pub fn new(cfg: AiConfig) -> Self {
        let timeout = Duration::from_secs(cfg.timeout_secs.max(5));
        let http = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .unwrap_or_default();
        Self { cfg, http }
    }

    pub fn is_configured(&self) -> bool {
        self.cfg.enabled
            && self
                .cfg
                .api_key
                .as_ref()
                .map(|k| !k.is_empty())
                .unwrap_or(false)
    }

    pub async fn chat(&self, messages: Vec<ChatMessage>) -> Result<ChatResponse, String> {
        if !self.is_configured() {
            return Err("AI 未启用或缺少 api_key".to_string());
        }
        let url = format!(
            "{}/chat/completions",
            self.cfg.effective_base_url().trim_end_matches('/')
        );
        let req = ChatRequest {
            model: self.cfg.effective_model(),
            messages,
            temperature: Some(self.cfg.temperature),
            max_tokens: Some(self.cfg.max_tokens.max(64)),
            stream: false,
        };
        let mut rb = self
            .http
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&req);
        if let Some(key) = &self.cfg.api_key {
            if !key.is_empty() {
                rb = rb.bearer_auth(key);
            }
        }
        let resp = rb.send().await.map_err(|e| format!("请求失败: {e}"))?;
        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| format!("读取响应失败: {e}"))?;
        if !status.is_success() {
            if let Ok(env) = serde_json::from_str::<ApiErrorEnvelope>(&text) {
                return Err(format!(
                    "AI 接口错误 ({}): {}",
                    status.as_u16(),
                    env.error.message
                ));
            }
            return Err(format!("AI 接口错误 ({}): {}", status.as_u16(), text));
        }
        let parsed: Value = serde_json::from_str(&text)
            .map_err(|e| format!("解析响应失败: {e}: {}", truncate(&text, 200)))?;
        if let Some(err) = parsed.get("error") {
            return Err(format!("AI 错误: {}", err));
        }
        serde_json::from_value::<ChatResponse>(parsed)
            .map_err(|e| format!("反序列化失败: {e}"))
    }

    pub fn config(&self) -> &AiConfig {
        &self.cfg
    }

    pub fn system_prompt(&self) -> String {
        self.cfg
            .system_prompt
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| {
                "你是一个经验丰富的 Python/Go/TypeScript 教学助手,擅长用简洁的中文解释代码错误并给出修复建议。回答要直接、准确,优先指出根因再给示例。".to_string()
            })
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(n).collect();
        out.push('…');
        out
    }
}

pub fn build_messages(system: &str, user: &str) -> Vec<ChatMessage> {
    vec![
        ChatMessage {
            role: "system".into(),
            content: system.to_string(),
        },
        ChatMessage {
            role: "user".into(),
            content: user.to_string(),
        },
    ]
}

pub fn extract_content(resp: &ChatResponse) -> String {
    resp.choices
        .first()
        .map(|c| c.message.content.clone())
        .unwrap_or_default()
}

pub fn usage_summary(resp: &ChatResponse) -> Value {
    json!({
        "prompt_tokens": resp.usage.as_ref().map(|u| u.prompt_tokens).unwrap_or(0),
        "completion_tokens": resp.usage.as_ref().map(|u| u.completion_tokens).unwrap_or(0),
        "total_tokens": resp.usage.as_ref().map(|u| u.total_tokens).unwrap_or(0),
        "model": resp.model,
    })
}
