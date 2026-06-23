use std::sync::Arc;

use serde_json::json;
use tokio::sync::Mutex;

use crate::ai::client::{self, Client};
use crate::history::{HistoryStore, RunRecord};
use crate::settings::Settings;

const EXPLANATION_PROMPT: &str = r#"以下是用户运行的 Python 代码,以及它执行时产生的错误信息。请用中文回答:
1. 用 1-2 句话直接指出错误根因
2. 给出最可能的修复方案(代码片段优先)
3. 如果错误信息不全,指出还缺什么信息

只输出解释本身,不要复述错误日志原文。"#;

pub struct AiService {
    settings: Arc<Mutex<Settings>>,
    history: Arc<Mutex<HistoryStore>>,
}

impl AiService {
    pub fn new(settings: Arc<Mutex<Settings>>, history: Arc<Mutex<HistoryStore>>) -> Self {
        Self { settings, history }
    }

    pub async fn snapshot_client(&self) -> Client {
        let cfg = self.settings.lock().await.ai.clone();
        Client::new(cfg)
    }

    pub async fn explain_run(&self, run_id: &str) -> Result<ExplainResult, String> {
        let rec = {
            let h = self.history.lock().await;
            h.get(run_id).await
        };
        let rec = rec.ok_or_else(|| format!("未找到运行记录: {run_id}"))?;
        if let Some(existing) = &rec.ai_explanation {
            if !existing.is_empty() {
                return Ok(ExplainResult {
                    run_id: rec.id,
                    explanation: existing.clone(),
                    cached: true,
                });
            }
        }
        let client = self.snapshot_client().await;
        if !client.is_configured() {
            return Err("AI 未启用或未配置 api_key".to_string());
        }
        let prompt = build_explain_prompt(&rec);
        let messages = client::build_messages(&client.system_prompt(), &prompt);
        let resp = client.chat(messages).await?;
        let content = client::extract_content(&resp).trim().to_string();
        if content.is_empty() {
            return Err("AI 返回内容为空".to_string());
        }
        {
            let h = self.history.lock().await;
            h.attach_ai_explanation(&rec.id, &content).await;
        }
        Ok(ExplainResult {
            run_id: rec.id,
            explanation: content,
            cached: false,
        })
    }

    pub async fn explain_text(
        &self,
        code: &str,
        error: &str,
    ) -> Result<ExplainResult, String> {
        let client = self.snapshot_client().await;
        if !client.is_configured() {
            return Err("AI 未启用或未配置 api_key".to_string());
        }
        let user = format!(
            "```python\n{}\n```\n\n错误信息:\n```\n{}\n```",
            truncate(code, 4000),
            truncate(error, 2000)
        );
        let messages =
            client::build_messages(&format!("{EXPLANATION_PROMPT}\n\n{}", client.system_prompt()), &user);
        let resp = client.chat(messages).await?;
        let content = client::extract_content(&resp).trim().to_string();
        if content.is_empty() {
            return Err("AI 返回内容为空".to_string());
        }
        Ok(ExplainResult {
            run_id: String::new(),
            explanation: content,
            cached: false,
        })
    }

    pub async fn status(&self) -> serde_json::Value {
        let cfg = self.settings.lock().await.ai.clone();
        let client = Client::new(cfg.clone());
        json!({
            "enabled": cfg.enabled,
            "configured": client.is_configured(),
            "base_url": cfg.effective_base_url(),
            "model": cfg.effective_model(),
            "auto_explain_on_error": cfg.auto_explain_on_error,
            "timeout_secs": cfg.timeout_secs,
            "max_tokens": cfg.max_tokens,
        })
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ExplainResult {
    pub run_id: String,
    pub explanation: String,
    pub cached: bool,
}

fn build_explain_prompt(rec: &RunRecord) -> String {
    let code = rec.code.clone();
    let output = rec.output.clone();
    let header = format!(
        "运行 ID: {}\n时间戳: {}\n是否成功: {}\n退出码: {:?}\n运行时长: {} ms\n",
        rec.id, rec.timestamp, rec.success, rec.exit_code, rec.duration
    );
    format!(
        "{EXPLANATION_PROMPT}\n\n{header}\n```python\n{}\n```\n\n运行输出 / 错误:\n```\n{}\n```",
        truncate(&code, 4000),
        truncate(&output, 2000)
    )
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
