use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use super::encoder::{add_encoder_device_args, add_forced_idr_args, video_filter};
use super::models::*;
use super::tiers::{select_video_tiers_with, tier0_bitrate};
use crate::config::Config;

// ============================================================================
// Per-job HLS segment duration — DO NOT REGRESS TO A GLOBAL SETTING
// ============================================================================
//
// HISTORY
// -------
// Up to season N-1 we had a global user-facing setting `HLS_SEGMENT_DURATION`
// (default 4 s) passed verbatim to ffmpeg's `-hls_time`. This was wrong in
// practice: a 4 s slice of a 50 Mbps 4K master is ~25 MB, well past Telegram's
// 20 MB per-file ceiling, so the segment had to be re-encoded down or split at
// upload time. Meanwhile a 4 s slice of a 1 Mbps 480p master is ~500 KB and
// wasted everyone's time by producing 5× more segments than necessary.
//
// In season N we deleted the user-facing setting and switched to deriving the
// per-tier segment duration from `SEGMENT_TARGET_SIZE` (the byte ceiling we
// want each segment to land near).
//
// THE FORMULA
// -----------
//   target_seconds = SEGMENT_TARGET_SIZE / byterate
//
// where `byterate` is the bytes-per-second the encoded tier is expected to
// produce:
//
//   - Copy tier (no re-encode): byterate = file_size / duration of the source.
//   - Encode tier:               byterate = tier_bitrate_bps / 8.
//
// The result is clamped to [2, 30] s so a 200 KB GIF or a 4 GB IMAX rip don't
// produce useless GOPs.
//
// WHY NOT JUST PICK A SHORT DURATION?
// -----------------------------------
// Short durations (≤ 2 s) explode segment count and balloon the playlist; HLS
// player join latency suffers. Long durations (≥ 30 s) cripple seeking and
// blow past Telegram's per-file ceiling on high-bitrate sources. The clamp is
// load-bearing — keep it.
//
// WHY NOT EXPOSE THIS AS A SETTING?
// ---------------------------------
// `SEGMENT_TARGET_SIZE` already exposes the only knob the user actually cares
// about ("how big a single chunk Telegram has to swallow"). The seconds figure
// is a *consequence* of that knob and the per-job source bitrate; giving the
// user a second knob just lets them desync the two. Don't add it back.
//
// IF YOU NEED TO TOUCH THIS
// -------------------------
// - The fallback `Config.hls_segment_duration` field (= 4 s by default) is
//   intentionally NOT loaded from settings any more. It is used only by the
//   playlist-rendering paths (`api/playlists.rs`, `api/playback/virtual_.rs`)
//   where there is no job context to compute a real value from. Leave it.
// - Encode-time callers (`encode_video_tier_ts`, `copied_segments_need_reencode`,
//   `repair_oversized_video_segments`, `repair_oversized_segment_max_bitrate`)
//   take `target_secs: u32` plumbed from `target_segment_seconds_for_tier`.
//   New encode-time call sites should plumb it the same way; do not reach for
//   `cfg.hls_segment_duration`.
// - Audio is a separate concern. Audio bitrate is small and stable, so
//   `AUDIO_SEGMENT_DURATION` is intentionally still a static user setting
//   (see settings_registry.rs, file_handling category). Don't merge it
//   into this formula.
// - Tests in `src/media/mod.rs` drive the formula by setting
//   `cfg.segment_target_size`, not by setting a segment-duration field.
// ============================================================================
pub(crate) fn target_segment_seconds_for_tier(
    cfg: &Config,
    analysis: &MediaAnalysis,
    tier: &VideoTier,
) -> u32 {
    let target_bytes = cfg.segment_target_size as f64;

    let byterate: f64 = if tier.copy {
        // Copy tier: the muxer just re-packages the source, so the output's
        // byterate matches the source's byterate. duration.max(1.0) guards
        // against zero-duration probes from corrupt inputs.
        let dur = analysis.duration.max(1.0);
        (analysis.file_size as f64 / dur).max(1.0)
    } else {
        // Encode tier: ffmpeg targets `tier.bitrate` (e.g. "5M", "1500k").
        // 4 Mbps fallback only fires if the tier string is malformed —
        // tiers come from the registry/UI which validate the format.
        let bps = parse_bitrate_bps(&tier.bitrate).unwrap_or(4_000_000);
        (bps as f64 / 8.0).max(1.0)
    };

    // [2, 30] s clamp — see "WHY NOT JUST PICK A SHORT DURATION?" above.
    let secs = (target_bytes / byterate).round().max(1.0);
    secs.clamp(2.0, 30.0) as u32
}

/// Parses ffmpeg-style bitrate strings ("5M", "1500k", "4000000") into
/// bits-per-second. Returns `None` for malformed input so callers can fall
/// back to a safe default.
fn parse_bitrate_bps(s: &str) -> Option<u64> {
    let trimmed = s.trim();
    let (num, mult) = match trimmed.bytes().last()? {
        b'k' | b'K' => (&trimmed[..trimmed.len() - 1], 1_000u64),
        b'm' | b'M' => (&trimmed[..trimmed.len() - 1], 1_000_000u64),
        b'g' | b'G' => (&trimmed[..trimmed.len() - 1], 1_000_000_000u64),
        _ => (trimmed, 1u64),
    };
    let v: f64 = num.parse().ok()?;
    Some((v * mult as f64) as u64)
}

pub async fn process_media(
    analysis: &MediaAnalysis,
    job_id: &str,
    output_dir: &Path,
    cfg: &Config,
    cancel: &Arc<AtomicBool>,
    abr_tiers_override: Option<&str>,
) -> Result<ProcessingResult> {
    tokio::fs::create_dir_all(output_dir).await?;
    let video = analysis
        .video_streams
        .first()
        .ok_or_else(|| anyhow!("no video stream found"))?;
    let encoder = super::encoder::select_encoder(cfg).await;
    let tiers = match abr_tiers_override {
        Some(raw) => select_video_tiers_with(cfg, &video.codec_name, video.height, raw),
        None => super::tiers::select_video_tiers(cfg, &video.codec_name, video.height),
    };
    let tier_summary = tiers
        .iter()
        .map(|t| {
            format!(
                "{}:{}:{}",
                t.index,
                t.height,
                if t.copy { "copy" } else { &t.bitrate }
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    tracing::info!(
        job_id,
        source_codec = %video.codec_name,
        source_width = video.width,
        source_height = video.height,
        encoder = %encoder.name,
        vaapi_device = encoder.vaapi_device.as_deref().unwrap_or(""),
        tiers = %tier_summary,
        "media processing plan selected"
    );
    let encode_semaphore = Arc::new(Semaphore::new(cfg.max_parallel_encodes.max(1) as usize));
    let analysis_for_tiers = Arc::new(analysis.clone());
    let mut tier_handles = Vec::with_capacity(tiers.len());

    for tier in tiers.iter().cloned() {
        let analysis = analysis_for_tiers.clone();
        let video = video.clone();
        let encoder = encoder.clone();
        let cfg = cfg.clone();
        let cancel = cancel.clone();
        let output_dir = output_dir.to_path_buf();
        let job_id = job_id.to_string();
        let encode_semaphore = encode_semaphore.clone();

        tier_handles.push(tokio::spawn(async move {
            let tier_dir = format!("video_{}", tier.index);
            let dir = output_dir.join(&tier_dir);
            tokio::fs::create_dir_all(&dir).await?;
            let started = Instant::now();
            tracing::info!(
                job_id = %job_id,
                tier = tier.index,
                height = tier.height,
                bitrate = %tier.bitrate,
                copy = tier.copy,
                "video tier encode started"
            );
            let oversized_count = encode_video_tier(
                &analysis,
                &video,
                &tier,
                &encoder,
                &cfg,
                &dir,
                &cancel,
                &encode_semaphore,
            )
            .await?;
            tracing::info!(
                job_id = %job_id,
                tier = tier.index,
                elapsed_ms = started.elapsed().as_millis(),
                "video tier encode complete"
            );
            Ok::<_, anyhow::Error>((
                tier.index,
                VideoPlaylist {
                    playlist_path: dir.join("playlist.m3u8"),
                    tier_dir,
                    width: scaled_width(video.width, video.height, tier.height),
                    height: tier.height,
                    bitrate: tier.bitrate.clone(),
                },
                oversized_count,
            ))
        }));
    }

    let mut indexed_video_playlists = Vec::with_capacity(tier_handles.len());
    let mut tier_error = None;
    let mut total_oversized_repaired = 0usize;
    for handle in tier_handles {
        match handle.await {
            Ok(Ok((index, playlist, oversized))) => {
                total_oversized_repaired += oversized;
                indexed_video_playlists.push((index, playlist));
            }
            Ok(Err(err)) => {
                if tier_error.is_none() {
                    tier_error = Some(err);
                }
            }
            Err(err) => {
                if tier_error.is_none() {
                    tier_error = Some(err.into());
                }
            }
        }
    }
    if let Some(err) = tier_error {
        return Err(err);
    }
    indexed_video_playlists.sort_by_key(|(index, _)| *index);
    let video_playlists = indexed_video_playlists
        .into_iter()
        .map(|(_, playlist)| playlist)
        .collect();

    let mut audio_playlists = Vec::new();
    for (idx, audio) in analysis.audio_streams.iter().enumerate() {
        let audio_dir = format!("audio_{idx}");
        let dir = output_dir.join(&audio_dir);
        tokio::fs::create_dir_all(&dir).await?;
        let started = Instant::now();
        tracing::info!(
            job_id,
            stream = audio.index,
            codec = %audio.codec_name,
            channels = audio.channels,
            language = %audio.language,
            "audio track encode started"
        );
        encode_audio_track(analysis, audio, cfg, &dir, cancel).await?;
        tracing::info!(
            job_id,
            stream = audio.index,
            elapsed_ms = started.elapsed().as_millis(),
            "audio track encode complete"
        );
        audio_playlists.push(AudioPlaylist {
            playlist_path: dir.join("playlist.m3u8"),
            audio_dir,
            language: audio.language.clone(),
            title: audio.title.clone(),
            channels: output_audio_channels(audio),
        });
    }

    let mut subtitle_files = Vec::new();
    for (idx, sub) in analysis.subtitle_streams.iter().enumerate() {
        if !is_text_subtitle(&sub.codec_name) {
            tracing::warn!(codec = %sub.codec_name, stream = sub.index, "skipping bitmap subtitle");
            continue;
        }
        let sub_dir = format!("sub_{idx}");
        let dir = output_dir.join(&sub_dir);
        tokio::fs::create_dir_all(&dir).await?;
        let vtt_path = dir.join("subtitles.vtt");
        let started = Instant::now();
        tracing::info!(
            job_id,
            stream = sub.index,
            codec = %sub.codec_name,
            language = %sub.language,
            "subtitle extract started"
        );
        run_ffmpeg_cancellable(
            Command::new("ffmpeg")
                .arg("-y")
                .arg("-i")
                .arg(&analysis.file_path)
                .arg("-map")
                .arg(format!("0:{}", sub.index))
                .arg("-c:s")
                .arg("webvtt")
                .arg(&vtt_path),
            cancel,
        )
        .await
        .with_context(|| format!("extracting subtitle stream {}", sub.index))?;
        tracing::info!(
            job_id,
            stream = sub.index,
            elapsed_ms = started.elapsed().as_millis(),
            "subtitle extract complete"
        );
        subtitle_files.push(SubtitleFile {
            vtt_path,
            sub_dir,
            language: sub.language.clone(),
            title: sub.title.clone(),
            enum_idx: idx,
            original_stream_idx: sub.index,
        });
    }

    let thumbnail_path = extract_thumbnail(analysis, output_dir).await;
    let segment_durations = collect_segment_durations(output_dir).await?;
    tracing::info!(
        job_id,
        segment_durations = segment_durations.len(),
        thumbnail = thumbnail_path.is_some(),
        "media processing outputs indexed"
    );

    Ok(ProcessingResult {
        job_id: job_id.to_string(),
        output_dir: output_dir.to_path_buf(),
        video_playlists,
        audio_playlists,
        subtitle_files,
        segment_durations,
        thumbnail_path,
        oversized_segments_repaired: total_oversized_repaired,
    })
}

async fn encode_video_tier(
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
    let permit = acquire_ffmpeg_permit(encode_semaphore, cancel).await?;
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
            let permit = acquire_ffmpeg_permit(encode_semaphore, cancel).await?;
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
        let duration = probe_duration(path)
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

async fn encode_video_tier_ts(
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
            .arg(double_bitrate(&tier.bitrate))
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
    run_ffmpeg_cancellable(&mut cmd, cancel)
        .await
        .with_context(|| format!("encoding video tier {}", tier.index))
}

async fn repair_oversized_video_segments(
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

        let duration = probe_duration(&path).await.unwrap_or(target_secs as f64);
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
            let _permit = acquire_ffmpeg_permit(&encode_semaphore, &cancel).await?;
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

async fn repair_oversized_segment_max_bitrate(
    path: &Path,
    encoder: &SelectedEncoder,
    cfg: &Config,
    target_secs: u32,
    cancel: &Arc<AtomicBool>,
) -> Result<()> {
    let duration = probe_duration(path).await.unwrap_or(target_secs as f64);
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
        .arg(fmp4_input_arg(path))
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
        .arg(double_bitrate(&bitrate))
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
    run_ffmpeg_cancellable(&mut cmd, cancel)
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

async fn remux_video_ts_to_fmp4(
    ts_dir: &Path,
    cfg: &Config,
    out_dir: &Path,
    cancel: &Arc<AtomicBool>,
) -> Result<()> {
    let segments = sorted_files_with_ext(ts_dir, "ts").await?;
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
    run_ffmpeg_cancellable(&mut cmd, cancel)
        .await
        .context("remuxing repaired video TS sequence to fMP4 HLS")
}

async fn encode_audio_track(
    analysis: &MediaAnalysis,
    audio: &AudioStream,
    cfg: &Config,
    dir: &Path,
    cancel: &Arc<AtomicBool>,
) -> Result<()> {
    // If source is already AAC at or below target bitrate, copy instead of re-encode.
    if audio.codec_name.eq_ignore_ascii_case("aac") {
        let source_bps = audio.bit_rate.trim().parse::<u64>().unwrap_or(0);
        if source_bps > 0 && source_bps <= bitrate_bits(&cfg.audio_bitrate) {
            tracing::info!(
                stream = audio.index,
                codec = %audio.codec_name,
                source_bitrate = source_bps,
                target_bitrate = %cfg.audio_bitrate,
                "AAC source at or below target bitrate; copying audio stream"
            );
            let mut cmd = Command::new("ffmpeg");
            cmd.arg("-y")
                .arg("-nostdin")
                .arg("-i")
                .arg(&analysis.file_path)
                .arg("-map")
                .arg(format!("0:{}", audio.index))
                .arg("-vn")
                .arg("-sn")
                .arg("-c:a")
                .arg("copy")
                .arg("-f")
                .arg("hls")
                .arg("-hls_time")
                .arg(cfg.audio_segment_duration.to_string())
                .arg("-hls_playlist_type")
                .arg("vod")
                .arg("-hls_segment_type")
                .arg("fmp4")
                .arg("-hls_fmp4_init_filename")
                .arg("init.mp4")
                .arg("-hls_segment_filename")
                .arg(dir.join("audio_%04d.m4s"))
                .arg("-hls_flags")
                .arg("independent_segments")
                .arg("-hls_list_size")
                .arg("0")
                .arg(dir.join("playlist.m3u8"));
            return run_ffmpeg_cancellable(&mut cmd, cancel)
                .await
                .with_context(|| format!("encoding audio stream {}", audio.index));
        }
    }

    let mut cmd = Command::new("ffmpeg");
    cmd.arg("-y")
        .arg("-nostdin")
        .arg("-i")
        .arg(&analysis.file_path)
        .arg("-map")
        .arg(format!("0:{}", audio.index))
        .arg("-vn")
        .arg("-sn")
        .arg("-c:a")
        .arg("aac")
        .arg("-b:a")
        .arg(resolve_audio_bitrate(cfg, audio))
        .arg("-ac")
        .arg(output_audio_channels(audio).to_string())
        .arg("-f")
        .arg("hls")
        .arg("-hls_time")
        .arg(cfg.audio_segment_duration.to_string())
        .arg("-hls_playlist_type")
        .arg("vod")
        .arg("-hls_segment_type")
        .arg("fmp4")
        .arg("-hls_fmp4_init_filename")
        .arg("init.mp4")
        .arg("-hls_segment_filename")
        .arg(dir.join("audio_%04d.m4s"))
        .arg("-hls_flags")
        .arg("independent_segments")
        .arg("-hls_list_size")
        .arg("0")
        .arg(dir.join("playlist.m3u8"));
    run_ffmpeg_cancellable(&mut cmd, cancel)
        .await
        .with_context(|| format!("encoding audio stream {}", audio.index))
}

async fn run_ffmpeg(cmd: &mut Command) -> Result<()> {
    let output = cmd.output().await.context("running ffmpeg")?;
    if output.status.success() {
        return Ok(());
    }
    bail!(
        "ffmpeg failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    )
}

async fn acquire_ffmpeg_permit(
    semaphore: &Arc<Semaphore>,
    cancel: &Arc<AtomicBool>,
) -> Result<OwnedSemaphorePermit> {
    loop {
        if cancel.load(Ordering::Relaxed) {
            bail!("cancelled");
        }
        tokio::select! {
            permit = semaphore.clone().acquire_owned() => {
                return permit.context("acquiring ffmpeg encode permit");
            }
            _ = tokio::time::sleep(Duration::from_millis(200)) => {}
        }
    }
}

async fn run_ffmpeg_cancellable(cmd: &mut Command, cancel: &Arc<AtomicBool>) -> Result<()> {
    tracing::debug!(cmd = ?cmd, "ffmpeg spawn");
    let started = Instant::now();
    cmd.stdout(Stdio::null()).stderr(Stdio::piped());
    let mut child = cmd.spawn().context("spawning ffmpeg")?;

    // Drain stderr in background so FFmpeg never blocks on a full pipe.
    let stderr_task = child.stderr.take().map(|mut stderr| {
        tokio::spawn(async move {
            let mut buf = Vec::with_capacity(8192);
            let mut tmp = [0u8; 4096];
            loop {
                match stderr.read(&mut tmp).await {
                    Ok(0) => break,
                    Ok(n) => {
                        buf.extend_from_slice(&tmp[..n]);
                        if buf.len() > 8192 {
                            let keep = 8192;
                            buf = buf.split_off(buf.len() - keep);
                        }
                    }
                    Err(_) => break,
                }
            }
            buf
        })
    });

    let exit_status = loop {
        tokio::select! {
            status = child.wait() => break status,
            _ = tokio::time::sleep(Duration::from_millis(200)) => {
                if cancel.load(Ordering::Relaxed) {
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                    if let Some(h) = stderr_task { h.abort(); }
                    bail!("cancelled");
                }
            }
        }
    };

    let stderr_bytes = if let Some(h) = stderr_task {
        h.await.unwrap_or_default()
    } else {
        Vec::new()
    };
    let exit_status = exit_status.context("waiting for ffmpeg")?;
    if exit_status.success() {
        tracing::debug!(
            elapsed_ms = started.elapsed().as_millis(),
            "ffmpeg complete"
        );
        return Ok(());
    }
    tracing::warn!(
        elapsed_ms = started.elapsed().as_millis(),
        stderr = %String::from_utf8_lossy(&stderr_bytes).trim(),
        "ffmpeg failed"
    );
    bail!(
        "ffmpeg failed: {}",
        String::from_utf8_lossy(&stderr_bytes).trim()
    )
}

async fn extract_thumbnail(analysis: &MediaAnalysis, output_dir: &Path) -> Option<PathBuf> {
    let dir = output_dir.join("thumbnail");
    if tokio::fs::create_dir_all(&dir).await.is_err() {
        return None;
    }
    let path = dir.join("thumbnail.jpg");
    let seek = analysis.duration.mul_add(0.10, 0.0).max(2.0).to_string();
    let mut cmd = Command::new("ffmpeg");
    cmd.arg("-y")
        .arg("-ss")
        .arg(seek)
        .arg("-i")
        .arg(&analysis.file_path)
        .arg("-vframes")
        .arg("1")
        .arg("-q:v")
        .arg("5")
        .arg("-vf")
        .arg("scale='min(640,iw)':-2")
        .arg(&path);
    match run_ffmpeg(&mut cmd).await {
        Ok(()) => Some(path),
        Err(e) => {
            tracing::warn!(error = %e, "thumbnail extraction failed");
            None
        }
    }
}

async fn collect_segment_durations(output_dir: &Path) -> Result<HashMap<String, f64>> {
    let mut out = HashMap::new();
    let mut dirs = tokio::fs::read_dir(output_dir).await?;
    while let Some(dir) = dirs.next_entry().await? {
        if !dir.file_type().await?.is_dir() {
            continue;
        }
        let prefix = dir.file_name().to_string_lossy().to_string();
        let dir_path = dir.path();
        let playlist_path = dir_path.join("playlist.m3u8");
        if let Ok(playlist) = tokio::fs::read_to_string(&playlist_path).await {
            out.extend(parse_hls_segment_durations(&prefix, &playlist));
        }

        let mut files = tokio::fs::read_dir(dir_path).await?;
        while let Some(file) = files.next_entry().await? {
            let path = file.path();
            let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
                continue;
            };
            if !matches!(ext, "m4s" | "ts" | "vtt" | "jpg") {
                continue;
            }
            let key = format!("{prefix}/{}", file.file_name().to_string_lossy());
            let duration = if matches!(ext, "m4s" | "ts") {
                if out.contains_key(&key) {
                    continue;
                }
                probe_duration(&path).await.unwrap_or(0.0)
            } else {
                0.0
            };
            out.insert(key, duration);
        }
    }
    Ok(out)
}

async fn sorted_files_with_ext(dir: &Path, ext: &str) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let mut entries = tokio::fs::read_dir(dir).await?;
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some(ext) {
            out.push(path);
        }
    }
    out.sort();
    Ok(out)
}

pub(crate) fn parse_hls_segment_durations(prefix: &str, playlist: &str) -> HashMap<String, f64> {
    let mut out = HashMap::new();
    let mut pending_duration = None;
    for line in playlist.lines().map(str::trim) {
        if let Some(raw) = line.strip_prefix("#EXTINF:") {
            pending_duration = raw
                .split_once(',')
                .map(|(duration, _)| duration)
                .unwrap_or(raw)
                .parse::<f64>()
                .ok();
            continue;
        }
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some(duration) = pending_duration.take() else {
            continue;
        };
        let uri = line.split_once('?').map(|(head, _)| head).unwrap_or(line);
        let Some(filename) = Path::new(uri).file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        out.insert(format!("{prefix}/{filename}"), duration);
    }
    out
}

pub(crate) fn fmp4_input_arg(path: &Path) -> String {
    if path.extension().and_then(|e| e.to_str()) == Some("m4s") {
        let init = path.parent().unwrap_or(Path::new(".")).join("init.mp4");
        format!(
            "concat:{}|{}",
            init.to_string_lossy(),
            path.to_string_lossy()
        )
    } else {
        path.to_string_lossy().into_owned()
    }
}

async fn probe_duration(path: &Path) -> Result<f64> {
    let output = Command::new("ffprobe")
        .arg("-v")
        .arg("error")
        .arg("-show_entries")
        .arg("format=duration")
        .arg("-of")
        .arg("default=noprint_wrappers=1:nokey=1")
        .arg(fmp4_input_arg(path))
        .output()
        .await?;
    if !output.status.success() {
        bail!("ffprobe duration failed");
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .unwrap_or(0.0))
}

pub(crate) fn output_audio_channels(audio: &AudioStream) -> i64 {
    let layout = audio.channel_layout.as_str();
    if audio.channels == 1 {
        1
    } else if audio.channels == 2 || layout == "5.1" || layout == "3.1" {
        audio.channels
    } else {
        2
    }
}

fn resolve_audio_bitrate(cfg: &Config, audio: &AudioStream) -> String {
    let floor = if audio.channels > 2 { "192k" } else { "0k" };
    if bitrate_bits(&cfg.audio_bitrate) >= bitrate_bits(floor) {
        cfg.audio_bitrate.clone()
    } else {
        floor.to_string()
    }
}

pub(crate) fn bitrate_bits(raw: &str) -> u64 {
    let raw = raw.trim();
    if raw.is_empty() {
        return 0;
    }
    let lower = raw.to_ascii_lowercase();
    let suffixes = [
        ("kbps", 1_000.0),
        ("mbps", 1_000_000.0),
        ("gbps", 1_000_000_000.0),
        ("bps", 1.0),
        ("k", 1_000.0),
        ("m", 1_000_000.0),
        ("g", 1_000_000_000.0),
    ];
    let (number, mult) = suffixes
        .iter()
        .find_map(|(suffix, mult)| {
            lower
                .ends_with(suffix)
                .then(|| (&raw[..raw.len() - suffix.len()], *mult))
        })
        .unwrap_or((raw, 1.0));
    let number = number.trim().parse::<f64>().unwrap_or(0.0);
    (number * mult) as u64
}

pub(crate) fn double_bitrate(raw: &str) -> String {
    let raw = raw.trim();
    if raw.is_empty() {
        return "0".to_string();
    }
    let last_char = raw.chars().last().unwrap();
    if last_char.is_ascii_alphabetic() {
        let unit = last_char;
        let number_str = &raw[..raw.len() - unit.len_utf8()];
        if let Ok(number) = number_str.parse::<f64>() {
            let doubled = number * 2.0;
            if doubled.fract() == 0.0 {
                format!("{:.0}{}", doubled, unit)
            } else {
                format!("{}{}", doubled, unit)
            }
        } else {
            raw.to_string()
        }
    } else {
        if let Ok(number) = raw.parse::<f64>() {
            let doubled = number * 2.0;
            if doubled.fract() == 0.0 {
                format!("{:.0}", doubled)
            } else {
                format!("{}", doubled)
            }
        } else {
            raw.to_string()
        }
    }
}

pub(crate) fn is_text_subtitle(codec: &str) -> bool {
    matches!(
        codec,
        "subrip" | "srt" | "ass" | "ssa" | "webvtt" | "mov_text" | "text" | "ttml"
    )
}

pub(crate) fn scaled_width(source_width: i64, source_height: i64, target_height: i64) -> i64 {
    if source_width <= 0 || source_height <= 0 || target_height <= 0 {
        return 0;
    }
    let width = source_width * target_height / source_height;
    width - (width % 2)
}
