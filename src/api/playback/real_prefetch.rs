use std::sync::Arc;

use super::AppState;

use crate::db;

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
                        })
                        .await
                        .unwrap_or_else(|e| Err(anyhow::anyhow!(e))),
                        Err(e) => Err(e),
                    }
                };
                match parts {
                    Ok(parts) => {
                        super::real_fetch::fetch_reconstructed_with_singleflight(
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
                super::real_fetch::fetch_real_with_singleflight(
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
                        })
                        .await
                        .unwrap_or_else(|e| Err(anyhow::anyhow!(e))),
                        Err(e) => Err(e),
                    }
                };
                match parts_result {
                    Ok(parts) => {
                        super::real_fetch::fetch_reconstructed_with_singleflight(
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
                super::real_fetch::fetch_real_with_singleflight(
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
