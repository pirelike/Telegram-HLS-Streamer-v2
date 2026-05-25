use std::path::{Component, Path as FsPath, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use axum::http::StatusCode;
use axum::response::Response;

use super::cache::{claim_inflight, finish_inflight, CacheEntry, InflightGuard};
use super::{api_error, db_unavailable, AppState};
use crate::config::Config;

use super::real_fetch::{fetch_real_with_singleflight, reconstructed_fetch_into_cache};
#[cfg(test)]
pub(super) use super::real_prefetch::select_cache_warmup_segments;
use super::real_prefetch::spawn_prefetch_real;
#[cfg(test)]
pub(super) use super::real_recovery::telegram_error_suggests_stale_file_id;
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

    match fetch_real_with_singleflight(
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
            spawn_prefetch_real(state, job_id, key);
            super::cache_response(entry)
        }
        Err(e) => api_error(StatusCode::NOT_FOUND, "fetch_failed", e.to_string()),
    }
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

    let mut guard = InflightGuard::new(state.clone(), cache_key.clone(), inflight.clone());
    let entry = match reconstructed_fetch_into_cache(&state, &cfg, &cache_key, &key, &parts).await {
        Ok(entry) => entry,
        Err(e) => {
            finish_inflight(&state, &cache_key, inflight, &Err::<CacheEntry, _>(e)).await;
            guard.disarm();
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
    guard.disarm();

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

    // Fetch segment keys and source_path from cache (populated once per job, TTL 1 hour)
    let cache_ttl = std::time::Duration::from_secs(3600);
    let (segment_keys, source_path) = {
        let cached = {
            let cache = state.segment_meta_cache.lock().await;
            cache.get(job_id).and_then(|(keys, sp, ts)| {
                if ts.elapsed() < cache_ttl {
                    Some((keys.clone(), sp.clone()))
                } else {
                    None
                }
            })
        };
        match cached {
            Some(v) => v,
            None => {
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
                    let source_path =
                        db::get_job(&conn, &job_id_for_db)?.and_then(|job| job.source_path);
                    Ok((segments, source_path))
                })
                .await;
                match result {
                    Ok(Ok((segments, source))) => {
                        let keys = Arc::new(
                            segments
                                .iter()
                                .map(|s| s.segment_key.clone())
                                .collect::<Vec<_>>(),
                        );
                        state.segment_meta_cache.lock().await.insert(
                            job_id.to_string(),
                            (keys.clone(), source.clone(), std::time::Instant::now()),
                        );
                        (keys, source)
                    }
                    Ok(Err(e)) => {
                        tracing::warn!(job_id, error = %e, "segment playback cleanup DB query failed");
                        return;
                    }
                    Err(e) => {
                        tracing::warn!(job_id, error = %e, "segment playback cleanup DB task failed");
                        return;
                    }
                }
            }
        }
    };

    if segment_keys.is_empty() {
        return;
    }
    let all_played = {
        let played = state.played_segments.lock().await;
        let Some((played_set, _)) = played.get(job_id) else {
            return;
        };
        segment_keys.iter().all(|k| played_set.contains(k))
    };
    if !all_played {
        return;
    }

    let Some(path) = pending_delete_source_path(state, source_path.as_deref()) else {
        state.played_segments.lock().await.remove(job_id);
        state.segment_meta_cache.lock().await.remove(job_id);
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
    state.segment_meta_cache.lock().await.remove(job_id);
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

pub(super) async fn bytes_for_re_upload(
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
