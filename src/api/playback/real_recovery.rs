use anyhow::Result;

use super::cache::CacheEntry;
use super::AppState;
use crate::config::Config;
use crate::telegram;

use crate::db;

pub(super) async fn reupload_from_cache_or_source(
    state: &AppState,
    cfg: &Config,
    job_id: &str,
    key: &str,
    stale_bot_index: i64,
    fetch_err: &anyhow::Error,
) -> Option<Result<CacheEntry>> {
    if !telegram_error_suggests_stale_file_id(fetch_err) {
        tracing::debug!(job_id, key, error = %fetch_err, "not a permanent Telegram error, skipping recovery");
        return None;
    }

    let cache_key = format!("{job_id}/{key}");
    let entry = match super::real::bytes_for_re_upload(state, cfg, job_id, key, &cache_key).await {
        Ok(entry) => entry,
        Err(e) => return Some(Err(e)),
    };

    tracing::warn!(
        job_id,
        key,
        stale_bot_index,
        bytes = entry.bytes.len(),
        "attempting segment re-upload recovery"
    );

    let bots = &cfg.bots;
    let bot_indices: Vec<i64> = std::iter::once(stale_bot_index)
        .chain((0..bots.len() as i64).filter(|i| *i != stale_bot_index))
        .collect();

    for bot_idx in &bot_indices {
        let Some(bot) = bots.get(*bot_idx as usize) else {
            continue;
        };

        let tmp_path = state
            .processing_dir
            .join(format!("reupload_{}", uuid::Uuid::new_v4().simple()));
        if let Err(e) = tokio::fs::write(&tmp_path, entry.bytes.as_slice()).await {
            tracing::warn!(error = %e, "failed to write temp file for re-upload");
            continue;
        }

        let segment_key_for_upload = key;
        let result = telegram::upload_document(
            &state.http,
            &state.telegram,
            &state.telegram_base_url,
            bot.clone(),
            *bot_idx,
            &tmp_path,
            segment_key_for_upload.to_string(),
            cfg.telegram_encryption_key.as_ref(),
            // User-configurable; raise if Telegram increases Bot API limits.
            cfg.telegram_max_file_size,
        )
        .await;

        let _ = tokio::fs::remove_file(&tmp_path).await;

        match result {
            Ok(uploaded) => {
                tracing::info!(
                    job_id, key,
                    new_file_id = %uploaded.file_id,
                    new_bot_index = uploaded.bot_index,
                    "segment re-uploaded successfully"
                );
                if let Ok(conn) = state.db_conn().await {
                    let _ = db::update_segment_file_id(
                        &conn,
                        job_id,
                        key,
                        &uploaded.file_id,
                        uploaded.bot_index,
                        uploaded.encryption_nonce.as_deref(),
                    );
                }
                return Some(Ok(entry));
            }
            Err(e) => {
                tracing::warn!(
                    job_id, key, bot_index = *bot_idx,
                    error = %e,
                    "re-upload attempt failed"
                );
            }
        }
    }

    tracing::warn!(job_id, key, "all re-upload attempts failed");
    Some(Err(anyhow::anyhow!("segment re-upload recovery failed")))
}

pub(super) fn telegram_error_suggests_stale_file_id(err: &anyhow::Error) -> bool {
    let err = err.to_string();
    err.contains("400")
        || err.contains("403")
        || err.contains("Bad Request")
        || err.contains("Forbidden")
}
