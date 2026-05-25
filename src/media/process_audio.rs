use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::process::Command;

use super::models::*;
use crate::config::Config;

pub(super) async fn encode_audio_track(
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
            return super::ffmpeg::run_ffmpeg_cancellable(
                &mut cmd,
                cancel,
                cfg.job_timeout_seconds as u64,
            )
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
    super::ffmpeg::run_ffmpeg_cancellable(&mut cmd, cancel, cfg.job_timeout_seconds as u64)
        .await
        .with_context(|| format!("encoding audio stream {}", audio.index))
}

pub(crate) fn output_audio_channels(audio: &AudioStream) -> i64 {
    let layout = audio.channel_layout.as_str();
    if audio.channels == 1 {
        1
    } else if audio.channels == 2 || layout.starts_with("5.1") || layout.starts_with("3.1") {
        audio.channels
    } else {
        2
    }
}

pub(super) fn resolve_audio_bitrate(cfg: &Config, audio: &AudioStream) -> String {
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
    let lower = raw.to_ascii_lowercase();
    let suffixes = ["kbps", "mbps", "gbps", "bps", "k", "m", "g"];
    let (number_str, suffix_len) = suffixes
        .iter()
        .find_map(|s| lower.ends_with(s).then_some(s.len()))
        .map_or((raw, 0), |len| (&raw[..raw.len() - len], len));
    let suffix = if suffix_len > 0 {
        &raw[raw.len() - suffix_len..]
    } else {
        ""
    };
    match number_str.trim().parse::<f64>() {
        Ok(number) => {
            let doubled = number * 2.0;
            if doubled.fract() == 0.0 {
                format!("{:.0}{}", doubled, suffix)
            } else {
                format!("{}{}", doubled, suffix)
            }
        }
        Err(_) => raw.to_string(),
    }
}
