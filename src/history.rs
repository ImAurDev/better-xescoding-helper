use serde::{Deserialize, Serialize};

use crate::config::{cache_dir, history_file, MAX_HISTORY_RECORDS};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunRecord {
    pub id: String,
    pub timestamp: i64,
    pub code: String,
    pub output: String,
    pub has_go_blocks: bool,
    pub success: bool,
    pub duration: i64,
}

pub struct HistoryStore {
    records: Vec<RunRecord>,
}

impl HistoryStore {
    pub fn new() -> Self {
        Self {
            records: Vec::new(),
        }
    }

    pub async fn init(&mut self) {
        let dir = cache_dir();
        if !dir.exists() {
            if let Err(e) = tokio::fs::create_dir_all(&dir).await {
                tracing::error!("加载历史记录失败: {e}");
            }
        }
        let file = history_file();
        if !file.exists() {
            return;
        }
        match tokio::fs::read_to_string(&file).await {
            Ok(content) if !content.trim().is_empty() => {
                match serde_json::from_str::<Vec<RunRecord>>(&content) {
                    Ok(recs) => self.records = recs,
                    Err(e) => {
                        tracing::error!("加载历史记录失败: {e}");
                        self.records.clear();
                    }
                }
            }
            _ => {}
        }
    }

    async fn save(&mut self) {
        if self.records.len() > MAX_HISTORY_RECORDS {
            let start = self.records.len() - MAX_HISTORY_RECORDS;
            self.records = self.records[start..].to_vec();
        }
        let file = history_file();
        if let Some(parent) = file.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
        let data = serde_json::to_string_pretty(&self.records).unwrap_or_default();
        if let Err(e) = tokio::fs::write(&file, data).await {
            tracing::error!("保存历史记录失败: {e}");
        }
    }

    pub async fn add(&mut self, record: RunRecord) {
        self.records.push(record);
        if self.records.len() > MAX_HISTORY_RECORDS {
            let start = self.records.len() - MAX_HISTORY_RECORDS;
            self.records = self.records[start..].to_vec();
        }
        self.save().await;
    }

    pub fn list(&self) -> Vec<RunRecord> {
        self.records.iter().rev().cloned().collect()
    }

    pub async fn clear(&mut self) {
        self.records.clear();
        self.save().await;
    }

    pub async fn delete(&mut self, id: &str) -> bool {
        let idx = self.records.iter().position(|r| r.id == id);
        if let Some(i) = idx {
            self.records.remove(i);
            self.save().await;
            true
        } else {
            false
        }
    }
}

pub fn now_millis() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp() * 1000
        + (time::OffsetDateTime::now_utc().nanosecond() as i64 / 1_000_000)
}

pub fn gen_id() -> String {
    use rand::RngExt;
    let ts = now_millis();
    let rand_part: String = rand::rng()
        .sample_iter(&rand::distr::Alphanumeric)
        .take(6)
        .map(|b| (b as char).to_ascii_lowercase())
        .collect();
    format!("{ts}_{rand_part}")
}
