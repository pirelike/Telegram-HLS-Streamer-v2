use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::Mutex;
use tokio::time::Duration;

use crate::config::BotConfig;
mod download;
mod errors;
mod upload;

pub use download::{get_file_bytes, get_file_response};
use errors::{api_url, normalize_reqwest_error, normalize_telegram_api_error};
pub use upload::upload_document;

pub const DEFAULT_API_BASE: &str = "https://api.telegram.org";
const MAX_ATTEMPTS: usize = 3;
const CONSECUTIVE_FAILURE_THRESHOLD: u32 = 3;

fn jittered_backoff(attempt: usize) -> Duration {
    let base = 2_u64.pow(attempt as u32);
    let base_ms = base.saturating_mul(1000);
    let jitter_ms = rand::random::<u64>() % base_ms.max(1);
    Duration::from_millis(base_ms.saturating_add(jitter_ms))
}

#[derive(Debug, Clone)]
pub struct UploadedFile {
    pub segment_key: String,
    pub file_id: String,
    pub bot_index: i64,
    pub file_size: u64,
    pub encryption_nonce: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelegramHealthResult {
    pub index: usize,
    pub channel_id: i64,
    pub ok: bool,
    pub error: Option<String>,
}

#[derive(Debug, Default)]
pub struct TelegramRuntime {
    upload_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    metrics: Mutex<TelegramMetrics>,
}

impl TelegramRuntime {
    pub fn new() -> Self {
        Self::default()
    }

    async fn upload_lock(&self, token: &str) -> Arc<Mutex<()>> {
        let mut locks = self.upload_locks.lock().await;
        locks
            .entry(token.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    pub async fn metrics_snapshot(&self) -> TelegramMetrics {
        self.metrics.lock().await.clone()
    }

    async fn record_upload_success(&self, bot_index: i64, bytes: u64, seconds: f64) {
        let mut metrics = self.metrics.lock().await;
        metrics.upload_count += 1;
        metrics.upload_total_seconds += seconds;
        let bot = metrics.per_bot.entry(bot_index).or_default();
        bot.upload_count += 1;
        bot.upload_bytes += bytes;
    }

    async fn record_upload_error(&self, bot_index: i64) {
        let mut metrics = self.metrics.lock().await;
        metrics.upload_errors += 1;
        metrics.per_bot.entry(bot_index).or_default().upload_errors += 1;
    }

    pub(crate) async fn record_download_success(&self, bot_index: i64, bytes: u64, seconds: f64) {
        let mut metrics = self.metrics.lock().await;
        metrics.download_count += 1;
        metrics.download_total_seconds += seconds;
        let bot = metrics.per_bot.entry(bot_index).or_default();
        bot.download_count += 1;
        bot.download_bytes += bytes;
    }

    pub(crate) async fn record_download_error(&self, bot_index: i64) {
        let mut metrics = self.metrics.lock().await;
        metrics.download_errors += 1;
        metrics
            .per_bot
            .entry(bot_index)
            .or_default()
            .download_errors += 1;
    }

    pub(crate) async fn record_consecutive_failure(&self, bot_index: i64) {
        let mut metrics = self.metrics.lock().await;
        metrics
            .per_bot
            .entry(bot_index)
            .or_default()
            .consecutive_failures += 1;
    }

    pub(crate) async fn reset_consecutive_failures(&self, bot_index: i64) {
        let mut metrics = self.metrics.lock().await;
        if let Some(bot) = metrics.per_bot.get_mut(&bot_index) {
            bot.consecutive_failures = 0;
        }
    }

    pub async fn is_bot_healthy(&self, bot_index: i64) -> bool {
        let metrics = self.metrics.lock().await;
        metrics
            .per_bot
            .get(&bot_index)
            .map(|b| b.consecutive_failures < CONSECUTIVE_FAILURE_THRESHOLD)
            .unwrap_or(true) // unknown bot = healthy
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct TelegramMetrics {
    pub upload_count: u64,
    pub upload_errors: u64,
    pub upload_total_seconds: f64,
    pub download_count: u64,
    pub download_errors: u64,
    pub download_total_seconds: f64,
    pub per_bot: HashMap<i64, PerBotMetrics>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct PerBotMetrics {
    pub upload_count: u64,
    pub upload_bytes: u64,
    pub upload_errors: u64,
    pub download_count: u64,
    pub download_bytes: u64,
    pub download_errors: u64,
    pub consecutive_failures: u32,
}

pub async fn probe_bot(
    client: &reqwest::Client,
    base_url: &str,
    index: usize,
    bot: &BotConfig,
) -> TelegramHealthResult {
    let result = client
        .post(api_url(base_url, &bot.token, "getChat"))
        .json(&json!({ "chat_id": bot.channel_id }))
        .send()
        .await;
    match result {
        Ok(resp) => {
            let status = resp.status();
            match resp.json::<Value>().await {
                Ok(body)
                    if status.is_success()
                        && body.get("ok").and_then(Value::as_bool) == Some(true) =>
                {
                    TelegramHealthResult {
                        index,
                        channel_id: bot.channel_id,
                        ok: true,
                        error: None,
                    }
                }
                Ok(body) => TelegramHealthResult {
                    index,
                    channel_id: bot.channel_id,
                    ok: false,
                    error: Some(normalize_telegram_api_error(status.as_u16(), &body)),
                },
                Err(e) => TelegramHealthResult {
                    index,
                    channel_id: bot.channel_id,
                    ok: false,
                    error: Some(normalize_reqwest_error(&e)),
                },
            }
        }
        Err(e) => TelegramHealthResult {
            index,
            channel_id: bot.channel_id,
            ok: false,
            error: Some(normalize_reqwest_error(&e)),
        },
    }
}

#[cfg(test)]
mod tests;
