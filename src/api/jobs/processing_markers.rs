use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use super::super::AppState;
use super::types::*;
use crate::config::Config;
use crate::{db, media};
use serde_json::{json, Value};

#[derive(Debug, Clone)]
pub(super) struct PreparedMarkerDetection {
    markers: Vec<db::NewMediaMarker>,
    fingerprints: Vec<media::GeneratedFingerprint>,
    media_type: String,
    series_name: String,
    season_number: Option<i64>,
    existing_fingerprint_count: usize,
}

pub(super) async fn prepare_marker_detection(
    state: &Arc<AppState>,
    request: &JobRequest,
    analysis: &media::MediaAnalysis,
    cfg: &Config,
    cancel: &Arc<AtomicBool>,
) -> Option<PreparedMarkerDetection> {
    if !cfg.intro_detection_enabled {
        tracing::info!(
            job_id = %request.job_id,
            filename = %request.filename,
            "marker detection skipped: disabled"
        );
        write_marker_audit(
            state,
            request,
            "skipped",
            "intro_detection_disabled",
            json!({}),
        )
        .await;
        return None;
    }
    let metadata = &request.metadata;
    let media_type = metadata.media_type.as_deref().unwrap_or("Film");
    let series_name = metadata.series_name.as_deref().unwrap_or("");
    let season_number = metadata.season_number.map(i64::from);
    let episode_number = metadata.episode_number.map(i64::from);

    let fingerprints = if series_name.is_empty() {
        tracing::info!(
            job_id = %request.job_id,
            filename = %request.filename,
            media_type,
            series_name,
            season_number,
            episode_number,
            "marker detection has no series name; chromaprint matching will be skipped"
        );
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
                    Ok(Ok(fps)) => {
                        tracing::info!(
                            job_id = %request.job_id,
                            filename = %request.filename,
                            media_type,
                            series_name,
                            season_number,
                            episode_number,
                            existing_fingerprints = fps.len(),
                            "marker detection loaded existing fingerprints"
                        );
                        fps
                    }
                    Ok(Err(e)) => {
                        tracing::warn!(
                            job_id = %request.job_id,
                            error = %e,
                            "marker detection: failed to load existing fingerprints"
                        );
                        Vec::new()
                    }
                    Err(e) => {
                        tracing::warn!(
                            job_id = %request.job_id,
                            error = %e,
                            "marker detection: fingerprint load task failed"
                        );
                        Vec::new()
                    }
                }
            }
            Err(e) => {
                tracing::warn!(job_id = %request.job_id, error = %e, "marker detection: db unavailable");
                write_marker_audit(
                    state,
                    request,
                    "skipped",
                    "db_unavailable_loading_fingerprints",
                    json!({ "error": e.to_string() }),
                )
                .await;
                return None;
            }
        }
    };

    let has_audio = analysis.audio_streams.iter().any(|s| s.channels > 0);
    let new_fingerprints = if cfg.intro_chromaprint_enabled && !series_name.is_empty() && has_audio
    {
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
        if !has_audio {
            tracing::info!(
                job_id = %request.job_id,
                filename = %request.filename,
                "chromaprint marker generation skipped: no audio streams"
            );
        } else if !cfg.intro_chromaprint_enabled {
            tracing::info!(
                job_id = %request.job_id,
                filename = %request.filename,
                media_type,
                series_name,
                season_number,
                episode_number,
                "chromaprint marker generation skipped: disabled"
            );
        }
        Vec::new()
    };
    tracing::info!(
        job_id = %request.job_id,
        filename = %request.filename,
        media_type,
        series_name,
        season_number,
        episode_number,
        existing_fingerprints = fingerprints.len(),
        generated_fingerprints = new_fingerprints.len(),
        "marker detection inputs ready"
    );

    match media::detect_markers(
        analysis,
        &fingerprints,
        &new_fingerprints,
        Some(&request.source_path),
        cancel,
    )
    .await
    {
        Ok(result) => {
            tracing::info!(
                job_id = %request.job_id,
                filename = %request.filename,
                media_type,
                series_name,
                season_number,
                episode_number,
                existing_fingerprints = fingerprints.len(),
                generated_fingerprints = new_fingerprints.len(),
                markers = result.markers.len(),
                "marker detection complete"
            );
            Some(PreparedMarkerDetection {
                markers: result.markers,
                fingerprints: new_fingerprints,
                media_type: media_type.to_string(),
                series_name: series_name.to_string(),
                season_number,
                existing_fingerprint_count: fingerprints.len(),
            })
        }
        Err(e) => {
            tracing::warn!(job_id = %request.job_id, error = %e, "marker detection failed (non-fatal)");
            write_marker_audit(
                state,
                request,
                "failed",
                "detection_failed",
                json!({
                    "media_type": media_type,
                    "series_name": series_name,
                    "season_number": season_number,
                    "episode_number": episode_number,
                    "existing_fingerprints": fingerprints.len(),
                    "generated_fingerprints": new_fingerprints.len(),
                    "error": e.to_string(),
                }),
            )
            .await;
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
    let mut saved_markers = 0usize;
    let mut saved_fingerprints = 0usize;
    let mut save_errors = Vec::new();
    if !prepared.markers.is_empty() {
        let markers = prepared.markers.clone();
        match state.db_conn().await {
            Ok(conn) => {
                let marker_count = markers.len();
                let result = tokio::task::spawn_blocking(move || {
                    db::replace_auto_media_markers(&conn, &jid, &markers)
                })
                .await;
                if let Err(e) = result.unwrap_or_else(|e| Err(anyhow::anyhow!(e))) {
                    tracing::warn!(job_id = %request.job_id, error = %e, "marker save failed");
                    save_errors.push(format!("marker_save_failed: {e}"));
                } else {
                    saved_markers = marker_count;
                    tracing::info!(
                        job_id = %request.job_id,
                        filename = %request.filename,
                        media_type = %prepared.media_type,
                        series_name = %prepared.series_name,
                        season_number = prepared.season_number,
                        episode_number = request.metadata.episode_number.map(i64::from),
                        saved_markers,
                        "marker save complete"
                    );
                }
            }
            Err(e) => {
                tracing::warn!(job_id = %request.job_id, error = %e, "marker save: db unavailable");
                save_errors.push(format!("marker_save_db_unavailable: {e}"));
            }
        }
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
    if !fingerprint_entries.is_empty() {
        match state.db_conn().await {
            Ok(conn) => {
                let fingerprint_count = fingerprint_entries.len();
                let result = tokio::task::spawn_blocking(move || {
                    for entry in &fingerprint_entries {
                        db::save_media_fingerprint(&conn, entry)?;
                    }
                    Ok::<_, anyhow::Error>(())
                })
                .await;
                if let Err(e) = result.unwrap_or_else(|e| Err(anyhow::anyhow!(e))) {
                    tracing::warn!(job_id = %request.job_id, error = %e, "fingerprint save failed");
                    save_errors.push(format!("fingerprint_save_failed: {e}"));
                } else {
                    saved_fingerprints = fingerprint_count;
                    tracing::info!(
                        job_id = %request.job_id,
                        filename = %request.filename,
                        media_type = %prepared.media_type,
                        series_name = %prepared.series_name,
                        season_number = prepared.season_number,
                        episode_number = request.metadata.episode_number.map(i64::from),
                        saved_fingerprints,
                        "marker fingerprint save complete"
                    );
                }
            }
            Err(e) => {
                tracing::warn!(job_id = %request.job_id, error = %e, "fingerprint save: db unavailable");
                save_errors.push(format!("fingerprint_save_db_unavailable: {e}"));
            }
        }
    }
    let status = if save_errors.is_empty() {
        "saved"
    } else {
        "save_failed"
    };
    write_marker_audit(
        state,
        request,
        status,
        if save_errors.is_empty() {
            "completed"
        } else {
            "save_error"
        },
        json!({
            "media_type": prepared.media_type,
            "series_name": prepared.series_name,
            "season_number": prepared.season_number,
            "episode_number": request.metadata.episode_number.map(i64::from),
            "existing_fingerprints": prepared.existing_fingerprint_count,
            "generated_fingerprints": prepared.fingerprints.len(),
            "markers": prepared.markers.len(),
            "saved_markers": saved_markers,
            "saved_fingerprints": saved_fingerprints,
            "errors": save_errors,
        }),
    )
    .await;
}

fn marker_audit_key(job_id: &str) -> String {
    format!("_marker_detection:{job_id}")
}

fn marker_audit_payload(
    request: &JobRequest,
    status: &str,
    reason: &str,
    details: Value,
) -> String {
    json!({
        "status": status,
        "reason": reason,
        "job_id": request.job_id,
        "filename": request.filename,
        "media_type": request.metadata.media_type,
        "series_name": request.metadata.series_name,
        "season_number": request.metadata.season_number,
        "episode_number": request.metadata.episode_number,
        "details": details,
    })
    .to_string()
}

async fn write_marker_audit(
    state: &Arc<AppState>,
    request: &JobRequest,
    status: &str,
    reason: &str,
    details: Value,
) {
    let key = marker_audit_key(&request.job_id);
    let value = marker_audit_payload(request, status, reason, details);
    match state.db_conn().await {
        Ok(conn) => {
            let result =
                tokio::task::spawn_blocking(move || db::set_internal_value(&conn, &key, &value))
                    .await;
            if let Err(e) = result.unwrap_or_else(|e| Err(anyhow::anyhow!(e))) {
                tracing::warn!(job_id = %request.job_id, error = %e, "marker audit save failed");
            }
        }
        Err(e) => {
            tracing::warn!(job_id = %request.job_id, error = %e, "marker audit save: db unavailable");
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
