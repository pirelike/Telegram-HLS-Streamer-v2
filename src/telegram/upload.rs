use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{anyhow, bail, Context, Result};
use tokio::time::sleep;

use crate::config::BotConfig;
use crate::crypto::EncryptionKey;

use super::errors::TelegramError;
use super::*;

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
        match super::download::send_document_attempt(
            client,
            base_url,
            &bot,
            path,
            filename,
            upload_size,
        )
        .await
        {
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
