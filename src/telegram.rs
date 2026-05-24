use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use anyhow::{anyhow, bail, Context, Result};
use reqwest::multipart::{Form, Part};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::fs::File;
use tokio::sync::Mutex;
use tokio::time::{sleep, Duration};

use crate::config::BotConfig;
use crate::crypto::EncryptionKey;

pub const DEFAULT_API_BASE: &str = "https://api.telegram.org";
const MAX_ATTEMPTS: usize = 3;
const CONSECUTIVE_FAILURE_THRESHOLD: u32 = 3;

fn jittered_backoff(attempt: usize) -> Duration {
    let base = 2_u64.pow(attempt as u32);
    let half = base.saturating_sub(1) / 2;
    let jitter = rand::random::<u64>() % (half.saturating_add(1));
    Duration::from_secs(base.saturating_add(jitter))
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

pub async fn upload_document(
    client: &reqwest::Client,
    runtime: &TelegramRuntime,
    base_url: &str,
    bot: BotConfig,
    bot_index: i64,
    path: &Path,
    segment_key: String,
    encryption_key: Option<&EncryptionKey>,
    // max_file_size is typically cfg.telegram_max_file_size — user-configurable;
    // raise if Telegram increases Bot API limits.
    max_file_size: u64,
) -> Result<UploadedFile> {
    let plaintext_size = tokio::fs::metadata(path)
        .await
        .with_context(|| format!("stat {}", path.display()))?
        .len();
    let prepared =
        prepare_upload_payload(path, &segment_key, encryption_key, plaintext_size).await?;
    if prepared.upload_size > max_file_size {
        runtime.record_upload_error(bot_index).await;
        let upload_size = prepared.upload_size;
        let _ = prepared.cleanup().await;
        bail!(
            "telegram_file_too_large: {} is {} bytes, max is {}",
            path.display(),
            upload_size,
            max_file_size
        );
    }
    let upload_result = upload_prepared_document(
        client,
        runtime,
        base_url,
        bot,
        bot_index,
        &prepared.path,
        &prepared.filename,
        segment_key,
        plaintext_size,
        prepared.upload_size,
        prepared.encryption_nonce.clone(),
    )
    .await;
    let cleanup_result = prepared.cleanup().await;
    if let Err(e) = cleanup_result {
        tracing::warn!(error = %e, "failed to remove encrypted upload staging file");
    }
    upload_result
}

struct PreparedUpload {
    path: PathBuf,
    filename: String,
    upload_size: u64,
    encryption_nonce: Option<String>,
    cleanup_path: Option<PathBuf>,
}

impl PreparedUpload {
    async fn cleanup(self) -> Result<()> {
        if let Some(path) = self.cleanup_path {
            match tokio::fs::remove_file(&path).await {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e).with_context(|| format!("removing {}", path.display())),
            }
        }
        Ok(())
    }
}

async fn prepare_upload_payload(
    path: &Path,
    segment_key: &str,
    encryption_key: Option<&EncryptionKey>,
    plaintext_size: u64,
) -> Result<PreparedUpload> {
    let Some(key) = encryption_key else {
        let filename = path
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| anyhow!("invalid upload filename: {}", path.display()))?
            .to_string();
        return Ok(PreparedUpload {
            path: path.to_path_buf(),
            filename,
            upload_size: plaintext_size,
            encryption_nonce: None,
            cleanup_path: None,
        });
    };

    let plaintext = tokio::fs::read(path)
        .await
        .with_context(|| format!("read upload file {}", path.display()))?;
    let encrypted = key.encrypt(&plaintext, segment_key)?;
    let filename = format!("{}.dat", uuid::Uuid::new_v4().simple());
    let staged_path = std::env::temp_dir().join(format!("thls-upload-{filename}"));
    tokio::fs::write(&staged_path, &encrypted.ciphertext)
        .await
        .with_context(|| format!("write encrypted upload {}", staged_path.display()))?;
    Ok(PreparedUpload {
        path: staged_path.clone(),
        filename,
        upload_size: encrypted.ciphertext.len() as u64,
        encryption_nonce: Some(encrypted.nonce_hex),
        cleanup_path: Some(staged_path),
    })
}

async fn upload_prepared_document(
    client: &reqwest::Client,
    runtime: &TelegramRuntime,
    base_url: &str,
    bot: BotConfig,
    bot_index: i64,
    path: &Path,
    filename: &str,
    segment_key: String,
    plaintext_size: u64,
    upload_size: u64,
    encryption_nonce: Option<String>,
) -> Result<UploadedFile> {
    let lock = runtime.upload_lock(&bot.token).await;
    let mut guard = lock.lock().await;
    let started = Instant::now();
    tracing::info!(
        segment_key = %segment_key,
        bot_index,
        file_size = plaintext_size,
        upload_size,
        filename = %filename,
        "telegram upload started"
    );

    for attempt in 0..MAX_ATTEMPTS {
        match send_document_attempt(client, base_url, &bot, path, filename, upload_size).await {
            Ok((file_id, remote_size)) => {
                if remote_size != upload_size {
                    runtime.record_upload_error(bot_index).await;
                    runtime.record_consecutive_failure(bot_index).await;
                    bail!(
                        "upload_integrity_mismatch: {} local={} telegram={}",
                        segment_key,
                        upload_size,
                        remote_size
                    );
                }
                runtime
                    .record_upload_success(bot_index, upload_size, started.elapsed().as_secs_f64())
                    .await;
                runtime.reset_consecutive_failures(bot_index).await;
                tracing::info!(
                    segment_key = %segment_key,
                    bot_index,
                    file_size = plaintext_size,
                    upload_size,
                    elapsed_ms = started.elapsed().as_millis(),
                    "telegram upload complete"
                );
                return Ok(UploadedFile {
                    segment_key,
                    file_id,
                    bot_index,
                    file_size: plaintext_size,
                    encryption_nonce,
                });
            }
            Err(TelegramError::Permanent(e)) => {
                runtime.record_upload_error(bot_index).await;
                runtime.record_consecutive_failure(bot_index).await;
                tracing::warn!(
                    segment_key = %segment_key,
                    bot_index,
                    error = %e,
                    "telegram upload permanent failure"
                );
                return Err(e);
            }
            Err(TelegramError::RetryAfter(wait)) if attempt + 1 < MAX_ATTEMPTS => {
                tracing::warn!(
                    segment_key = %segment_key,
                    bot_index,
                    attempt = attempt + 1,
                    wait_seconds = wait.as_secs(),
                    "telegram upload rate limited; retrying"
                );
                drop(guard);
                sleep(wait).await;
                guard = lock.lock().await;
            }
            Err(TelegramError::Retryable(e)) if attempt + 1 < MAX_ATTEMPTS => {
                tracing::warn!(
                    error = %e,
                    attempt = attempt + 1,
                    segment_key = %segment_key,
                    bot_index,
                    "Telegram upload attempt failed; retrying"
                );
                drop(guard);
                sleep(jittered_backoff(attempt)).await;
                guard = lock.lock().await;
            }
            Err(e) => {
                runtime.record_upload_error(bot_index).await;
                runtime.record_consecutive_failure(bot_index).await;
                let err = e.into_anyhow();
                tracing::warn!(
                    segment_key = %segment_key,
                    bot_index,
                    error = %err,
                    "telegram upload failed"
                );
                return Err(err);
            }
        }
    }
    unreachable!("retry loop always returns")
}

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

async fn send_document_attempt(
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
                let resp = client
                    .get(&url)
                    .send()
                    .await
                    .map_err(|e| anyhow!("download request failed: {e}"))?;
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

fn api_url(base_url: &str, token: &str, method: &str) -> String {
    format!("{}/bot{token}/{method}", base_url.trim_end_matches('/'))
}

fn file_url(base_url: &str, token: &str, file_path: &str) -> String {
    format!(
        "{}/file/bot{token}/{}",
        base_url.trim_end_matches('/'),
        file_path.trim_start_matches('/')
    )
}

fn validate_file_id(file_id: &str) -> Result<()> {
    if file_id.len() < 5 {
        tracing::warn!(
            len = file_id.len(),
            "validate_file_id called with suspiciously short file_id"
        );
    }
    if (50..=255).contains(&file_id.len())
        && file_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        Ok(())
    } else {
        bail!("invalid Telegram file_id (expected 50–255 chars of A-Za-z0-9/_-)")
    }
}

fn classify_reqwest(e: reqwest::Error) -> TelegramError {
    if e.is_timeout() || e.is_connect() || e.is_request() {
        TelegramError::Retryable(anyhow!(e))
    } else {
        TelegramError::Permanent(anyhow!(e))
    }
}

fn classify_api_body(status: u16, body: &Value) -> TelegramError {
    let code = body
        .get("error_code")
        .and_then(Value::as_u64)
        .unwrap_or(status as u64);
    if code == 429 {
        let retry_after = body
            .pointer("/parameters/retry_after")
            .and_then(Value::as_u64)
            .unwrap_or(1);
        return TelegramError::RetryAfter(Duration::from_secs(retry_after));
    }
    if code == 400 || code == 403 {
        return TelegramError::Permanent(anyhow!(normalize_telegram_api_error(status, body)));
    }
    TelegramError::Retryable(anyhow!(normalize_telegram_api_error(status, body)))
}

fn normalize_telegram_api_error(status: u16, body: &Value) -> String {
    if status == 403 {
        return "forbidden".into();
    }
    if status == 429 {
        if let Some(retry_after) = body
            .pointer("/parameters/retry_after")
            .and_then(Value::as_i64)
        {
            return format!("rate_limited:{retry_after}");
        }
        return "rate_limited".into();
    }
    let description = body
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("telegram api error");
    format!("telegram_api: {}", truncate(description, 120))
}

fn normalize_reqwest_error(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        "timeout".into()
    } else if e.is_connect() || e.is_request() {
        "network_error".into()
    } else {
        format!("reqwest: {}", truncate(&e.to_string(), 120))
    }
}

fn truncate(value: &str, limit: usize) -> String {
    if value.len() <= limit {
        value.to_string()
    } else {
        let mut boundary = limit;
        while boundary > 0 && !value.is_char_boundary(boundary) {
            boundary -= 1;
        }
        value[..boundary].to_string()
    }
}

#[derive(Debug)]
enum TelegramError {
    Retryable(anyhow::Error),
    RetryAfter(Duration),
    Permanent(anyhow::Error),
}

impl TelegramError {
    fn into_anyhow(self) -> anyhow::Error {
        match self {
            Self::Retryable(e) | Self::Permanent(e) => e,
            Self::RetryAfter(wait) => anyhow!("telegram rate limited after retries: {wait:?}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use axum::body::Bytes;
    use axum::extract::State;
    use axum::http::{Method, StatusCode, Uri};
    use axum::response::{IntoResponse, Response};
    use axum::routing::any;
    use axum::Router;
    use serde_json::json;
    use tokio::sync::Mutex;

    use crate::config::{BotConfig, BotSource};

    #[derive(Debug)]
    struct FakeTelegram {
        send_attempts: AtomicUsize,
        upload_size: u64,
        mismatch: bool,
        rate_limit_first: bool,
        bad_request: bool,
        forbidden: bool,
        download_bytes: Vec<u8>,
        upload_bodies: Mutex<Vec<Vec<u8>>>,
    }

    async fn fake_handler(
        State(fake): State<Arc<FakeTelegram>>,
        method: Method,
        uri: Uri,
        body: Bytes,
    ) -> Response {
        let path = uri.path();
        if method == Method::POST && path.ends_with("/sendDocument") {
            fake.upload_bodies.lock().await.push(body.to_vec());
            let attempt = fake.send_attempts.fetch_add(1, Ordering::SeqCst);
            if fake.rate_limit_first && attempt == 0 {
                return (
                    StatusCode::TOO_MANY_REQUESTS,
                    axum::Json(json!({
                        "ok": false,
                        "description": "Too Many Requests",
                        "parameters": { "retry_after": 0 }
                    })),
                )
                    .into_response();
            }
            if fake.bad_request {
                return (
                    StatusCode::BAD_REQUEST,
                    axum::Json(json!({
                        "ok": false,
                        "description": "Bad Request: rejected"
                    })),
                )
                    .into_response();
            }
            if fake.forbidden {
                return (
                    StatusCode::FORBIDDEN,
                    axum::Json(json!({
                        "ok": false,
                        "description": "Forbidden: bot was blocked"
                    })),
                )
                    .into_response();
            }
            let remote_size = if fake.mismatch {
                fake.upload_size + 1
            } else {
                fake.upload_size
            };
            return axum::Json(json!({
                "ok": true,
                "result": {
                    "document": {
                        "file_id": long_file_id(),
                        "file_size": remote_size
                    }
                }
            }))
            .into_response();
        }
        if method == Method::POST && path.ends_with("/getFile") {
            return axum::Json(json!({
                "ok": true,
                "result": { "file_path": "payload.bin" }
            }))
            .into_response();
        }
        if method == Method::GET && path.contains("/file/bot") {
            return fake.download_bytes.clone().into_response();
        }
        StatusCode::NOT_FOUND.into_response()
    }

    async fn fake_server(fake: FakeTelegram) -> (String, Arc<FakeTelegram>) {
        let fake = Arc::new(fake);
        let app = Router::new()
            .route("/*path", any(fake_handler))
            .with_state(fake.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{addr}"), fake)
    }

    fn bot() -> BotConfig {
        BotConfig {
            token: "12345678:abcdefghijklmnopqrstuvwxyzABCDEFGHI".into(),
            channel_id: -100,
            source: BotSource::Db,
            db_id: None,
            label: String::new(),
        }
    }

    fn long_file_id() -> String {
        "A".repeat(50)
    }

    async fn temp_file(bytes: &[u8]) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "thls_telegram_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        tokio::fs::write(&path, bytes).await.unwrap();
        path
    }

    #[tokio::test]
    async fn upload_success_records_metrics() {
        let path = temp_file(b"abcdef").await;
        let (base_url, fake) = fake_server(FakeTelegram {
            send_attempts: AtomicUsize::new(0),
            upload_size: 6,
            mismatch: false,
            rate_limit_first: false,
            bad_request: false,
            forbidden: false,
            download_bytes: Vec::new(),
            upload_bodies: Mutex::new(Vec::new()),
        })
        .await;
        let runtime = TelegramRuntime::new();
        let uploaded = upload_document(
            &reqwest::Client::new(),
            &runtime,
            &base_url,
            bot(),
            2,
            &path,
            "video_0/video_0001.m4s".into(),
            None,
            20,
        )
        .await
        .unwrap();

        assert_eq!(uploaded.file_size, 6);
        assert_eq!(uploaded.bot_index, 2);
        assert_eq!(fake.send_attempts.load(Ordering::SeqCst), 1);
        let metrics = runtime.metrics_snapshot().await;
        assert_eq!(metrics.upload_count, 1);
        assert_eq!(metrics.per_bot.get(&2).unwrap().upload_bytes, 6);
        let _ = tokio::fs::remove_file(path).await;
    }

    #[tokio::test]
    async fn encrypted_upload_uses_random_dat_filename_and_records_plain_size() {
        let path = temp_file(b"abcdef").await;
        let (base_url, fake) = fake_server(FakeTelegram {
            send_attempts: AtomicUsize::new(0),
            upload_size: 22,
            mismatch: false,
            rate_limit_first: false,
            bad_request: false,
            forbidden: false,
            download_bytes: Vec::new(),
            upload_bodies: Mutex::new(Vec::new()),
        })
        .await;
        let runtime = TelegramRuntime::new();
        let key =
            crate::crypto::EncryptionKey::from_hex(&"33".repeat(crate::crypto::KEY_LEN)).unwrap();
        let uploaded = upload_document(
            &reqwest::Client::new(),
            &runtime,
            &base_url,
            bot(),
            0,
            &path,
            "video_0/video_0001.m4s".into(),
            Some(&key),
            22,
        )
        .await
        .unwrap();

        assert_eq!(uploaded.file_size, 6);
        assert!(uploaded.encryption_nonce.is_some());
        let metrics = runtime.metrics_snapshot().await;
        assert_eq!(metrics.per_bot.get(&0).unwrap().upload_bytes, 22);
        let bodies = fake.upload_bodies.lock().await;
        let body = String::from_utf8_lossy(&bodies[0]);
        assert!(body.contains(".dat\""));
        assert!(!body.contains("video_0001.m4s"));
        assert!(!body.as_bytes().windows(6).any(|w| w == b"abcdef"));
        let _ = tokio::fs::remove_file(path).await;
    }

    #[tokio::test]
    async fn oversized_upload_fails_before_contacting_telegram() {
        let path = temp_file(b"abcdef").await;
        let (base_url, fake) = fake_server(FakeTelegram {
            send_attempts: AtomicUsize::new(0),
            upload_size: 6,
            mismatch: false,
            rate_limit_first: false,
            bad_request: false,
            forbidden: false,
            download_bytes: Vec::new(),
            upload_bodies: Mutex::new(Vec::new()),
        })
        .await;
        let runtime = TelegramRuntime::new();
        let err = upload_document(
            &reqwest::Client::new(),
            &runtime,
            &base_url,
            bot(),
            0,
            &path,
            "video_0/video_0001.m4s".into(),
            None,
            5,
        )
        .await
        .unwrap_err();

        assert!(err.to_string().contains("telegram_file_too_large"));
        assert_eq!(fake.send_attempts.load(Ordering::SeqCst), 0);
        assert_eq!(runtime.metrics_snapshot().await.upload_errors, 1);
        let _ = tokio::fs::remove_file(path).await;
    }

    #[tokio::test]
    async fn integrity_mismatch_fails_after_upload_response() {
        let path = temp_file(b"abcdef").await;
        let (base_url, _fake) = fake_server(FakeTelegram {
            send_attempts: AtomicUsize::new(0),
            upload_size: 6,
            mismatch: true,
            rate_limit_first: false,
            bad_request: false,
            forbidden: false,
            download_bytes: Vec::new(),
            upload_bodies: Mutex::new(Vec::new()),
        })
        .await;
        let runtime = TelegramRuntime::new();
        let err = upload_document(
            &reqwest::Client::new(),
            &runtime,
            &base_url,
            bot(),
            0,
            &path,
            "video_0/video_0001.m4s".into(),
            None,
            20,
        )
        .await
        .unwrap_err();

        assert!(err.to_string().contains("upload_integrity_mismatch"));
        assert_eq!(runtime.metrics_snapshot().await.upload_errors, 1);
        let _ = tokio::fs::remove_file(path).await;
    }

    #[tokio::test]
    async fn retry_after_retries_but_bad_request_does_not() {
        let path = temp_file(b"abcdef").await;
        let (base_url, fake) = fake_server(FakeTelegram {
            send_attempts: AtomicUsize::new(0),
            upload_size: 6,
            mismatch: false,
            rate_limit_first: true,
            bad_request: false,
            forbidden: false,
            download_bytes: Vec::new(),
            upload_bodies: Mutex::new(Vec::new()),
        })
        .await;
        let runtime = TelegramRuntime::new();
        upload_document(
            &reqwest::Client::new(),
            &runtime,
            &base_url,
            bot(),
            0,
            &path,
            "video_0/video_0001.m4s".into(),
            None,
            20,
        )
        .await
        .unwrap();
        assert_eq!(fake.send_attempts.load(Ordering::SeqCst), 2);

        let (base_url, fake) = fake_server(FakeTelegram {
            send_attempts: AtomicUsize::new(0),
            upload_size: 6,
            mismatch: false,
            rate_limit_first: false,
            bad_request: true,
            forbidden: false,
            download_bytes: Vec::new(),
            upload_bodies: Mutex::new(Vec::new()),
        })
        .await;
        let runtime = TelegramRuntime::new();
        let err = upload_document(
            &reqwest::Client::new(),
            &runtime,
            &base_url,
            bot(),
            0,
            &path,
            "video_0/video_0001.m4s".into(),
            None,
            20,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("Bad Request"));
        assert_eq!(fake.send_attempts.load(Ordering::SeqCst), 1);
        let _ = tokio::fs::remove_file(path).await;
    }

    #[tokio::test]
    async fn get_file_bytes_uses_configured_bot_index() {
        let (base_url, _fake) = fake_server(FakeTelegram {
            send_attempts: AtomicUsize::new(0),
            upload_size: 0,
            mismatch: false,
            rate_limit_first: false,
            bad_request: false,
            forbidden: false,
            download_bytes: b"payload".to_vec(),
            upload_bodies: Mutex::new(Vec::new()),
        })
        .await;
        let runtime = TelegramRuntime::new();
        let bytes = get_file_bytes(
            &reqwest::Client::new(),
            &runtime,
            &base_url,
            &[bot()],
            &long_file_id(),
            0,
        )
        .await
        .unwrap();

        assert_eq!(bytes, b"payload");
        let metrics = runtime.metrics_snapshot().await;
        assert_eq!(metrics.download_count, 1);
        assert_eq!(metrics.per_bot.get(&0).unwrap().download_bytes, 7);
    }

    #[tokio::test]
    async fn retry_matrix_forbidden_is_not_retried() {
        let path = temp_file(b"abcdef").await;
        let (base_url, fake) = fake_server(FakeTelegram {
            send_attempts: AtomicUsize::new(0),
            upload_size: 6,
            mismatch: false,
            rate_limit_first: false,
            bad_request: false,
            forbidden: true,
            download_bytes: Vec::new(),
            upload_bodies: Mutex::new(Vec::new()),
        })
        .await;
        let runtime = TelegramRuntime::new();
        let err = upload_document(
            &reqwest::Client::new(),
            &runtime,
            &base_url,
            bot(),
            0,
            &path,
            "video_0/video_0001.m4s".into(),
            None,
            20,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("forbidden"));
        // Forbidden is permanent — only 1 attempt
        assert_eq!(fake.send_attempts.load(Ordering::SeqCst), 1);
        assert_eq!(runtime.metrics_snapshot().await.upload_errors, 1);
        let _ = tokio::fs::remove_file(path).await;
    }

    #[tokio::test]
    async fn bot_reload_preserves_in_flight_uploads() {
        // The TelegramRuntime's per-bot locks are keyed by token string.
        // When a bot is "reloaded" (new config), old locks persist for in-flight tasks.
        // This test verifies that upload_lock returns the same Arc for the same token
        // even when called from concurrent tasks.
        let runtime = TelegramRuntime::new();

        // Simulate two concurrent tasks acquiring the lock for the same bot token
        let lock1 = runtime.upload_lock("bot_token_A").await;
        let lock2 = runtime.upload_lock("bot_token_A").await;

        // Both should be the same Arc (pointer-equal)
        assert!(
            Arc::ptr_eq(&lock1, &lock2),
            "same token must yield the same lock Arc"
        );

        // Different token gets a different lock
        let lock3 = runtime.upload_lock("bot_token_B").await;
        assert!(
            !Arc::ptr_eq(&lock1, &lock3),
            "different tokens must yield different lock Arcs"
        );
    }

    #[tokio::test]
    async fn concurrent_upload_lock_requests_get_same_lock() {
        let runtime = Arc::new(TelegramRuntime::new());
        let mut handles = Vec::new();

        for _ in 0..10 {
            let rt = runtime.clone();
            handles.push(tokio::spawn(
                async move { rt.upload_lock("shared_token").await },
            ));
        }

        let mut results = Vec::new();
        for h in handles {
            results.push(h.await.unwrap());
        }
        let locks: Vec<Arc<Mutex<()>>> = results;

        // All 10 concurrent requests must return pointer-identical Arcs
        let first = &locks[0];
        for lock in &locks[1..] {
            assert!(
                Arc::ptr_eq(first, lock),
                "concurrent upload_lock calls must return the same Arc"
            );
        }
    }

    #[test]
    fn validate_file_id_rejects_short_input() {
        let err = validate_file_id("").unwrap_err();
        assert!(
            err.to_string().contains("invalid Telegram file_id"),
            "empty string should be rejected: {err}"
        );

        let err = validate_file_id("abc").unwrap_err();
        assert!(
            err.to_string().contains("invalid Telegram file_id"),
            "short string (3 chars) should be rejected: {err}"
        );

        let err = validate_file_id(&"a".repeat(49)).unwrap_err();
        assert!(
            err.to_string().contains("invalid Telegram file_id"),
            "49-char string should be rejected: {err}"
        );
    }

    #[test]
    fn validate_file_id_accepts_valid_ids() {
        assert!(validate_file_id(&"A".repeat(50)).is_ok());
        assert!(validate_file_id(&"aB3_z-".repeat(10)).is_ok());
        assert!(validate_file_id(&"A".repeat(255)).is_ok());
    }
}
