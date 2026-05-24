use std::sync::Arc;

use anyhow::{bail, Context, Result};
use tokio::task::JoinSet;
use tokio::time::{timeout, Duration};

use super::cache::{claim_inflight, finish_inflight, CacheEntry};
use super::AppState;
use crate::config::Config;
use crate::telegram;

use crate::db;

#[cfg(not(test))]
const SEGMENT_FETCH_TIMEOUT: Duration = Duration::from_secs(60);
#[cfg(test)]
const SEGMENT_FETCH_TIMEOUT: Duration = Duration::from_millis(100);

pub(super) fn segment_part_aad(segment_key: &str, part_index: i64) -> String {
    format!("{segment_key}/part_{part_index}")
}

pub(super) async fn fetch_real_with_singleflight(
    state: &Arc<AppState>,
    cfg: &Config,
    cache_key: &str,
    job_id: &str,
    file_id: &str,
    bot_index: i64,
    key: &str,
    encryption_nonce: Option<&str>,
) -> Result<CacheEntry> {
    let (inflight, is_leader) = claim_inflight(state, cache_key).await;
    if !is_leader {
        let outcome = inflight.wait_for_outcome().await;
        match outcome {
            Ok(Some(entry)) => return Ok(entry),
            Ok(None) => {
                if let Some(entry) = state.cache.get(cache_key).await {
                    return Ok(entry);
                }
                bail!("cache miss after single-flight wait");
            }
            Err(e) => bail!(e),
        }
    }
    let result = real_fetch_into_cache(
        state,
        cfg,
        cache_key,
        job_id,
        file_id,
        bot_index,
        key,
        encryption_nonce,
    )
    .await;
    finish_inflight(state, cache_key, inflight, &result).await;
    result
}

pub(super) async fn real_fetch_into_cache(
    state: &AppState,
    cfg: &Config,
    cache_key: &str,
    job_id: &str,
    file_id: &str,
    bot_index: i64,
    key: &str,
    encryption_nonce: Option<&str>,
) -> Result<CacheEntry> {
    let bytes =
        match fetch_file_bytes_with_timeout(state, cfg, file_id, bot_index, job_id, key).await {
            Ok(bytes) => bytes,
            Err(fetch_err) => {
                if let Some(recovered) = super::real_recovery::reupload_from_cache_or_source(
                    state, cfg, job_id, key, bot_index, &fetch_err,
                )
                .await
                {
                    return recovered;
                }
                return Err(fetch_err).with_context(|| format!("fetching {key} from Telegram"));
            }
        };
    let bytes = crate::crypto::decrypt_optional(
        cfg.telegram_encryption_key.as_ref(),
        encryption_nonce,
        key,
        bytes,
    )?;
    let entry = super::cache_entry_for_bytes(cfg, cache_key, key, bytes).await?;
    state
        .cache
        .insert(
            cache_key.to_string(),
            entry.clone(),
            Some((cfg.segment_cache_size_mb as u64) * 1024 * 1024),
        )
        .await;
    Ok(entry)
}

pub(super) async fn fetch_reconstructed_with_singleflight(
    state: &Arc<AppState>,
    cfg: &Config,
    cache_key: &str,
    key: &str,
    parts: &[db::SegmentPartLookup],
) -> Result<CacheEntry> {
    let (inflight, is_leader) = claim_inflight(state, cache_key).await;
    if !is_leader {
        let outcome = inflight.wait_for_outcome().await;
        match outcome {
            Ok(Some(entry)) => return Ok(entry),
            Ok(None) => {
                if let Some(entry) = state.cache.get(cache_key).await {
                    return Ok(entry);
                }
                bail!("cache miss after single-flight wait");
            }
            Err(e) => bail!(e),
        }
    }
    let result = reconstructed_fetch_into_cache(state, cfg, cache_key, key, parts).await;
    finish_inflight(state, cache_key, inflight, &result).await;
    result
}

pub(super) async fn reconstructed_fetch_into_cache(
    state: &Arc<AppState>,
    cfg: &Config,
    cache_key: &str,
    key: &str,
    parts: &[db::SegmentPartLookup],
) -> Result<CacheEntry> {
    let mut tasks = JoinSet::new();
    let bots = Arc::new(cfg.bots.clone());
    for (i, part) in parts.iter().cloned().enumerate() {
        let state = state.clone();
        let bots = bots.clone();
        let key = key.to_string();
        let encryption_key = cfg.telegram_encryption_key.clone();
        tasks.spawn(async move {
            let bytes = match timeout(
                SEGMENT_FETCH_TIMEOUT,
                telegram::get_file_bytes(
                    &state.http,
                    &state.telegram,
                    &state.telegram_base_url,
                    &bots,
                    &part.file_id,
                    part.bot_index,
                ),
            )
            .await
            {
                Ok(Ok(bytes)) => bytes,
                Ok(Err(e)) => return Err(e).with_context(|| format!("fetching part {i} of {key}")),
                Err(_) => {
                    state.telegram.record_download_error(part.bot_index).await;
                    tracing::warn!(
                        key = %key,
                        part_index = part.part_index,
                        bot_index = part.bot_index,
                        timeout_ms = SEGMENT_FETCH_TIMEOUT.as_millis(),
                        "Telegram segment part fetch timed out"
                    );
                    bail!("fetching part {i} of {key} from Telegram timed out");
                }
            };
            let aad = segment_part_aad(&key, part.part_index);
            let bytes = crate::crypto::decrypt_optional(
                encryption_key.as_ref(),
                part.encryption_nonce.as_deref(),
                &aad,
                bytes,
            )?;
            Ok::<_, anyhow::Error>((i, bytes))
        });
    }

    let mut ordered = vec![Vec::new(); parts.len()];
    while let Some(result) = tasks.join_next().await {
        let (i, bytes) = result.context("multipart fetch task panicked")??;
        ordered[i] = bytes;
    }

    let total_size = ordered.iter().map(Vec::len).sum();
    let mut all_bytes = Vec::with_capacity(total_size);
    for bytes in ordered {
        all_bytes.extend_from_slice(&bytes);
    }
    let entry = super::cache_entry_for_bytes(cfg, cache_key, key, all_bytes).await?;
    state
        .cache
        .insert(
            cache_key.to_string(),
            entry.clone(),
            Some((cfg.segment_cache_size_mb as u64) * 1024 * 1024),
        )
        .await;
    Ok(entry)
}

pub(super) async fn fetch_file_bytes_with_timeout(
    state: &AppState,
    cfg: &Config,
    file_id: &str,
    bot_index: i64,
    job_id: &str,
    key: &str,
) -> Result<Vec<u8>> {
    match timeout(
        SEGMENT_FETCH_TIMEOUT,
        telegram::get_file_bytes(
            &state.http,
            &state.telegram,
            &state.telegram_base_url,
            &cfg.bots,
            file_id,
            bot_index,
        ),
    )
    .await
    {
        Ok(Ok(bytes)) => Ok(bytes),
        Ok(Err(e)) => Err(e).with_context(|| format!("fetching {key} from Telegram")),
        Err(_) => {
            state.telegram.record_download_error(bot_index).await;
            tracing::warn!(
                job_id,
                key,
                bot_index,
                timeout_ms = SEGMENT_FETCH_TIMEOUT.as_millis(),
                "Telegram segment fetch timed out"
            );
            bail!("fetching {key} from Telegram timed out")
        }
    }
}
