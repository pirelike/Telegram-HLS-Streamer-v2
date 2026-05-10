use std::sync::Arc;

use anyhow::{anyhow, bail, Context, Result};
use axum::body::Body;
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use tokio_stream::wrappers::ReceiverStream;

use super::cache::{claim_inflight, finish_inflight, CacheEntry};
use super::{api_error, AppState};
use crate::db;
use crate::telegram;

pub async fn serve_real_segment(state: Arc<AppState>, job_id: String, key: String) -> Response {
    let cache_key = format!("{job_id}/{key}");
    if let Some(entry) = state.cache.get(&cache_key).await {
        return super::cache_response(super::entry_for_key(&key, entry));
    }
    state
        .cache
        .misses
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    // First check if this segment has parts
    let parts = {
        let conn = state.db.lock().await;
        match db::get_segment_parts(&conn, &job_id, &key) {
            Ok(p) => p,
            Err(e) => {
                return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db_error", e.to_string())
            }
        }
    };

    if !parts.is_empty() {
        // Reconstruct from parts
        return serve_reconstructed_segment(state, job_id, key, parts).await;
    }

    // Normal segment (no parts)
    let (file_id, bot_index) = {
        let conn = state.db.lock().await;
        match db::get_segment(&conn, &job_id, &key) {
            Ok(Some(s)) => (s.file_id, s.bot_index),
            Ok(None) => return api_error(StatusCode::NOT_FOUND, "not_found", "segment not found"),
            Err(e) => {
                return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db_error", e.to_string())
            }
        }
    };

    if key.ends_with(".vtt") {
        return match fetch_real_with_singleflight(&state, &cache_key, &file_id, bot_index, &key)
            .await
        {
            Ok(entry) => super::cache_response(entry),
            Err(e) => api_error(StatusCode::NOT_FOUND, "fetch_failed", e.to_string()),
        };
    }

    let (inflight, is_leader) = claim_inflight(&state, &cache_key).await;
    if !is_leader {
        // Non-leader: wait for the leader to finish, then serve from cache.
        inflight.notify.notified().await;
        let outcome = inflight.outcome.lock().await.clone();
        return match outcome {
            Some(Ok(())) => {
                if let Some(entry) = state.cache.get(&cache_key).await {
                    spawn_prefetch_real(state, job_id, key);
                    super::cache_response(entry)
                } else {
                    api_error(
                        StatusCode::NOT_FOUND,
                        "fetch_failed",
                        "cache miss after single-flight wait",
                    )
                }
            }
            Some(Err(e)) => api_error(StatusCode::NOT_FOUND, "fetch_failed", e),
            None => api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "fetch_failed",
                "leader produced no outcome",
            ),
        };
    }

    // Leader: start streaming immediately, accumulate for cache in background.
    let cfg = state.config.read().await.clone();
    let resp = match telegram::get_file_response(
        &state.http,
        &state.telegram,
        &state.telegram_base_url,
        &cfg.bots,
        &file_id,
        bot_index,
    )
    .await
    {
        Ok(r) => r,
        Err(e) => {
            finish_inflight(
                &state,
                &cache_key,
                inflight,
                &Err::<CacheEntry, _>(e.context(format!("fetching {key} from Telegram"))),
            )
            .await;
            return api_error(
                StatusCode::NOT_FOUND,
                "fetch_failed",
                format!("fetching {key} from Telegram failed"),
            );
        }
    };

    let (tx, rx) = tokio::sync::mpsc::channel::<Result<axum::body::Bytes, std::io::Error>>(16);
    let ct = super::content_type_for(&key);
    let cache_key_owned = cache_key.clone();
    let cache_budget = (cfg.segment_cache_size_mb as u64) * 1024 * 1024;
    let state2 = state.clone();

    tokio::spawn(async move {
        use tokio_stream::StreamExt as _;
        let t0 = std::time::Instant::now();
        let mut all_bytes: Vec<u8> = Vec::new();
        let stream = resp.bytes_stream();
        tokio::pin!(stream);
        loop {
            match stream.next().await {
                None => break,
                Some(Ok(chunk)) => {
                    all_bytes.extend_from_slice(&chunk);
                    // Ignore send error: client may have disconnected, but we still
                    // want to finish downloading and populate the cache.
                    let _ = tx.send(Ok(chunk)).await;
                }
                Some(Err(e)) => {
                    let err: anyhow::Error = anyhow::anyhow!("stream error: {e}");
                    let _ = tx.send(Err(std::io::Error::other(e))).await;
                    state2.telegram.record_download_error(bot_index).await;
                    finish_inflight(&state2, &cache_key_owned, inflight, &Err(err)).await;
                    return;
                }
            }
        }
        let elapsed = t0.elapsed().as_secs_f64();
        let nbytes = all_bytes.len() as u64;
        let entry = CacheEntry {
            content_type: ct,
            bytes: Arc::new(all_bytes),
        };
        state2
            .cache
            .insert(cache_key_owned.clone(), entry.clone(), Some(cache_budget))
            .await;
        state2
            .telegram
            .record_download_success(bot_index, nbytes, elapsed)
            .await;
        finish_inflight(&state2, &cache_key_owned, inflight, &Ok(entry)).await;
    });

    spawn_prefetch_real(state, job_id, key);

    let body = Body::from_stream(ReceiverStream::new(rx));
    let mut resp = (StatusCode::OK, body).into_response();
    let headers = resp.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(ct));
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=3600"),
    );
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_static("*"),
    );
    resp
}

pub(super) async fn serve_reconstructed_segment(
    state: Arc<AppState>,
    job_id: String,
    key: String,
    parts: Vec<db::SegmentPartLookup>,
) -> Response {
    let cache_key = format!("{job_id}/{key}");
    let (inflight, is_leader) = claim_inflight(&state, &cache_key).await;
    if !is_leader {
        // Non-leader: wait for leader to finish, then serve from cache.
        inflight.notify.notified().await;
        let outcome = inflight.outcome.lock().await.clone();
        return match outcome {
            Some(Ok(())) => {
                if let Some(entry) = state.cache.get(&cache_key).await {
                    spawn_prefetch_real(state, job_id, key);
                    super::cache_response(entry)
                } else {
                    api_error(
                        StatusCode::NOT_FOUND,
                        "fetch_failed",
                        "cache miss after single-flight wait",
                    )
                }
            }
            Some(Err(e)) => api_error(StatusCode::NOT_FOUND, "fetch_failed", e),
            None => api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "fetch_failed",
                "leader produced no outcome",
            ),
        };
    }

    let cfg = state.config.read().await.clone();
    let ct = super::content_type_for(&key);
    let cache_budget = (cfg.segment_cache_size_mb as u64) * 1024 * 1024;

    // Download all parts and concatenate
    let mut all_bytes = Vec::new();
    for (i, part) in parts.iter().enumerate() {
        match telegram::get_file_response(
            &state.http,
            &state.telegram,
            &state.telegram_base_url,
            &cfg.bots,
            &part.file_id,
            part.bot_index,
        )
        .await
        {
            Ok(resp) => {
                use tokio_stream::StreamExt as _;
                let stream = resp.bytes_stream();
                tokio::pin!(stream);
                loop {
                    match stream.next().await {
                        None => break,
                        Some(Ok(chunk)) => {
                            all_bytes.extend_from_slice(&chunk[..]);
                        }
                        Some(Err(e)) => {
                            finish_inflight(
                                &state,
                                &cache_key,
                                inflight,
                                &Err::<CacheEntry, _>(anyhow!("part {} fetch error: {}", i, e)),
                            )
                            .await;
                            return api_error(
                                StatusCode::NOT_FOUND,
                                "fetch_failed",
                                format!("fetching part {} of {} failed", i, key),
                            );
                        }
                    }
                }
            }
            Err(e) => {
                finish_inflight(
                    &state,
                    &cache_key,
                    inflight,
                    &Err::<CacheEntry, _>(anyhow!("part {} fetch error: {}", i, e)),
                )
                .await;
                return api_error(
                    StatusCode::NOT_FOUND,
                    "fetch_failed",
                    format!("fetching part {} of {} failed", i, key),
                );
            }
        }
    }

    let entry = CacheEntry {
        content_type: ct,
        bytes: Arc::new(all_bytes),
    };

    let entry_clone = entry.clone();
    let entry_for_cache = entry.clone();
    let state2 = state.clone();
    tokio::spawn(async move {
        state2
            .cache
            .insert(cache_key.clone(), entry_for_cache, Some(cache_budget))
            .await;
        finish_inflight(&state2, &cache_key, inflight, &Ok(entry_clone)).await;
    });

    spawn_prefetch_real(state, job_id.clone(), key.clone());
    super::cache_response(entry)
}

pub(super) async fn fetch_real_with_singleflight(
    state: &Arc<AppState>,
    cache_key: &str,
    file_id: &str,
    bot_index: i64,
    key: &str,
) -> Result<CacheEntry> {
    let (inflight, is_leader) = claim_inflight(state, cache_key).await;
    if !is_leader {
        inflight.notify.notified().await;
        let outcome = inflight.outcome.lock().await.clone();
        match outcome {
            Some(Ok(())) => {
                if let Some(entry) = state.cache.get(cache_key).await {
                    return Ok(entry);
                }
                bail!("cache miss after single-flight wait");
            }
            Some(Err(e)) => bail!(e),
            None => bail!("leader produced no outcome"),
        }
    }
    let result = real_fetch_into_cache(state, cache_key, file_id, bot_index, key).await;
    finish_inflight(state, cache_key, inflight, &result).await;
    result
}

async fn real_fetch_into_cache(
    state: &AppState,
    cache_key: &str,
    file_id: &str,
    bot_index: i64,
    key: &str,
) -> Result<CacheEntry> {
    let cfg = state.config.read().await.clone();
    let bytes = telegram::get_file_bytes(
        &state.http,
        &state.telegram,
        &state.telegram_base_url,
        &cfg.bots,
        file_id,
        bot_index,
    )
    .await
    .with_context(|| format!("fetching {key} from Telegram"))?;
    let entry = CacheEntry {
        content_type: super::content_type_for(key),
        bytes: Arc::new(super::bytes_for_key(key, bytes)),
    };
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

pub(super) fn spawn_prefetch_real(state: Arc<AppState>, job_id: String, key: String) {
    tokio::spawn(async move {
        let cfg = state.config.read().await.clone();
        let n = cfg.segment_prefetch_count as usize;
        if n == 0 {
            return;
        }
        if cfg.segment_prefetch_min_free_bytes > 0
            && state.cache.free_bytes().await < cfg.segment_prefetch_min_free_bytes
        {
            return;
        }
        let prefix = match key.split_once('/') {
            Some((p, _)) => p.to_string(),
            None => return,
        };
        let segs = {
            let conn = state.db.lock().await;
            match db::get_segments_for_prefix(&conn, &job_id, &prefix) {
                Ok(s) => s,
                Err(_) => return,
            }
        };
        let pos = match segs.iter().position(|s| s.segment_key == key) {
            Some(p) => p,
            None => return,
        };
        for next in segs.iter().skip(pos + 1).take(n) {
            let cache_key = format!("{job_id}/{}", next.segment_key);
            if state.cache.get(&cache_key).await.is_some() {
                continue;
            }
            let _ = fetch_real_with_singleflight(
                &state,
                &cache_key,
                &next.file_id,
                next.bot_index,
                &next.segment_key,
            )
            .await;
        }
    });
}
