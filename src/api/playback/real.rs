use std::path::{Component, Path as FsPath, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use axum::body::Body;
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use tokio::task::JoinSet;
use tokio_stream::wrappers::ReceiverStream;

use super::cache::{claim_inflight, finish_inflight, CacheEntry, Inflight};
use super::{api_error, db_unavailable, AppState};
use crate::config::Config;
use crate::telegram;

struct InflightGuard {
    state: Arc<AppState>,
    key: String,
    inflight: Option<Arc<Inflight>>,
    fired: bool,
}

impl Drop for InflightGuard {
    fn drop(&mut self) {
        if self.inflight.is_some() && !self.fired {
            let state = self.state.clone();
            let key = self.key.clone();
            let inflight = self.inflight.take().unwrap();
            let err =
                Err::<CacheEntry, _>(anyhow::anyhow!("leader task panicked or was cancelled"));
            tokio::spawn(async move {
                finish_inflight(&state, &key, inflight, &err).await;
            });
        }
    }
}
use crate::{db, media};

pub async fn serve_real_segment(state: Arc<AppState>, job_id: String, key: String) -> Response {
    let cache_key = format!("{job_id}/{key}");
    if let Some(entry) = state.cache.get(&cache_key).await {
        mark_segment_played_and_cleanup(&state, &job_id, &key).await;
        return super::cache_response(super::entry_for_key(&key, entry));
    }
    state
        .cache
        .misses
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let cfg = state.config.read().await.clone();

    // First check if this segment has parts
    let parts = {
        let conn = match state.db_conn().await {
            Ok(conn) => conn,
            Err(e) => return db_unavailable(e),
        };
        let job_id_for_db = job_id.clone();
        let key_for_db = key.clone();
        match tokio::task::spawn_blocking(move || {
            db::get_segment_parts(&conn, &job_id_for_db, &key_for_db)
        })
        .await
        {
            Ok(Ok(p)) => p,
            Ok(Err(e)) => {
                return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db_error", e.to_string());
            }
            Err(e) => {
                return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db_error", e.to_string());
            }
        }
    };

    if !parts.is_empty() {
        // Reconstruct from parts
        return serve_reconstructed_segment(state, cfg, job_id, key, parts).await;
    }

    // Normal segment (no parts)
    let (file_id, bot_index, encryption_nonce) = {
        let conn = match state.db_conn().await {
            Ok(conn) => conn,
            Err(e) => return db_unavailable(e),
        };
        let job_id_for_db = job_id.clone();
        let key_for_db = key.clone();
        match tokio::task::spawn_blocking(move || {
            db::get_segment(&conn, &job_id_for_db, &key_for_db)
        })
        .await
        {
            Ok(Ok(Some(s))) => (s.file_id, s.bot_index, s.encryption_nonce),
            Ok(Ok(None)) => {
                return api_error(StatusCode::NOT_FOUND, "not_found", "segment not found");
            }
            Ok(Err(e)) => {
                return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db_error", e.to_string());
            }
            Err(e) => {
                return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db_error", e.to_string());
            }
        }
    };

    if key.ends_with(".vtt") || encryption_nonce.is_some() {
        return match fetch_real_with_singleflight(
            &state,
            &cfg,
            &cache_key,
            &job_id,
            &file_id,
            bot_index,
            &key,
            encryption_nonce.as_deref(),
        )
        .await
        {
            Ok(entry) => {
                mark_segment_played_and_cleanup(&state, &job_id, &key).await;
                super::cache_response(entry)
            }
            Err(e) => api_error(StatusCode::NOT_FOUND, "fetch_failed", e.to_string()),
        };
    }

    let (inflight, is_leader) = claim_inflight(&state, &cache_key).await;
    if !is_leader {
        // Non-leader: wait for the leader to finish, then serve from cache.
        let outcome = inflight.wait_for_outcome().await;
        return match outcome {
            Ok(Some(entry)) => {
                mark_segment_played_and_cleanup(&state, &job_id, &key).await;
                spawn_prefetch_real(state, job_id, key);
                super::cache_response(entry)
            }
            Ok(None) => {
                if let Some(entry) = state.cache.get(&cache_key).await {
                    mark_segment_played_and_cleanup(&state, &job_id, &key).await;
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
            Err(e) => api_error(StatusCode::NOT_FOUND, "fetch_failed", e),
        };
    }

    // Leader: start streaming immediately, accumulate for cache in background.
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
        Err(fetch_err) => {
            match reupload_from_cache_or_source(&state, &cfg, &job_id, &key, bot_index, &fetch_err)
                .await
            {
                Some(Ok(entry)) => {
                    finish_inflight(&state, &cache_key, inflight, &Ok(entry.clone())).await;
                    mark_segment_played_and_cleanup(&state, &job_id, &key).await;
                    spawn_prefetch_real(state, job_id, key);
                    return super::cache_response(entry);
                }
                Some(Err(recover_err)) => {
                    finish_inflight(
                        &state,
                        &cache_key,
                        inflight,
                        &Err::<CacheEntry, _>(
                            recover_err.context(format!("fetching {key} after recovery attempt")),
                        ),
                    )
                    .await;
                    return api_error(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "temporary_fetch_failed",
                        format!("fetching {key} from Telegram failed after recovery attempt"),
                    );
                }
                None => {
                    finish_inflight(
                        &state,
                        &cache_key,
                        inflight,
                        &Err::<CacheEntry, _>(
                            fetch_err.context(format!("fetching {key} from Telegram")),
                        ),
                    )
                    .await;
                    return api_error(
                        StatusCode::NOT_FOUND,
                        "fetch_failed",
                        format!("fetching {key} from Telegram failed"),
                    );
                }
            }
        }
    };

    let (tx, rx) = tokio::sync::mpsc::channel::<Result<axum::body::Bytes, std::io::Error>>(16);
    let ct = super::content_type_for(&key);
    let cache_key_owned = cache_key.clone();
    let response_key = key.clone();
    let job_id_for_cleanup = job_id.clone();
    let cache_budget = (cfg.segment_cache_size_mb as u64) * 1024 * 1024;
    let cfg_for_cache = cfg.clone();
    let state2 = state.clone();

    tokio::spawn(async move {
        use tokio_stream::StreamExt as _;
        let mut guard = InflightGuard {
            state: state2.clone(),
            key: cache_key_owned.clone(),
            inflight: Some(inflight.clone()),
            fired: false,
        };
        let t0 = std::time::Instant::now();
        let mut all_bytes: Vec<u8> = Vec::new();
        let mut client_disconnected = false;
        let stream = resp.bytes_stream();
        tokio::pin!(stream);
        loop {
            match stream.next().await {
                None => break,
                Some(Ok(chunk)) => {
                    all_bytes.extend_from_slice(&chunk);
                    if !client_disconnected && tx.send(Ok(chunk)).await.is_err() {
                        client_disconnected = true;
                    }
                }
                Some(Err(e)) => {
                    let err: anyhow::Error = anyhow::anyhow!("stream error: {e}");
                    if !client_disconnected {
                        let _ = tx.send(Err(std::io::Error::other(e))).await;
                    }
                    guard.state.telegram.record_download_error(bot_index).await;
                    guard.fired = true;
                    finish_inflight(
                        &guard.state,
                        &guard.key,
                        guard.inflight.take().unwrap(),
                        &Err(err),
                    )
                    .await;
                    return;
                }
            }
        }
        let elapsed = t0.elapsed().as_secs_f64();
        let nbytes = all_bytes.len() as u64;
        let entry = match super::cache_entry_for_bytes(
            &cfg_for_cache,
            &guard.key,
            &response_key,
            all_bytes,
        )
        .await
        {
            Ok(entry) => entry,
            Err(e) => {
                guard.fired = true;
                finish_inflight(
                    &guard.state,
                    &guard.key,
                    guard.inflight.take().unwrap(),
                    &Err::<CacheEntry, _>(e),
                )
                .await;
                return;
            }
        };
        guard
            .state
            .cache
            .insert(guard.key.clone(), entry.clone(), Some(cache_budget))
            .await;
        guard
            .state
            .telegram
            .record_download_success(bot_index, nbytes, elapsed)
            .await;
        guard.fired = true;
        finish_inflight(
            &guard.state,
            &guard.key,
            guard.inflight.take().unwrap(),
            &Ok(entry),
        )
        .await;
        mark_segment_played_and_cleanup(&guard.state, &job_id_for_cleanup, &response_key).await;
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
    cfg: Arc<Config>,
    job_id: String,
    key: String,
    parts: Vec<db::SegmentPartLookup>,
) -> Response {
    let cache_key = format!("{job_id}/{key}");
    let (inflight, is_leader) = claim_inflight(&state, &cache_key).await;
    if !is_leader {
        // Non-leader: wait for leader to finish, then serve from cache.
        let outcome = inflight.wait_for_outcome().await;
        return match outcome {
            Ok(Some(entry)) => {
                mark_segment_played_and_cleanup(&state, &job_id, &key).await;
                spawn_prefetch_real(state, job_id, key);
                super::cache_response(entry)
            }
            Ok(None) => {
                if let Some(entry) = state.cache.get(&cache_key).await {
                    mark_segment_played_and_cleanup(&state, &job_id, &key).await;
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
            Err(e) => api_error(StatusCode::NOT_FOUND, "fetch_failed", e),
        };
    }

    let entry = match reconstructed_fetch_into_cache(&state, &cfg, &cache_key, &key, &parts).await {
        Ok(entry) => entry,
        Err(e) => {
            finish_inflight(&state, &cache_key, inflight, &Err::<CacheEntry, _>(e)).await;
            return api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "cache_write_failed",
                "writing reconstructed segment cache failed",
            );
        }
    };

    let entry_clone = entry.clone();
    let state2 = state.clone();
    tokio::spawn(async move {
        finish_inflight(&state2, &cache_key, inflight, &Ok(entry_clone)).await;
    });

    spawn_prefetch_real(state.clone(), job_id.clone(), key.clone());
    mark_segment_played_and_cleanup(&state, &job_id, &key).await;
    super::cache_response(entry)
}

pub(super) async fn mark_segment_played_and_cleanup(
    state: &Arc<AppState>,
    job_id: &str,
    key: &str,
) {
    {
        let mut played = state.played_segments.lock().await;
        let now = std::time::Instant::now();
        // Prune entries older than 2 hours (7200 seconds) to prevent memory leaks from abandoned playbacks
        played.retain(|_, (_, last_activity)| {
            now.duration_since(*last_activity) < std::time::Duration::from_secs(7200)
        });

        let entry = played
            .entry(job_id.to_string())
            .or_insert_with(|| (std::collections::HashSet::new(), now));
        entry.0.insert(key.to_string());
        entry.1 = now;
    }

    let (segments, source_path) = {
        let conn = match state.db_conn().await {
            Ok(conn) => conn,
            Err(e) => {
                tracing::warn!(job_id, error = %e, "segment playback cleanup DB connection failed");
                return;
            }
        };
        let job_id_for_db = job_id.to_string();
        let result = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
            let segments = db::get_segments_for_job(&conn, &job_id_for_db)?;
            let source_path = db::get_job(&conn, &job_id_for_db)?.and_then(|job| job.source_path);
            Ok((segments, source_path))
        })
        .await;
        match result {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => {
                tracing::warn!(job_id, error = %e, "segment playback cleanup DB query failed");
                return;
            }
            Err(e) => {
                tracing::warn!(job_id, error = %e, "segment playback cleanup DB task failed");
                return;
            }
        }
    };

    if segments.is_empty() {
        return;
    }
    let all_played = {
        let played = state.played_segments.lock().await;
        let Some((played_set, _)) = played.get(job_id) else {
            return;
        };
        segments
            .iter()
            .all(|segment| played_set.contains(&segment.segment_key))
    };
    if !all_played {
        return;
    }

    let Some(path) = pending_delete_source_path(state, source_path.as_deref()) else {
        state.played_segments.lock().await.remove(job_id);
        return;
    };
    match tokio::fs::remove_file(&path).await {
        Ok(()) => {
            tracing::info!(
                job_id,
                source_path = %path.display(),
                "deleted retained source after segment playback confirmation"
            );
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            tracing::warn!(
                job_id,
                source_path = %path.display(),
                error = %e,
                "failed to delete retained source after segment playback confirmation"
            );
            return;
        }
    }
    state.played_segments.lock().await.remove(job_id);
}

fn pending_delete_source_path(
    state: &AppState,
    source_path: Option<&str>,
) -> Option<std::path::PathBuf> {
    let rel = source_path?;
    if rel.contains("..") || rel.contains('/') || rel.contains('\\') || rel.contains('\0') {
        tracing::warn!(source_path = %rel, "ignoring invalid stored source path for deferred deletion");
        return None;
    }
    if rel.ends_with(".pending_delete") {
        Some(state.uploads_dir.join(rel))
    } else {
        Some(state.uploads_dir.join(format!("{rel}.pending_delete")))
    }
}

async fn bytes_for_re_upload(
    state: &AppState,
    cfg: &Config,
    job_id: &str,
    key: &str,
    cache_key: &str,
) -> Result<CacheEntry> {
    if let Some(entry) = state.cache.get(cache_key).await {
        return Ok(entry);
    }

    let Some(bytes) = extract_recovery_segment_from_source(state, cfg, job_id, key).await? else {
        tracing::warn!(
            job_id,
            key,
            "Telegram permanent error but no cached or retained source data for recovery"
        );
        bail!("stale Telegram file_id and no cached segment available");
    };

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

async fn extract_recovery_segment_from_source(
    state: &AppState,
    cfg: &Config,
    job_id: &str,
    key: &str,
) -> Result<Option<Vec<u8>>> {
    let source_path = {
        let conn = state
            .db_conn()
            .await
            .context("getting sqlite connection for recovery source lookup")?;
        let job_id_for_db = job_id.to_string();
        match tokio::task::spawn_blocking(move || db::get_job(&conn, &job_id_for_db)).await?? {
            Some(job) => pending_delete_source_path(state, job.source_path.as_deref()),
            None => None,
        }
    };
    let Some(source_path) = source_path else {
        return Ok(None);
    };
    match tokio::fs::metadata(&source_path).await {
        Ok(meta) if meta.is_file() => {}
        Ok(_) => return Ok(None),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("stat {}", source_path.display())),
    }

    let Some(segment_path) = recovery_segment_path(key) else {
        bail!("invalid recovery segment key {key}");
    };
    // Recovery via full source re-encode is capped to init segments only.
    // Full re-encode for a media segment could take hours for feature-length
    // content and consume tens of GB of disk — not practical for a single segment.
    let file_name = segment_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    if !file_name.starts_with("init") {
        tracing::info!(
            job_id = %job_id,
            key = %key,
            "skipping stale file_id recovery for non-init segment"
        );
        return Ok(None);
    }
    let work_dir = state.processing_dir.join(format!(
        "reupload_extract_{}",
        uuid::Uuid::new_v4().simple()
    ));
    tokio::fs::create_dir_all(&work_dir)
        .await
        .with_context(|| format!("create recovery workspace {}", work_dir.display()))?;

    let result = async {
        let analysis = media::analyze_media(&source_path)
            .await
            .with_context(|| format!("analyzing retained source {}", source_path.display()))?;
        let cancel = Arc::new(AtomicBool::new(false));
        media::process_media(&analysis, job_id, &work_dir, cfg, &cancel, None)
            .await
            .with_context(|| format!("re-extracting {key} from retained source"))?;
        let output_path = work_dir.join(segment_path);
        tokio::fs::read(&output_path)
            .await
            .with_context(|| format!("read recovered segment {}", output_path.display()))
    }
    .await;

    let _ = tokio::fs::remove_dir_all(&work_dir).await;
    result.map(Some)
}

fn recovery_segment_path(key: &str) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for component in FsPath::new(key).components() {
        match component {
            Component::Normal(part) => out.push(part),
            _ => return None,
        }
    }
    if out.as_os_str().is_empty() {
        None
    } else {
        Some(out)
    }
}

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

async fn real_fetch_into_cache(
    state: &AppState,
    cfg: &Config,
    cache_key: &str,
    job_id: &str,
    file_id: &str,
    bot_index: i64,
    key: &str,
    encryption_nonce: Option<&str>,
) -> Result<CacheEntry> {
    let bytes = match telegram::get_file_bytes(
        &state.http,
        &state.telegram,
        &state.telegram_base_url,
        &cfg.bots,
        file_id,
        bot_index,
    )
    .await
    {
        Ok(bytes) => bytes,
        Err(fetch_err) => {
            if let Some(recovered) =
                reupload_from_cache_or_source(state, cfg, job_id, key, bot_index, &fetch_err).await
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

async fn fetch_reconstructed_with_singleflight(
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

async fn reconstructed_fetch_into_cache(
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
            let bytes = telegram::get_file_bytes(
                &state.http,
                &state.telegram,
                &state.telegram_base_url,
                &bots,
                &part.file_id,
                part.bot_index,
            )
            .await
            .with_context(|| format!("fetching part {i} of {key}"))?;
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

async fn reupload_from_cache_or_source(
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
    let entry = match bytes_for_re_upload(state, cfg, job_id, key, &cache_key).await {
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
            let conn = match state.db_conn().await {
                Ok(conn) => conn,
                Err(e) => {
                    tracing::debug!(job_id = %job_id, error = %e, "segment prefetch DB connection failed");
                    return;
                }
            };
            let job_id_for_db = job_id.clone();
            let prefix_for_db = prefix.clone();
            match tokio::task::spawn_blocking(move || {
                db::get_segments_for_prefix(&conn, &job_id_for_db, &prefix_for_db)
            })
            .await
            {
                Ok(Ok(s)) => s,
                _ => return,
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
            let result = if next.is_split {
                let parts = {
                    let segment_key = next.segment_key.clone();
                    let job_id_for_db = job_id.clone();
                    match state.db_conn().await {
                        Ok(conn) => tokio::task::spawn_blocking(move || {
                            db::get_segment_parts(&conn, &job_id_for_db, &segment_key)
                                .map_err(anyhow::Error::from)
                        })
                        .await
                        .unwrap_or_else(|e| Err(anyhow::anyhow!(e))),
                        Err(e) => Err(e),
                    }
                };
                match parts {
                    Ok(parts) => {
                        fetch_reconstructed_with_singleflight(
                            &state,
                            &cfg,
                            &cache_key,
                            &next.segment_key,
                            &parts,
                        )
                        .await
                    }
                    Err(e) => Err(e),
                }
            } else {
                fetch_real_with_singleflight(
                    &state,
                    &cfg,
                    &cache_key,
                    &job_id,
                    &next.file_id,
                    next.bot_index,
                    &next.segment_key,
                    next.encryption_nonce.as_deref(),
                )
                .await
            };
            if let Err(e) = result {
                tracing::debug!(job_id = %job_id, segment_key = %next.segment_key, error = %e, "segment prefetch failed");
            }
        }
    });
}

pub(crate) fn spawn_cache_warmup(state: Arc<AppState>, job_id: String) {
    tokio::spawn(async move {
        let cfg = state.config.read().await.clone();
        if !cfg.cache_warmup_enabled {
            return;
        }
        let segs = {
            let conn = match state.db_conn().await {
                Ok(conn) => conn,
                Err(e) => {
                    tracing::warn!(job_id = %job_id, error = %e, "cache warm-up DB connection failed");
                    return;
                }
            };
            let job_id_for_db = job_id.clone();
            match tokio::task::spawn_blocking(move || {
                db::get_segments_for_job(&conn, &job_id_for_db)
            })
            .await
            {
                Ok(Ok(segs)) => segs,
                Ok(Err(e)) => {
                    tracing::warn!(job_id = %job_id, error = %e, "cache warm-up segment query failed");
                    return;
                }
                Err(e) => {
                    tracing::warn!(job_id = %job_id, error = %e, "cache warm-up segment query failed");
                    return;
                }
            }
        };
        for seg in select_cache_warmup_segments(&segs) {
            if cfg.segment_prefetch_min_free_bytes > 0
                && state.cache.free_bytes().await < cfg.segment_prefetch_min_free_bytes
            {
                return;
            }
            let cache_key = format!("{job_id}/{}", seg.segment_key);
            if state.cache.get(&cache_key).await.is_some() {
                continue;
            }
            let result = if seg.is_split {
                let parts_result = {
                    let segment_key = seg.segment_key.clone();
                    let job_id_for_db = job_id.clone();
                    match state.db_conn().await {
                        Ok(conn) => tokio::task::spawn_blocking(move || {
                            db::get_segment_parts(&conn, &job_id_for_db, &segment_key)
                                .map_err(anyhow::Error::from)
                        })
                        .await
                        .unwrap_or_else(|e| Err(anyhow::anyhow!(e))),
                        Err(e) => Err(e),
                    }
                };
                match parts_result {
                    Ok(parts) => {
                        fetch_reconstructed_with_singleflight(
                            &state,
                            &cfg,
                            &cache_key,
                            &seg.segment_key,
                            &parts,
                        )
                        .await
                    }
                    Err(e) => Err(e),
                }
            } else {
                fetch_real_with_singleflight(
                    &state,
                    &cfg,
                    &cache_key,
                    &job_id,
                    &seg.file_id,
                    seg.bot_index,
                    &seg.segment_key,
                    seg.encryption_nonce.as_deref(),
                )
                .await
            };
            if let Err(e) = result {
                tracing::warn!(job_id = %job_id, segment_key = %seg.segment_key, error = %e, "cache warm-up fetch failed");
            }
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
    });
}

pub(super) fn select_cache_warmup_segments(segs: &[db::SegmentRow]) -> Vec<db::SegmentRow> {
    let mut video: Vec<&db::SegmentRow> = segs
        .iter()
        .filter(|s| s.segment_key.starts_with("video_") && !s.segment_key.ends_with("/init.mp4"))
        .collect();
    let mut audio: Vec<&db::SegmentRow> = segs
        .iter()
        .filter(|s| s.segment_key.starts_with("audio_"))
        .collect();
    video.sort_by_key(|s| warmup_sort_key(&s.segment_key, "video_0"));
    audio.sort_by_key(|s| warmup_sort_key(&s.segment_key, "audio_0"));

    let mut selected = Vec::new();
    if let Some(seg) = video.first() {
        selected.push((*seg).clone());
    }
    if let Some(seg) = audio.first() {
        selected.push((*seg).clone());
    }
    if let Some(seg) = segs
        .iter()
        .find(|s| s.segment_key == "thumbnail/thumbnail.jpg")
    {
        selected.push(seg.clone());
    }
    selected
}

fn warmup_sort_key<'a>(segment_key: &'a str, preferred_prefix: &str) -> (u8, &'a str) {
    let prefix = segment_key.split_once('/').map(|(p, _)| p).unwrap_or("");
    (u8::from(prefix != preferred_prefix), segment_key)
}
