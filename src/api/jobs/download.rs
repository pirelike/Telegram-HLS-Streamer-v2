use std::io::SeekFrom;
use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use axum::body::{Body, Bytes};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::process::Command;
use tokio::task::JoinSet;
use tokio_stream::wrappers::ReceiverStream;

use super::super::{api_error, db_unavailable, AppState};
use super::types::JobMetadata;
use crate::config::BotConfig;
use crate::{db, telegram};

pub(super) async fn full_job_response(state: &AppState, job_id: &str) -> Response {
    let conn = match state.db_conn().await {
        Ok(conn) => conn,
        Err(e) => return db_unavailable(e),
    };
    let job_id_clone = job_id.to_string();
    let db_result = tokio::task::spawn_blocking(move || {
        let job = db::get_job(&conn, &job_id_clone)?;
        let tracks = db::get_job_tracks(&conn, &job_id_clone, None)?;
        let segment_count = db::count_job_segments(&conn, &job_id_clone)?;
        Ok::<(Option<db::JobRow>, Vec<db::TrackRow>, i64), anyhow::Error>((
            job,
            tracks,
            segment_count,
        ))
    })
    .await;

    let (job_opt, tracks, segment_count) = match db_result {
        Ok(Ok(val)) => val,
        Ok(Err(e)) => {
            return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db_error", e.to_string())
        }
        Err(e) => {
            return api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "blocking_error",
                e.to_string(),
            )
        }
    };

    let Some(job) = job_opt else {
        return api_error(StatusCode::NOT_FOUND, "not_found", "job not found");
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
    let conn = state.db_conn().await.map_err(db_unavailable)?;
    let job_id_clone = job_id.to_string();
    let db_result = tokio::task::spawn_blocking(move || db::get_job(&conn, &job_id_clone)).await;

    match db_result {
        Ok(Ok(Some(job))) if job.status == "complete" => Ok(job),
        Ok(Ok(Some(_))) => Err(api_error(
            StatusCode::CONFLICT,
            "not_complete",
            "job is not complete",
        )),
        Ok(Ok(None)) => Err(api_error(
            StatusCode::NOT_FOUND,
            "not_found",
            "job not found",
        )),
        Ok(Err(e)) => Err(api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "db_error",
            e.to_string(),
        )),
        Err(e) => Err(api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "blocking_error",
            e.to_string(),
        )),
    }
}

pub(super) async fn reconstruct_job_source(
    state: &Arc<AppState>,
    job: &db::JobRow,
) -> Result<PathBuf> {
    let segments = {
        let conn = state
            .db_conn()
            .await
            .context("getting sqlite connection for reconstruction")?;
        let job_id_clone = job.job_id.clone();
        tokio::task::spawn_blocking(move || db::get_segments_for_job(&conn, &job_id_clone))
            .await??
    };
    let video_segments = prefix_segments(&segments, "video_0");
    if video_segments.is_empty() {
        bail!("job has no tier-0 video segments");
    }

    let stamp = uuid::Uuid::new_v4().simple().to_string();
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
    state: &Arc<AppState>,
    segments: &[db::SegmentRow],
    path: &FsPath,
) -> Result<()> {
    let downloads = reconstruct_downloads(state, segments).await?;
    let total_size = downloads
        .iter()
        .try_fold(0_u64, |total, item| total.checked_add(item.expected_size))
        .context("reconstructed track is too large")?;

    let file = tokio::fs::File::create(path).await?;
    file.set_len(total_size).await?;
    drop(file);

    let cfg = state.config.read().await.clone();
    let bots = Arc::new(cfg.bots.clone());
    let parallelism = cfg.upload_parallelism.max(1) as usize;
    let mut pending = downloads.into_iter();
    let mut tasks = JoinSet::new();

    loop {
        while tasks.len() < parallelism {
            let Some(item) = pending.next() else {
                break;
            };
            let state = state.clone();
            let bots = bots.clone();
            let path = path.to_path_buf();
            tasks.spawn(async move { stream_download_to_path_at(state, bots, path, item).await });
        }

        let Some(result) = tasks.join_next().await else {
            break;
        };
        result.context("reconstruction download task panicked")??;
    }

    Ok(())
}

#[derive(Debug)]
struct ReconstructDownload {
    label: String,
    file_id: String,
    bot_index: i64,
    offset: u64,
    expected_size: u64,
}

async fn reconstruct_downloads(
    state: &AppState,
    segments: &[db::SegmentRow],
) -> Result<Vec<ReconstructDownload>> {
    let conn = state
        .db_conn()
        .await
        .context("getting connection for reconstruct downloads")?;
    let segments_vec = segments.to_vec();
    tokio::task::spawn_blocking(move || {
        let mut downloads = Vec::new();
        let mut offset = 0_u64;
        for segment in segments_vec {
            if segment.is_split {
                let parts = db::get_segment_parts(&conn, &segment.job_id, &segment.segment_key)
                    .with_context(|| {
                        format!("getting parts for split segment {}", segment.segment_key)
                    })?;
                for (part_index, part) in parts.into_iter().enumerate() {
                    let expected_size = u64::try_from(part.file_size).with_context(|| {
                        format!("invalid part size for {}", segment.segment_key)
                    })?;
                    downloads.push(ReconstructDownload {
                        label: format!("{} part {part_index}", segment.segment_key),
                        file_id: part.file_id,
                        bot_index: part.bot_index,
                        offset,
                        expected_size,
                    });
                    offset = offset
                        .checked_add(expected_size)
                        .context("reconstructed track is too large")?;
                }
            } else {
                let expected_size = u64::try_from(segment.file_size)
                    .with_context(|| format!("invalid segment size for {}", segment.segment_key))?;
                downloads.push(ReconstructDownload {
                    label: segment.segment_key.clone(),
                    file_id: segment.file_id.clone(),
                    bot_index: segment.bot_index,
                    offset,
                    expected_size,
                });
                offset = offset
                    .checked_add(expected_size)
                    .context("reconstructed track is too large")?;
            }
        }
        Ok::<Vec<ReconstructDownload>, anyhow::Error>(downloads)
    })
    .await?
}

async fn stream_download_to_path_at(
    state: Arc<AppState>,
    bots: Arc<Vec<BotConfig>>,
    path: PathBuf,
    item: ReconstructDownload,
) -> Result<()> {
    let started = std::time::Instant::now();
    let resp = telegram::get_file_response(
        &state.http,
        &state.telegram,
        &state.telegram_base_url,
        &bots,
        &item.file_id,
        item.bot_index,
    )
    .await
    .with_context(|| format!("fetching {}", item.label))?;

    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .await
        .with_context(|| format!("open {}", path.display()))?;
    file.seek(SeekFrom::Start(item.offset))
        .await
        .with_context(|| format!("seek {}", path.display()))?;

    let mut written = 0_u64;
    use tokio_stream::StreamExt as _;
    let stream = resp.bytes_stream();
    tokio::pin!(stream);
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.with_context(|| format!("streaming {}", item.label))?;
        file.write_all(&chunk)
            .await
            .with_context(|| format!("write {}", path.display()))?;
        written += chunk.len() as u64;
    }
    file.flush()
        .await
        .with_context(|| format!("flush {}", path.display()))?;
    if written != item.expected_size {
        bail!(
            "download_size_mismatch: {} expected={} wrote={}",
            item.label,
            item.expected_size,
            written
        );
    }
    state
        .telegram
        .record_download_success(item.bot_index, written, started.elapsed().as_secs_f64())
        .await;
    Ok(())
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
