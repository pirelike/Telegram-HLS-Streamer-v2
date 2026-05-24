use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use super::super::AppState;
use super::types::*;
use crate::config::Config;
use crate::{db, media};

#[derive(Debug, Clone)]
pub(super) struct PreparedMarkerDetection {
    markers: Vec<db::NewMediaMarker>,
    fingerprints: Vec<media::GeneratedFingerprint>,
    media_type: String,
    series_name: String,
    season_number: Option<i64>,
}

pub(super) async fn prepare_marker_detection(
    state: &Arc<AppState>,
    request: &JobRequest,
    analysis: &media::MediaAnalysis,
    cfg: &Config,
    cancel: &Arc<AtomicBool>,
) -> Option<PreparedMarkerDetection> {
    if !cfg.intro_detection_enabled {
        return None;
    }
    let metadata = &request.metadata;
    let media_type = metadata.media_type.as_deref().unwrap_or("Film");
    let series_name = metadata.series_name.as_deref().unwrap_or("");
    let season_number = metadata.season_number.map(i64::from);

    let fingerprints = if series_name.is_empty() {
        Vec::new()
    } else {
        match state.db_conn().await {
            Ok(conn) => {
                let mt = media_type.to_string();
                let sn = series_name.to_string();
                let sn_copy = season_number;
                match tokio::task::spawn_blocking(move || {
                    let mut rows = db::get_media_fingerprints_for_series_window(
                        &conn, &mt, &sn, sn_copy, "intro",
                    )?;
                    rows.extend(db::get_media_fingerprints_for_series_window(
                        &conn, &mt, &sn, sn_copy, "outro",
                    )?);
                    Ok::<_, anyhow::Error>(rows)
                })
                .await
                {
                    Ok(Ok(fps)) => fps,
                    _ => Vec::new(),
                }
            }
            Err(e) => {
                tracing::warn!(job_id = %request.job_id, error = %e, "marker detection: db unavailable");
                return None;
            }
        }
    };

    let new_fingerprints = if cfg.intro_chromaprint_enabled && !series_name.is_empty() {
        if !media::chromaprint_available() {
            tracing::warn!(job_id = %request.job_id, "chromaprint fingerprint generation unavailable");
            Vec::new()
        } else {
            match media::generate_fingerprints(&request.source_path, analysis.duration, cancel)
                .await
            {
                Ok(fp) => fp,
                Err(e) => {
                    tracing::warn!(job_id = %request.job_id, error = %e, "chromaprint fingerprint generation skipped");
                    Vec::new()
                }
            }
        }
    } else {
        Vec::new()
    };

    match media::detect_markers(
        analysis,
        &fingerprints,
        &new_fingerprints,
        Some(&request.source_path),
        cancel,
    )
    .await
    {
        Ok(result) => Some(PreparedMarkerDetection {
            markers: result.markers,
            fingerprints: new_fingerprints,
            media_type: media_type.to_string(),
            series_name: series_name.to_string(),
            season_number,
        }),
        Err(e) => {
            tracing::warn!(job_id = %request.job_id, error = %e, "marker detection failed (non-fatal)");
            None
        }
    }
}

pub(super) async fn save_prepared_markers(
    state: &Arc<AppState>,
    request: &JobRequest,
    analysis: &media::MediaAnalysis,
    prepared: Option<PreparedMarkerDetection>,
) {
    let Some(prepared) = prepared else {
        return;
    };
    let jid = request.job_id.clone();
    if !prepared.markers.is_empty() {
        let markers = prepared.markers.clone();
        match state.db_conn().await {
            Ok(conn) => {
                let result = tokio::task::spawn_blocking(move || {
                    db::replace_auto_media_markers(&conn, &jid, &markers)
                })
                .await;
                if let Err(e) = result.unwrap_or_else(|e| Err(anyhow::anyhow!(e))) {
                    tracing::warn!(job_id = %request.job_id, error = %e, "marker save failed");
                }
            }
            Err(e) => {
                tracing::warn!(job_id = %request.job_id, error = %e, "marker save: db unavailable")
            }
        }
    }

    if prepared.fingerprints.is_empty() {
        return;
    }
    let fingerprint_entries: Vec<_> = prepared
        .fingerprints
        .iter()
        .map(|fp| db::NewMediaFingerprint {
            job_id: request.job_id.clone(),
            media_type: prepared.media_type.clone(),
            series_name: prepared.series_name.clone(),
            season_number: prepared.season_number,
            window_type: fp.window_type.clone(),
            window_start_seconds: fp.window_start_seconds,
            window_duration_seconds: fp.window_duration_seconds,
            duration_seconds: analysis.duration,
            fingerprint: fp.fingerprint.clone(),
            fingerprint_source: "chromaprint".to_string(),
        })
        .collect();
    match state.db_conn().await {
        Ok(conn) => {
            let result = tokio::task::spawn_blocking(move || {
                for entry in &fingerprint_entries {
                    db::save_media_fingerprint(&conn, entry)?;
                }
                Ok::<_, anyhow::Error>(())
            })
            .await;
            if let Err(e) = result.unwrap_or_else(|e| Err(anyhow::anyhow!(e))) {
                tracing::warn!(job_id = %request.job_id, error = %e, "fingerprint save failed");
            }
        }
        Err(e) => {
            tracing::warn!(job_id = %request.job_id, error = %e, "fingerprint save: db unavailable")
        }
    }
}

pub(super) async fn auto_fetch_metadata_if_enabled(state: &Arc<AppState>, request: &JobRequest) {
    let cfg = state.config.read().await.clone();
    if !cfg.metadata_auto_fetch_enabled {
        return;
    }

    // Skip if already linked.
    let conn = match state.db_conn().await {
        Ok(c) => c,
        Err(_) => return,
    };
    let jid = request.job_id.clone();
    let already_linked = tokio::task::spawn_blocking(move || {
        db::get_job_metadata_links(&conn, &jid).map(|l| !l.is_empty())
    })
    .await
    .ok()
    .and_then(|r| r.ok())
    .unwrap_or(false);
    if already_linked {
        return;
    }

    let media_type = request.metadata.media_type.as_deref().unwrap_or("Film");
    let search_term = request
        .metadata
        .series_name
        .as_deref()
        .filter(|s| !s.is_empty())
        .or_else(|| request.metadata.title.as_deref().filter(|s| !s.is_empty()))
        .unwrap_or("");
    if search_term.is_empty() {
        return;
    }

    super::super::metadata::auto_fetch_and_link(state, &request.job_id, search_term, media_type)
        .await;
}
