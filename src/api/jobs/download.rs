use std::path::{Path as FsPath, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use axum::body::{Body, Bytes};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio_stream::wrappers::ReceiverStream;

use super::super::{api_error, AppState};
use super::types::JobMetadata;
use crate::{db, telegram};

pub(super) async fn full_job_response(state: &AppState, job_id: &str) -> Response {
    let conn = state.db.lock().await;
    let job = match db::get_job(&conn, job_id) {
        Ok(Some(job)) => job,
        Ok(None) => return api_error(StatusCode::NOT_FOUND, "not_found", "job not found"),
        Err(e) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db_error", e.to_string()),
    };
    let tracks = match db::get_job_tracks(&conn, job_id, None) {
        Ok(tracks) => tracks,
        Err(e) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db_error", e.to_string()),
    };
    let segment_count = match db::count_job_segments(&conn, job_id) {
        Ok(count) => count,
        Err(e) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db_error", e.to_string()),
    };
    let audio_count = tracks.iter().filter(|t| t.track_type == "audio").count();
    let subtitle_count = tracks.iter().filter(|t| t.track_type == "subtitle").count();
    Json(json!({
        "job_id": job.job_id,
        "filename": job.filename,
        "duration": job.duration,
        "file_size": job.file_size,
        "video_codec": job.video_codec,
        "video_width": job.video_width,
        "video_height": job.video_height,
        "status": job.status,
        "media_type": job.media_type,
        "series_name": job.series_name,
        "is_series": job.is_series,
        "season_number": job.season_number,
        "episode_number": job.episode_number,
        "part_number": job.part_number,
        "has_thumbnail": job.has_thumbnail,
        "created_at": job.created_at,
        "audio_count": audio_count,
        "subtitle_count": subtitle_count,
        "segment_count": segment_count,
        "tracks": tracks,
    }))
    .into_response()
}

pub(super) async fn complete_job(state: &AppState, job_id: &str) -> Result<db::JobRow, Response> {
    let conn = state.db.lock().await;
    match db::get_job(&conn, job_id) {
        Ok(Some(job)) if job.status == "complete" => Ok(job),
        Ok(Some(_)) => Err(api_error(
            StatusCode::CONFLICT,
            "not_complete",
            "job is not complete",
        )),
        Ok(None) => Err(api_error(
            StatusCode::NOT_FOUND,
            "not_found",
            "job not found",
        )),
        Err(e) => Err(api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "db_error",
            e.to_string(),
        )),
    }
}

pub(super) async fn reconstruct_job_source(state: &AppState, job: &db::JobRow) -> Result<PathBuf> {
    let segments = {
        let conn = state.db.lock().await;
        db::get_segments_for_job(&conn, &job.job_id)?
    };
    let video_segments = prefix_segments(&segments, "video_0");
    if video_segments.is_empty() {
        bail!("job has no tier-0 video segments");
    }

    let stamp = unique_stamp();
    let base = std::env::temp_dir();
    let video_path = base.join(format!("thls_reconstruct_{stamp}_video.mp4"));
    let audio_path = base.join(format!("thls_reconstruct_{stamp}_audio.ts"));
    let output_path = base.join(format!("thls_reconstruct_{stamp}.mp4"));

    write_reconstructed_track(state, &video_segments, &video_path).await?;
    let audio_segments = prefix_segments(&segments, "audio_0");
    let has_audio = !audio_segments.is_empty();
    if has_audio {
        write_reconstructed_track(state, &audio_segments, &audio_path).await?;
    }

    let mut cmd = Command::new("ffmpeg");
    cmd.arg("-y").arg("-i").arg(&video_path);
    if has_audio {
        cmd.arg("-i").arg(&audio_path);
    }
    cmd.arg("-map").arg("0:v:0");
    if has_audio {
        cmd.arg("-map").arg("1:a:0?");
    }
    cmd.arg("-c")
        .arg("copy")
        .arg("-movflags")
        .arg("+faststart")
        .arg(&output_path);
    let output = cmd
        .output()
        .await
        .context("running ffmpeg reconstruction")?;
    let _ = tokio::fs::remove_file(&video_path).await;
    let _ = tokio::fs::remove_file(&audio_path).await;
    if !output.status.success() {
        let _ = tokio::fs::remove_file(&output_path).await;
        bail!(
            "ffmpeg reconstruction failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(output_path)
}

fn prefix_segments(segments: &[db::SegmentRow], prefix: &str) -> Vec<db::SegmentRow> {
    let prefix = format!("{prefix}/");
    let mut out: Vec<_> = segments
        .iter()
        .filter(|s| s.segment_key.starts_with(&prefix))
        .cloned()
        .collect();
    out.sort_by(|a, b| segment_sort_key(&a.segment_key).cmp(&segment_sort_key(&b.segment_key)));
    out
}

fn segment_sort_key(key: &str) -> (u8, &str) {
    if key.ends_with("/init.mp4") {
        (0, key)
    } else {
        (1, key)
    }
}

async fn write_reconstructed_track(
    state: &AppState,
    segments: &[db::SegmentRow],
    path: &FsPath,
) -> Result<()> {
    let mut file = tokio::fs::File::create(path).await?;
    for segment in segments {
        let bytes = fetch_segment_bytes(state, segment).await?;
        file.write_all(&bytes).await?;
    }
    file.flush().await?;
    Ok(())
}

async fn fetch_segment_bytes(state: &AppState, segment: &db::SegmentRow) -> Result<Vec<u8>> {
    let cfg = state.config.read().await.clone();
    telegram::get_file_bytes(
        &state.http,
        &state.telegram,
        &state.telegram_base_url,
        &cfg.bots,
        &segment.file_id,
        segment.bot_index,
    )
    .await
    .with_context(|| format!("fetching {}", segment.segment_key))
}

pub(super) async fn stream_temp_file(path: PathBuf, filename: &str) -> Response {
    let file = match tokio::fs::File::open(&path).await {
        Ok(file) => file,
        Err(e) => {
            let _ = tokio::fs::remove_file(path).await;
            return api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "download_failed",
                e.to_string(),
            );
        }
    };
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(8);
    let cleanup_path = path.clone();
    tokio::spawn(async move {
        let mut file = file;
        let mut buf = vec![0_u8; 256 * 1024];
        loop {
            match file.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    if tx
                        .send(Ok(Bytes::copy_from_slice(&buf[..n])))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Err(e) => {
                    let _ = tx.send(Err(e)).await;
                    break;
                }
            }
        }
        let _ = tokio::fs::remove_file(cleanup_path).await;
    });

    let mut response = Body::from_stream(ReceiverStream::new(rx)).into_response();
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static("video/mp4"));
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!("attachment; filename=\"{filename}\""))
            .unwrap_or_else(|_| HeaderValue::from_static("attachment")),
    );
    response
}

pub(super) fn metadata_from_job(job: &db::JobRow) -> JobMetadata {
    JobMetadata {
        media_type: Some(job.media_type.clone()),
        is_series: Some(job.is_series),
        series_name: Some(job.series_name.clone()),
        season_number: job.season_number.map(|v| v as i32),
        episode_number: job.episode_number.map(|v| v as i32),
        part_number: job.part_number.map(|v| v as i32),
        title: Some(job.filename.clone()),
        abr_tiers_override: None,
    }
}

pub(super) fn safe_download_name(name: &str) -> String {
    let stem = name.rsplit_once('.').map(|(stem, _)| stem).unwrap_or(name);
    let mut out = String::new();
    for ch in stem.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
            out.push(ch);
        } else if !out.ends_with('_') {
            out.push('_');
        }
    }
    if out.is_empty() {
        "video".into()
    } else {
        out
    }
}

fn unique_stamp() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}
