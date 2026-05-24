use std::path::Path;
use std::time::Instant;

use anyhow::{anyhow, Result};
use reqwest::multipart::{Form, Part};
use serde_json::{json, Value};
use tokio::fs::File;
use tokio::time::sleep;

use crate::config::BotConfig;

use super::errors::{
    api_url, classify_api_body, classify_reqwest, file_url, redact_bot_token, validate_file_id,
    TelegramError,
};
use super::*;

pub async fn get_file_bytes(
    client: &reqwest::Client,
    runtime: &TelegramRuntime,
    base_url: &str,
    bots: &[BotConfig],
    file_id: &str,
    bot_index: i64,
) -> Result<Vec<u8>> {
    validate_file_id(file_id)?;
    if bot_index < 0 {
        return Err(anyhow!("invalid bot index {bot_index}"));
    }
    let bot = bots
        .get(bot_index as usize)
        .ok_or_else(|| anyhow!("bot index {bot_index} is not configured"))?;
    let started = Instant::now();
    for attempt in 0..MAX_ATTEMPTS {
        match get_file_bytes_attempt(client, base_url, bot, file_id).await {
            Ok(bytes) => {
                runtime
                    .record_download_success(
                        bot_index,
                        bytes.len() as u64,
                        started.elapsed().as_secs_f64(),
                    )
                    .await;
                runtime.reset_consecutive_failures(bot_index).await;
                return Ok(bytes);
            }
            Err(TelegramError::Permanent(e)) => {
                runtime.record_download_error(bot_index).await;
                runtime.record_consecutive_failure(bot_index).await;
                return Err(e);
            }
            Err(TelegramError::RetryAfter(wait)) if attempt + 1 < MAX_ATTEMPTS => {
                sleep(wait).await;
            }
            Err(TelegramError::Retryable(e)) if attempt + 1 < MAX_ATTEMPTS => {
                tracing::warn!(
                    error = %e,
                    attempt = attempt + 1,
                    file_id = %file_id,
                    "Telegram download attempt failed; retrying"
                );
                sleep(jittered_backoff(attempt)).await;
            }
            Err(e) => {
                runtime.record_download_error(bot_index).await;
                runtime.record_consecutive_failure(bot_index).await;
                return Err(e.into_anyhow());
            }
        }
    }
    unreachable!("retry loop always returns")
}

pub(super) async fn send_document_attempt(
    client: &reqwest::Client,
    base_url: &str,
    bot: &BotConfig,
    path: &Path,
    filename: &str,
    file_size: u64,
) -> Result<(String, u64), TelegramError> {
    let file = File::open(path).await.map_err(|e| {
        TelegramError::Permanent(anyhow!("open upload file {}: {e}", path.display()))
    })?;
    let part = Part::stream_with_length(file, file_size).file_name(filename.to_string());
    let form = Form::new()
        .text("chat_id", bot.channel_id.to_string())
        .part("document", part);
    let resp = client
        .post(api_url(base_url, &bot.token, "sendDocument"))
        .multipart(form)
        .send()
        .await
        .map_err(classify_reqwest)?;
    let status = resp.status();
    let body = resp.json::<Value>().await.map_err(classify_reqwest)?;
    if !status.is_success() || body.get("ok").and_then(Value::as_bool) != Some(true) {
        return Err(classify_api_body(status.as_u16(), &body));
    }
    let doc = body.pointer("/result/document").ok_or_else(|| {
        TelegramError::Permanent(anyhow!("sendDocument response has no document"))
    })?;
    let file_id = doc
        .get("file_id")
        .and_then(Value::as_str)
        .ok_or_else(|| TelegramError::Permanent(anyhow!("sendDocument response has no file_id")))?
        .to_string();
    let file_size = doc
        .get("file_size")
        .and_then(Value::as_u64)
        .ok_or_else(|| {
            TelegramError::Permanent(anyhow!("sendDocument response has no file_size"))
        })?;
    Ok((file_id, file_size))
}

async fn resolve_file_url(
    client: &reqwest::Client,
    base_url: &str,
    bot: &BotConfig,
    file_id: &str,
) -> Result<String, TelegramError> {
    let resp = client
        .post(api_url(base_url, &bot.token, "getFile"))
        .json(&json!({ "file_id": file_id }))
        .send()
        .await
        .map_err(classify_reqwest)?;
    let status = resp.status();
    let body = resp.json::<Value>().await.map_err(classify_reqwest)?;
    if !status.is_success() || body.get("ok").and_then(Value::as_bool) != Some(true) {
        return Err(classify_api_body(status.as_u16(), &body));
    }
    let file_path = body
        .pointer("/result/file_path")
        .and_then(Value::as_str)
        .ok_or_else(|| TelegramError::Permanent(anyhow!("getFile response has no file_path")))?;
    Ok(file_url(base_url, &bot.token, file_path))
}

async fn get_file_bytes_attempt(
    client: &reqwest::Client,
    base_url: &str,
    bot: &BotConfig,
    file_id: &str,
) -> Result<Vec<u8>, TelegramError> {
    let url = resolve_file_url(client, base_url, bot, file_id).await?;
    let resp = client.get(&url).send().await.map_err(classify_reqwest)?;
    if !resp.status().is_success() {
        return Err(classify_api_body(resp.status().as_u16(), &json!({})));
    }
    resp.bytes()
        .await
        .map(|b| b.to_vec())
        .map_err(classify_reqwest)
}

/// Starts a streaming download for a segment. Returns the reqwest Response so the caller
/// can call `.bytes_stream()` for first-byte-latency streaming.
/// Only the `getFile` API call is retried; the actual download starts immediately.
pub async fn get_file_response(
    client: &reqwest::Client,
    runtime: &TelegramRuntime,
    base_url: &str,
    bots: &[BotConfig],
    file_id: &str,
    bot_index: i64,
) -> Result<reqwest::Response> {
    validate_file_id(file_id)?;
    if bot_index < 0 {
        return Err(anyhow!("invalid bot index {bot_index}"));
    }
    let bot = bots
        .get(bot_index as usize)
        .ok_or_else(|| anyhow!("bot index {bot_index} is not configured"))?;
    for attempt in 0..MAX_ATTEMPTS {
        match resolve_file_url(client, base_url, bot, file_id).await {
            Ok(url) => {
                let resp = client.get(&url).send().await.map_err(|e| {
                    anyhow!(
                        "download request failed: {}",
                        redact_bot_token(&e.to_string())
                    )
                })?;
                if !resp.status().is_success() {
                    return Err(anyhow!("download failed: status {}", resp.status()));
                }
                // Metrics are recorded by the caller after the full stream completes.
                runtime.reset_consecutive_failures(bot_index).await;
                return Ok(resp);
            }
            Err(TelegramError::Permanent(e)) => {
                runtime.record_download_error(bot_index).await;
                runtime.record_consecutive_failure(bot_index).await;
                return Err(e);
            }
            Err(TelegramError::RetryAfter(wait)) if attempt + 1 < MAX_ATTEMPTS => {
                sleep(wait).await;
            }
            Err(TelegramError::Retryable(e)) if attempt + 1 < MAX_ATTEMPTS => {
                tracing::warn!(
                    error = %e,
                    attempt = attempt + 1,
                    file_id = %file_id,
                    "getFile attempt failed; retrying"
                );
                sleep(jittered_backoff(attempt)).await;
            }
            Err(e) => {
                runtime.record_download_error(bot_index).await;
                runtime.record_consecutive_failure(bot_index).await;
                return Err(e.into_anyhow());
            }
        }
    }
    unreachable!("retry loop always returns")
}
