use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{anyhow, Context, Result};
use tokio::process::Command;
use tokio::sync::Semaphore;

use super::ffmpeg::run_ffmpeg_cancellable;
use super::models::*;
use super::process_audio::encode_audio_track;
pub(crate) use super::process_audio::output_audio_channels;
#[cfg(test)]
pub(crate) use super::process_audio::{bitrate_bits, double_bitrate};
use super::process_probe::{collect_segment_durations, extract_thumbnail};
#[cfg(test)]
pub(crate) use super::process_probe::{fmp4_input_arg, parse_hls_segment_durations};
use super::process_video::encode_video_tier;
#[cfg(test)]
pub(crate) use super::process_video::{
    copied_segments_need_reencode, max_bitrate_for_segment, repair_needs_split,
};
use super::tiers::select_video_tiers_with;
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
    let lower = trimmed.to_ascii_lowercase();
    let (num, mult) = if let Some(num) = lower.strip_suffix("kbps") {
        (num, 1_000u64)
    } else if let Some(num) = lower.strip_suffix("mbps") {
        (num, 1_000_000u64)
    } else if let Some(num) = lower.strip_suffix("gbps") {
        (num, 1_000_000_000u64)
    } else if let Some(num) = lower.strip_suffix("bps") {
        (num, 1u64)
    } else {
        match lower.bytes().last()? {
            b'k' => (&lower[..lower.len() - 1], 1_000u64),
            b'm' => (&lower[..lower.len() - 1], 1_000_000u64),
            b'g' => (&lower[..lower.len() - 1], 1_000_000_000u64),
            _ => (lower.as_str(), 1u64),
        }
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
            cfg.job_timeout_seconds as u64,
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

    let thumbnail_path =
        extract_thumbnail(analysis, output_dir, cancel, cfg.job_timeout_seconds as u64).await;
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
