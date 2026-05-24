use std::collections::{HashMap, HashSet};
use std::path::{Path as FsPath, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use super::super::AppState;
use super::types::*;
use crate::config::{BotConfig, Config};
use crate::{db, media, telegram};

pub(super) async fn upload_outputs(
    state: Arc<AppState>,
    cfg: &Config,
    result: &media::ProcessingResult,
    cancel_flag: &Arc<AtomicBool>,
) -> Result<(Vec<telegram::UploadedFile>, Option<i64>)> {
    // cfg.telegram_max_file_size is user-configurable; raise if Telegram increases Bot API limits.
    let files = prepare_upload_files(
        &result.output_dir,
        cfg.telegram_max_file_size,
        cfg.telegram_encryption_key.is_some(),
    )
    .await?;
    if files.is_empty() {
        bail!("no uploadable HLS output files found");
    }
    let total = files.len();
    tracing::info!(
        job_id = %result.job_id,
        files = total,
        upload_parallelism = cfg.upload_parallelism,
        "telegram output upload batch prepared"
    );
    let assignments = assign_upload_bots(&state, files.len()).await?;
    let last_upload_bot_index = assignments.last().map(|(index, _)| *index);
    let parallelism = cfg
        .upload_parallelism
        .max(1)
        .min(assignments.len() as u32)
        .min(files.len() as u32) as usize;
    let semaphore = Arc::new(Semaphore::new(parallelism));
    let mut tasks = JoinSet::new();

    for ((segment_key, path), (bot_index, bot)) in files.into_iter().zip(assignments) {
        if cancel_flag.load(Ordering::Relaxed) {
            tasks.abort_all();
            bail!("cancelled");
        }
        let permit = semaphore.clone().acquire_owned().await?;
        let state = state.clone();
        let base_url = state.telegram_base_url.clone();
        let client = state.http.clone();
        let max_file_size = cfg.telegram_max_file_size;
        let encryption_key = cfg.telegram_encryption_key.clone();
        let cancel_flag_clone = cancel_flag.clone();
        tasks.spawn(async move {
            let _permit = permit;
            if cancel_flag_clone.load(Ordering::Relaxed) {
                return Err(anyhow::anyhow!("cancelled"));
            }
            telegram::upload_document(
                &client,
                &state.telegram,
                &base_url,
                bot,
                bot_index,
                &path,
                segment_key,
                encryption_key.as_ref(),
                max_file_size,
            )
            .await
        });
    }

    let mut uploaded = Vec::new();
    while let Some(task_result) = tasks.join_next().await {
        if cancel_flag.load(Ordering::Relaxed) {
            tasks.abort_all();
            bail!("cancelled");
        }
        uploaded.push(task_result.context("upload task panicked")??);
        if let Some(last) = uploaded.last() {
            tracing::info!(
                job_id = %result.job_id,
                segment_key = %last.segment_key,
                file_size = last.file_size,
                bot_index = last.bot_index,
                uploaded = uploaded.len(),
                total,
                "telegram output file uploaded"
            );
        }
        update_upload_progress(&state, &result.job_id, uploaded.len(), total).await;
    }
    uploaded.sort_by(|a, b| a.segment_key.cmp(&b.segment_key));
    Ok((uploaded, last_upload_bot_index))
}

pub(super) async fn update_upload_progress(
    state: &AppState,
    job_id: &str,
    current: usize,
    total: usize,
) {
    let mut jobs = state.jobs.lock().await;
    let Some(job) = jobs.get_mut(job_id) else {
        return;
    };
    if job.status != JobStatus::Uploading || total == 0 {
        return;
    }
    let pct = current as f64 / total as f64;
    job.progress = 80.0 + pct * 18.0;
    job.description = format!("uploading to Telegram ({current}/{total})");
}

pub(super) async fn split_file_for_upload(
    path: &PathBuf,
    segment_key: &str,
    max_plaintext_size: u64,
    temp_dir: &FsPath,
) -> Result<Vec<(String, PathBuf)>> {
    let file_size = tokio::fs::metadata(path)
        .await
        .with_context(|| format!("stat {}", path.display()))?
        .len();

    if file_size <= max_plaintext_size {
        return Ok(vec![(segment_key.to_string(), path.clone())]);
    }

    let chunk_size = (max_plaintext_size.saturating_mul(95) / 100).max(1);
    let total_parts = ((file_size - 1) / chunk_size + 1) as i64;
    let mut file = tokio::fs::File::open(path)
        .await
        .with_context(|| format!("open {}", path.display()))?;
    let mut parts = Vec::new();

    tracing::info!(
        segment_key,
        file_size,
        chunk_size,
        total_parts,
        "splitting oversized segment"
    );

    for part_index in 0..total_parts {
        let part_key = format!("{}/part_{}", segment_key, part_index);
        let part_path = temp_dir.join(part_index.to_string());
        let offset = part_index as u64 * chunk_size;
        let mut remaining = (file_size - offset).min(chunk_size);
        let mut part_file = tokio::fs::File::create(&part_path)
            .await
            .with_context(|| format!("create part {}", part_path.display()))?;
        let mut buf = vec![0_u8; 256 * 1024];
        while remaining > 0 {
            let want = remaining.min(buf.len() as u64) as usize;
            let n = file
                .read(&mut buf[..want])
                .await
                .with_context(|| format!("read part {} from {}", part_index, path.display()))?;
            if n == 0 {
                bail!("unexpected EOF splitting {}", path.display());
            }
            part_file
                .write_all(&buf[..n])
                .await
                .with_context(|| format!("write part {}", part_path.display()))?;
            remaining -= n as u64;
        }
        part_file
            .flush()
            .await
            .with_context(|| format!("flush part {}", part_path.display()))?;
        parts.push((part_key, part_path));
    }

    Ok(parts)
}

pub(super) async fn collect_upload_files(output_dir: &FsPath) -> Result<Vec<(String, PathBuf)>> {
    let mut out = Vec::new();
    let mut dirs = tokio::fs::read_dir(output_dir).await?;
    while let Some(dir) = dirs.next_entry().await? {
        if !dir.file_type().await?.is_dir() {
            continue;
        }
        let prefix = dir.file_name().to_string_lossy().to_string();
        let mut files = tokio::fs::read_dir(dir.path()).await?;
        while let Some(file) = files.next_entry().await? {
            if !file.file_type().await?.is_file() {
                continue;
            }
            let name = file.file_name().to_string_lossy().to_string();
            let path = file.path();
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if name != "init.mp4" && !matches!(ext, "m4s" | "ts" | "vtt" | "jpg") {
                continue;
            }
            out.push((format!("{prefix}/{name}"), path));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

pub(super) async fn prepare_upload_files(
    output_dir: &FsPath,
    // User-configurable; matches cfg.telegram_max_file_size. Raise if Telegram changes limits.
    max_file_size: u64,
    encrypted: bool,
) -> Result<Vec<(String, PathBuf)>> {
    let files = collect_upload_files(output_dir).await?;
    if files.is_empty() {
        return Ok(files);
    }
    let max_plaintext_size = crate::crypto::max_plaintext_size(max_file_size, encrypted)?;

    let temp_dir = output_dir.join("temp_parts");
    tokio::fs::create_dir_all(&temp_dir)
        .await
        .with_context(|| format!("create temp dir {}", temp_dir.display()))?;

    let mut result = Vec::new();
    for (segment_key, path) in files {
        let file_size = tokio::fs::metadata(&path)
            .await
            .with_context(|| format!("stat {}", path.display()))?
            .len();

        if file_size <= max_plaintext_size {
            result.push((segment_key, path));
        } else {
            let parts_dir = temp_dir.join(segment_key.replace('/', "_"));
            tokio::fs::create_dir_all(&parts_dir)
                .await
                .with_context(|| format!("create parts dir {}", parts_dir.display()))?;
            let parts =
                split_file_for_upload(&path, &segment_key, max_plaintext_size, &parts_dir).await?;
            result.extend(parts);
        }
    }
    result.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(result)
}

pub(super) async fn assign_upload_bots(
    state: &AppState,
    file_count: usize,
) -> Result<Vec<(i64, BotConfig)>> {
    let cfg = state.config.read().await.clone();
    if cfg.bots.is_empty() {
        bail!("no Telegram bots configured");
    }

    // Collect unhealthy bot indices
    let unhealthy: HashSet<i64> = {
        let mut set = HashSet::new();
        for i in 0..cfg.bots.len() as i64 {
            if !state.telegram.is_bot_healthy(i).await {
                set.insert(i);
            }
        }
        set
    };
    let all_unhealthy = unhealthy.len() == cfg.bots.len();

    let last = state
        .last_bot_index
        .fetch_add(file_count as i64, std::sync::atomic::Ordering::Relaxed);
    let mut assignments = Vec::with_capacity(file_count);
    let bot_count = cfg.bots.len() as i64;
    for offset in 0..file_count {
        let mut index = (last + 1 + offset as i64).rem_euclid(bot_count);
        // Skip unhealthy bots unless all bots are unhealthy (avoid deadlock)
        if !all_unhealthy {
            while unhealthy.contains(&index) {
                index = (index + 1).rem_euclid(bot_count);
            }
        }
        assignments.push((index, cfg.bots[index as usize].clone()));
    }
    Ok(assignments)
}

pub(super) fn build_db_rows(
    request: &JobRequest,
    analysis: &media::MediaAnalysis,
    result: &media::ProcessingResult,
    uploads: Vec<telegram::UploadedFile>,
) -> (
    db::NewJob,
    Vec<db::NewTrack>,
    Vec<db::NewSegment>,
    Vec<db::NewSegmentPart>,
) {
    let video = analysis.video_streams.first();
    let metadata = &request.metadata;
    let is_series = metadata.is_series.unwrap_or(false);
    let media_type = metadata
        .media_type
        .clone()
        .unwrap_or_else(|| if is_series { "Series" } else { "Film" }.into());
    let mut job = db::NewJob {
        job_id: request.job_id.clone(),
        filename: metadata
            .title
            .clone()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| request.filename.clone()),
        duration: analysis.duration,
        file_size: analysis.file_size as i64,
        video_codec: video
            .map(|v| v.codec_name.clone())
            .unwrap_or_else(|| "unknown".into()),
        video_width: video.map(|v| v.width).unwrap_or(0),
        video_height: video.map(|v| v.height).unwrap_or(0),
        status: "complete".into(),
        media_type,
        series_name: metadata.series_name.clone().unwrap_or_default(),
        has_thumbnail: result.thumbnail_path.is_some(),
        is_series,
        season_number: metadata.season_number.map(i64::from),
        episode_number: metadata.episode_number.map(i64::from),
        part_number: metadata.part_number.map(i64::from),
        source_path: request.original_source_path.clone(),
        source_bitrate: video
            .map(|v| v.bit_rate.trim().parse::<i64>().unwrap_or(0))
            .unwrap_or(0),
    };
    if !job.is_series {
        job.series_name.clear();
    }

    let mut tracks = Vec::new();
    let source_video_index = video.map(|v| v.index).unwrap_or(-1);
    for (idx, playlist) in result.video_playlists.iter().enumerate() {
        tracks.push(db::NewTrack {
            track_type: "video".into(),
            track_index: idx as i64,
            codec: if playlist.bitrate == "copy" {
                job.video_codec.clone()
            } else {
                "h264".into()
            },
            language: "und".into(),
            title: String::new(),
            channels: 0,
            width: playlist.width,
            height: playlist.height,
            bitrate: playlist.bitrate.clone(),
            original_stream_index: source_video_index,
        });
    }
    for (idx, playlist) in result.audio_playlists.iter().enumerate() {
        let source = analysis.audio_streams.get(idx);
        tracks.push(db::NewTrack {
            track_type: "audio".into(),
            track_index: idx as i64,
            codec: "aac".into(),
            language: playlist.language.clone(),
            title: playlist.title.clone(),
            channels: playlist.channels,
            width: 0,
            height: 0,
            bitrate: String::new(),
            original_stream_index: source.map(|a| a.index).unwrap_or(-1),
        });
    }
    for sub in &result.subtitle_files {
        tracks.push(db::NewTrack {
            track_type: "subtitle".into(),
            track_index: sub.enum_idx as i64,
            codec: "webvtt".into(),
            language: sub.language.clone(),
            title: sub.title.clone(),
            channels: 0,
            width: 0,
            height: 0,
            bitrate: String::new(),
            original_stream_index: sub.original_stream_idx,
        });
    }

    let mut segments = Vec::new();
    let mut segment_parts = Vec::new();
    let mut split_segments: HashMap<String, (i64, i64)> = HashMap::new();

    for uploaded in uploads {
        if uploaded.segment_key.contains("/part_") {
            let parts: Vec<&str> = uploaded.segment_key.split("/part_").collect();
            if parts.len() == 2 {
                let logical_key = parts[0].to_string();
                let part_index: i64 = parts[1].parse().unwrap_or(0);
                let entry = split_segments
                    .entry(logical_key.clone())
                    .or_insert((0, uploaded.bot_index));
                entry.0 += uploaded.file_size as i64;
                segment_parts.push(db::NewSegmentPart {
                    job_id: job.job_id.clone(),
                    segment_key: logical_key,
                    part_index,
                    file_id: uploaded.file_id,
                    bot_index: uploaded.bot_index,
                    file_size: uploaded.file_size as i64,
                    encryption_nonce: uploaded.encryption_nonce,
                });
            }
        } else {
            let duration = if uploaded.segment_key.ends_with("/init.mp4") {
                None
            } else {
                result.segment_durations.get(&uploaded.segment_key).copied()
            };
            segments.push(db::NewSegment {
                segment_key: uploaded.segment_key,
                file_id: uploaded.file_id,
                bot_index: uploaded.bot_index,
                file_size: uploaded.file_size as i64,
                duration,
                is_split: false,
                encryption_nonce: uploaded.encryption_nonce,
            });
        }
    }

    for (segment_key, (file_size, bot_index)) in split_segments {
        segments.push(db::NewSegment {
            duration: result.segment_durations.get(&segment_key).copied(),
            segment_key,
            file_id: String::new(),
            bot_index,
            file_size,
            is_split: true,
            encryption_nonce: None,
        });
    }

    (job, tracks, segments, segment_parts)
}
