use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use tokio::process::Command;
use tokio::sync::Semaphore;

use super::encoder::{add_encoder_device_args, add_forced_idr_args, video_filter};
use super::models::*;
use super::process::target_segment_seconds_for_tier;
use super::process_probe::parse_hls_segment_durations;
use super::tiers::tier0_bitrate;
use crate::config::Config;

pub(super) async fn encode_video_tier(
    analysis: &MediaAnalysis,
    video: &VideoStream,
    tier: &VideoTier,
    encoder: &SelectedEncoder,
    cfg: &Config,
    dir: &Path,
    cancel: &Arc<AtomicBool>,
    encode_semaphore: &Arc<Semaphore>,
) -> Result<usize> {
    let ts_dir = dir.join("ts_work");
    tokio::fs::create_dir_all(&ts_dir).await?;
    let permit = super::ffmpeg::acquire_ffmpeg_permit(encode_semaphore, cancel).await?;
    encode_video_tier_ts(analysis, video, tier, encoder, cfg, &ts_dir, cancel).await?;
    drop(permit);
    let copy_target_secs = target_segment_seconds_for_tier(cfg, analysis, tier);
    let effective_tier =
        if tier.copy && copied_segments_need_reencode(&ts_dir, copy_target_secs).await? {
            tracing::warn!(
                tier = tier.index,
                "copy-mode video tier has poorly aligned segments; re-encoding whole tier"
            );
            tokio::fs::remove_dir_all(&ts_dir).await?;
            tokio::fs::create_dir_all(&ts_dir).await?;
            let reencode_tier = VideoTier {
                index: tier.index,
                height: tier.height,
                bitrate: tier0_bitrate(cfg, video.height),
                copy: false,
            };
            let permit = super::ffmpeg::acquire_ffmpeg_permit(encode_semaphore, cancel).await?;
            encode_video_tier_ts(
                analysis,
                video,
                &reencode_tier,
                encoder,
                cfg,
                &ts_dir,
                cancel,
            )
            .await?;
            drop(permit);
            reencode_tier
        } else {
            tier.clone()
        };
    let repair_target_secs = target_segment_seconds_for_tier(cfg, analysis, &effective_tier);
    repair_oversized_video_segments(
        &ts_dir,
        &effective_tier,
        encoder,
        cfg,
        repair_target_secs,
        cancel,
        encode_semaphore,
    )
    .await?;
    remux_video_ts_to_fmp4(&ts_dir, cfg, dir, cancel).await?;

    let mut oversized_m4s = Vec::new();
    {
        let mut entries = tokio::fs::read_dir(dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("m4s") {
                continue;
            }
            let size = entry.metadata().await?.len();
            // telegram_max_file_size is user-configurable; raise if Telegram increases Bot API limits.
            if size <= cfg.telegram_max_file_size {
                continue;
            }
            tracing::warn!(
                file = %path.display(),
                size,
                max = cfg.telegram_max_file_size,
                ".m4s segment exceeds Telegram limit; will be split at upload time"
            );
            oversized_m4s.push(path);
        }
    }

    // m4s repair is deferred to upload-time byte-splitting; no in-place re-encode is done.
    // Report 0 repaired so callers have an honest count.
    let m4s_repair_count = 0usize;

    for path in &oversized_m4s {
        let duration = super::process_probe::probe_duration(path)
            .await
            .unwrap_or(repair_target_secs as f64);
        let bps = max_bitrate_for_segment(cfg.telegram_max_file_size, duration);
        if repair_needs_split(bps) {
            // Upload-time splitting also cannot help here — the segment duration is too long
            // to fit within the Telegram limit at any sane bitrate. Fail loudly.
            anyhow::bail!(
                "oversized .m4s segment {} cannot be split at upload time (required bitrate {}bps too low); \
                 lower source bitrate or shrink SEGMENT_TARGET_SIZE",
                path.display(),
                bps
            );
        }
        tracing::warn!(
            segment = %path.display(),
            bitrate_bps = bps,
            "oversized .m4s segment will be split at upload time"
        );
    }

    let _ = tokio::fs::remove_dir_all(&ts_dir).await;
    Ok(m4s_repair_count)
}

pub(super) async fn encode_video_tier_ts(
    analysis: &MediaAnalysis,
    video: &VideoStream,
    tier: &VideoTier,
    encoder: &SelectedEncoder,
    cfg: &Config,
    dir: &Path,
    cancel: &Arc<AtomicBool>,
) -> Result<()> {
    let target_secs = target_segment_seconds_for_tier(cfg, analysis, tier);
    tracing::debug!(
        tier = tier.index,
        copy = tier.copy,
        target_secs,
        source_bytes = analysis.file_size,
        source_duration = analysis.duration,
        "per-job HLS segment duration"
    );
    let mut cmd = Command::new("ffmpeg");
    cmd.arg("-y").arg("-nostdin");
    add_encoder_device_args(&mut cmd, encoder);
    cmd.arg("-i")
        .arg(&analysis.file_path)
        .arg("-map")
        .arg("0:v:0")
        .arg("-an")
        .arg("-sn");
    if tier.copy {
        cmd.arg("-c:v").arg("copy");
    } else {
        cmd.arg("-c:v")
            .arg(&encoder.name)
            .arg("-b:v")
            .arg(&tier.bitrate)
            .arg("-minrate")
            .arg(&tier.bitrate)
            .arg("-maxrate")
            .arg(&tier.bitrate)
            .arg("-bufsize")
            .arg(super::process_audio::double_bitrate(&tier.bitrate))
            .arg("-flags")
            .arg("+cgop")
            .arg("-sc_threshold")
            .arg("0")
            .arg("-force_key_frames")
            .arg(format!("expr:gte(t,n_forced*{})", target_secs.max(1)));
        add_forced_idr_args(&mut cmd, encoder);
        let scale = if tier.height < video.height {
            Some(format!(
                "scale='trunc({}*iw/ih/2)*2':{}",
                tier.height, tier.height
            ))
        } else {
            None
        };
        if let Some(filter) = video_filter(encoder, scale) {
            cmd.arg("-vf").arg(filter);
        }
    }
    cmd.arg("-f")
        .arg("hls")
        .arg("-hls_time")
        .arg(target_secs.to_string())
        .arg("-hls_playlist_type")
        .arg("vod")
        .arg("-hls_segment_type")
        .arg("mpegts")
        .arg("-hls_segment_filename")
        .arg(dir.join("video_%04d.ts"))
        .arg("-hls_flags")
        .arg("independent_segments")
        .arg("-hls_list_size")
        .arg("0")
        .arg(dir.join("playlist.m3u8"));
    super::ffmpeg::run_ffmpeg_cancellable(&mut cmd, cancel, cfg.job_timeout_seconds as u64)
        .await
        .with_context(|| format!("encoding video tier {}", tier.index))
}

pub(super) async fn repair_oversized_video_segments(
    dir: &Path,
    _tier: &VideoTier,
    encoder: &SelectedEncoder,
    cfg: &Config,
    target_secs: u32,
    cancel: &Arc<AtomicBool>,
    encode_semaphore: &Arc<Semaphore>,
) -> Result<()> {
    let mut oversized = Vec::new();
    let mut entries = tokio::fs::read_dir(dir).await?;
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("ts") {
            continue;
        }
        let size = entry.metadata().await?.len();
        if size <= cfg.telegram_max_file_size {
            continue;
        }
        tracing::warn!(
            segment = %path.display(),
            size,
            max_size = cfg.telegram_max_file_size,
            "video segment exceeds Telegram limit; re-encoding segment at highest bitrate"
        );
        oversized.push(path);
    }

    if oversized.is_empty() {
        return Ok(());
    }

    let encoder = encoder.clone();
    let cfg = cfg.clone();
    let encode_semaphore = encode_semaphore.clone();
    let mut handles = Vec::with_capacity(oversized.len());

    for path in oversized {
        let encoder = encoder.clone();
        let cfg = cfg.clone();
        let cancel = cancel.clone();
        let encode_semaphore = encode_semaphore.clone();

        let duration = super::process_probe::probe_duration(&path)
            .await
            .unwrap_or(target_secs as f64);
        let bps = max_bitrate_for_segment(cfg.telegram_max_file_size, duration);
        if repair_needs_split(bps) {
            tracing::warn!(
                segment = %path.display(),
                bitrate_bps = bps,
                "TS segment bitrate floor too low; leaving original for upload-time splitting"
            );
            continue;
        }

        handles.push(tokio::spawn(async move {
            let _permit = super::ffmpeg::acquire_ffmpeg_permit(&encode_semaphore, &cancel).await?;
            repair_oversized_segment_max_bitrate(&path, &encoder, &cfg, target_secs, &cancel).await
        }));
    }

    for handle in handles {
        handle.await??;
    }

    Ok(())
}

pub(crate) async fn copied_segments_need_reencode(dir: &Path, target_secs: u32) -> Result<bool> {
    let playlist = tokio::fs::read_to_string(dir.join("playlist.m3u8")).await?;
    let durations = parse_hls_segment_durations("video", &playlist);
    if durations.is_empty() {
        return Ok(true);
    }
    let limit = (target_secs as f64 * 1.75).max(0.001);
    Ok(durations.values().any(|d| *d <= 0.0 || *d > limit))
}

/// Computes the highest bitrate (bps) that keeps a segment within max_file_size.
/// max_file_size is typically cfg.telegram_max_file_size — user-configurable;
/// raise if Telegram increases Bot API limits.
pub(crate) fn max_bitrate_for_segment(max_file_size: u64, duration_secs: f64) -> u64 {
    let max_size = (max_file_size as f64 * 0.95) as u64;
    let seconds = duration_secs.max(0.1);
    (max_size as f64 * 8.0 / seconds) as u64
}

pub(crate) fn repair_needs_split(bps: u64) -> bool {
    bps / 1000 <= 32
}

pub(super) async fn repair_oversized_segment_max_bitrate(
    path: &Path,
    encoder: &SelectedEncoder,
    cfg: &Config,
    target_secs: u32,
    cancel: &Arc<AtomicBool>,
) -> Result<()> {
    let duration = super::process_probe::probe_duration(path)
        .await
        .unwrap_or(target_secs as f64);
    // telegram_max_file_size is user-configurable; raise if Telegram increases Bot API limits.
    let max_size = (cfg.telegram_max_file_size as f64 * 0.95) as u64;
    let seconds = duration.max(0.1);
    let bps = ((max_size as f64 * 8.0) / seconds) as u64;
    let bitrate = format!("{}k", (bps / 1000).max(32));

    let current_size = tokio::fs::metadata(path)
        .await
        .map(|m| m.len())
        .unwrap_or(0);

    tracing::warn!(
        segment = %path.display(),
        size = current_size,
        max_size,
        duration,
        bitrate = %bitrate,
        "re-encoding oversized segment at highest bitrate"
    );

    let is_m4s = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e == "m4s")
        .unwrap_or(false);
    let tmp = if is_m4s {
        path.with_extension("m4s.tmp")
    } else {
        path.with_extension("ts.tmp")
    };

    let mut cmd = Command::new("ffmpeg");
    cmd.arg("-y").arg("-nostdin");
    add_encoder_device_args(&mut cmd, encoder);
    cmd.arg("-i")
        .arg(super::process_probe::fmp4_input_arg(path))
        .arg("-map")
        .arg("0:v:0")
        .arg("-an")
        .arg("-sn")
        .arg("-c:v")
        .arg(&encoder.name)
        .arg("-b:v")
        .arg(&bitrate)
        .arg("-maxrate")
        .arg(&bitrate)
        .arg("-bufsize")
        .arg(super::process_audio::double_bitrate(&bitrate))
        .arg("-flags")
        .arg("+cgop")
        .arg("-sc_threshold")
        .arg("0")
        .arg("-force_key_frames")
        .arg(format!("expr:gte(t,n_forced*{})", target_secs.max(1)));
    add_forced_idr_args(&mut cmd, encoder);
    if let Some(filter) = video_filter(encoder, None) {
        cmd.arg("-vf").arg(filter);
    }
    if is_m4s {
        cmd.arg("-f")
            .arg("mp4")
            .arg("-movflags")
            .arg("frag_keyframe+empty_moov");
    } else {
        cmd.arg("-f").arg("mpegts");
    }
    cmd.arg(&tmp);
    super::ffmpeg::run_ffmpeg_cancellable(&mut cmd, cancel, cfg.job_timeout_seconds as u64)
        .await
        .with_context(|| {
            format!(
                "re-encoding oversized segment at max bitrate {}",
                path.display()
            )
        })?;

    tokio::fs::rename(&tmp, path).await?;

    let repaired = tokio::fs::metadata(path).await?.len();
    // telegram_max_file_size is user-configurable; raise if Telegram increases Bot API limits.
    if repaired > cfg.telegram_max_file_size {
        bail!(
            "re-encoded segment {} is still too large after highest-bitrate repair: {} > {}",
            path.display(),
            repaired,
            cfg.telegram_max_file_size
        );
    }

    tracing::info!(
        segment = %path.display(),
        repaired_size = repaired,
        "oversized segment repaired at highest bitrate"
    );

    Ok(())
}

pub(super) async fn remux_video_ts_to_fmp4(
    ts_dir: &Path,
    cfg: &Config,
    out_dir: &Path,
    cancel: &Arc<AtomicBool>,
) -> Result<()> {
    let segments = super::process_probe::sorted_files_with_ext(ts_dir, "ts").await?;
    if segments.is_empty() {
        bail!("no video TS segments produced");
    }
    let list_path = ts_dir.join("concat.txt");
    let mut list = String::new();
    for path in &segments {
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        list.push_str("file '");
        list.push_str(&name.replace('\'', "'\\''"));
        list.push_str("'\n");
    }
    tokio::fs::write(&list_path, list).await?;

    let mut cmd = Command::new("ffmpeg");
    cmd.arg("-y")
        .arg("-nostdin")
        .arg("-f")
        .arg("concat")
        .arg("-safe")
        .arg("0")
        .arg("-i")
        .arg(&list_path)
        .arg("-map")
        .arg("0:v:0")
        .arg("-c")
        .arg("copy")
        .arg("-f")
        .arg("hls")
        .arg("-hls_time")
        .arg(cfg.hls_segment_duration.to_string())
        .arg("-hls_playlist_type")
        .arg("vod")
        .arg("-hls_segment_type")
        .arg("fmp4")
        .arg("-hls_fmp4_init_filename")
        .arg("init.mp4")
        .arg("-hls_segment_filename")
        .arg(out_dir.join("video_%04d.m4s"))
        .arg("-hls_flags")
        .arg("independent_segments")
        .arg("-hls_list_size")
        .arg("0")
        .arg(out_dir.join("playlist.m3u8"));
    super::ffmpeg::run_ffmpeg_cancellable(&mut cmd, cancel, cfg.job_timeout_seconds as u64)
        .await
        .context("remuxing repaired video TS sequence to fMP4 HLS")
}
