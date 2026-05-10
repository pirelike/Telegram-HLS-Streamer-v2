use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::sync::Semaphore;

use super::encoder::{add_encoder_device_args, add_forced_idr_args, video_filter};
use super::models::*;
use super::tiers::{select_video_tiers_with, tier0_bitrate};
use crate::config::Config;

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
    let mut video_playlists = Vec::new();

    for tier in &tiers {
        let tier_dir = format!("video_{}", tier.index);
        let dir = output_dir.join(&tier_dir);
        tokio::fs::create_dir_all(&dir).await?;
        let started = Instant::now();
        tracing::info!(
            job_id,
            tier = tier.index,
            height = tier.height,
            bitrate = %tier.bitrate,
            copy = tier.copy,
            "video tier encode started"
        );
        encode_video_tier(analysis, video, tier, &encoder, cfg, &dir, cancel).await?;
        tracing::info!(
            job_id,
            tier = tier.index,
            elapsed_ms = started.elapsed().as_millis(),
            "video tier encode complete"
        );
        video_playlists.push(VideoPlaylist {
            playlist_path: dir.join("playlist.m3u8"),
            tier_dir,
            width: scaled_width(video.width, video.height, tier.height),
            height: tier.height,
            bitrate: tier.bitrate.clone(),
        });
    }

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
) -> Result<()> {
    let ts_dir = dir.join("ts_work");
    tokio::fs::create_dir_all(&ts_dir).await?;
    encode_video_tier_ts(analysis, video, tier, encoder, cfg, &ts_dir, cancel).await?;
    let effective_tier = if tier.copy && copied_segments_need_reencode(&ts_dir, cfg).await? {
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
        reencode_tier
    } else {
        tier.clone()
    };
    repair_oversized_video_segments(&ts_dir, &effective_tier, encoder, cfg, cancel).await?;
    remux_video_ts_to_fmp4(&ts_dir, cfg, dir, cancel).await?;
    let _ = tokio::fs::remove_dir_all(&ts_dir).await;
    Ok(())
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
            .arg(format!(
                "expr:gte(t,n_forced*{})",
                cfg.hls_segment_duration.max(1)
            ));
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
        .arg(cfg.hls_segment_duration.to_string())
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
    tier: &VideoTier,
    encoder: &SelectedEncoder,
    cfg: &Config,
    cancel: &Arc<AtomicBool>,
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
        let duration = probe_duration(&path)
            .await
            .unwrap_or(cfg.hls_segment_duration as f64);
        let bitrate = repair_bitrate(cfg.telegram_max_file_size, duration);
        tracing::warn!(
            segment = %path.display(),
            size,
            max_size = cfg.telegram_max_file_size,
            duration,
            bitrate = %bitrate,
            "video segment exceeds Telegram limit; re-encoding segment only"
        );
        oversized.push((path, bitrate));
    }

    if oversized.is_empty() {
        return Ok(());
    }

    let semaphore = Arc::new(Semaphore::new(cfg.max_parallel_encodes as usize));
    let tier = tier.clone();
    let encoder = encoder.clone();
    let cfg = cfg.clone();
    let mut handles = Vec::with_capacity(oversized.len());

    for (path, bitrate) in oversized {
        let permit = semaphore.clone().acquire_owned().await?;
        let tier = tier.clone();
        let encoder = encoder.clone();
        let cfg = cfg.clone();
        let cancel = cancel.clone();

        handles.push(tokio::spawn(async move {
            let _permit = permit;
            reencode_video_segment(&path, &tier, &encoder, &bitrate, &cfg, &cancel).await?;
            let repaired = tokio::fs::metadata(&path).await?.len();
            if repaired > cfg.telegram_max_file_size {
                bail!(
                    "repaired segment {} is still too large: {} > {}",
                    path.display(),
                    repaired,
                    cfg.telegram_max_file_size
                );
            }
            Ok::<_, anyhow::Error>(())
        }));
    }

    for handle in handles {
        handle.await??;
    }

    Ok(())
}

pub(crate) async fn copied_segments_need_reencode(dir: &Path, cfg: &Config) -> Result<bool> {
    let playlist = tokio::fs::read_to_string(dir.join("playlist.m3u8")).await?;
    let durations = parse_hls_segment_durations("video", &playlist);
    if durations.is_empty() {
        return Ok(true);
    }
    let limit = (cfg.hls_segment_duration as f64 * 2.0)
        .max(cfg.hls_segment_duration as f64 * 1.75)
        .max(0.001);
    Ok(durations.values().any(|d| *d <= 0.0 || *d > limit))
}

async fn reencode_video_segment(
    path: &Path,
    tier: &VideoTier,
    encoder: &SelectedEncoder,
    bitrate: &str,
    cfg: &Config,
    cancel: &Arc<AtomicBool>,
) -> Result<()> {
    let tmp = path.with_extension("ts.tmp");
    let mut cmd = Command::new("ffmpeg");
    cmd.arg("-y").arg("-nostdin");
    add_encoder_device_args(&mut cmd, encoder);
    cmd.arg("-i")
        .arg(path)
        .arg("-map")
        .arg("0:v:0")
        .arg("-an")
        .arg("-sn")
        .arg("-c:v")
        .arg(&encoder.name)
        .arg("-b:v")
        .arg(bitrate)
        .arg("-maxrate")
        .arg(bitrate)
        .arg("-bufsize")
        .arg(double_bitrate(bitrate))
        .arg("-flags")
        .arg("+cgop")
        .arg("-sc_threshold")
        .arg("0")
        .arg("-force_key_frames")
        .arg(format!(
            "expr:gte(t,n_forced*{})",
            cfg.hls_segment_duration.max(1)
        ));
    add_forced_idr_args(&mut cmd, encoder);
    if let Some(filter) = video_filter(encoder, None) {
        cmd.arg("-vf").arg(filter);
    }
    cmd.arg("-f").arg("mpegts").arg(&tmp);
    run_ffmpeg_cancellable(&mut cmd, cancel)
        .await
        .with_context(|| format!("re-encoding oversized segment {}", path.display()))?;
    tokio::fs::rename(&tmp, path).await?;
    tracing::info!(
        segment = %path.display(),
        tier = tier.index,
        "oversized video segment repaired"
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
                .arg(cfg.hls_segment_duration.to_string())
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
        .arg(cfg.hls_segment_duration.to_string())
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

async fn run_ffmpeg_cancellable(cmd: &mut Command, cancel: &Arc<AtomicBool>) -> Result<()> {
    tracing::debug!(cmd = ?cmd, "ffmpeg spawn");
    let started = Instant::now();
    cmd.stdout(Stdio::null()).stderr(Stdio::piped());
    let mut child = cmd.spawn().context("spawning ffmpeg")?;

    // Drain stderr in background so FFmpeg never blocks on a full pipe.
    let stderr_task = child.stderr.take().map(|mut stderr| {
        tokio::spawn(async move {
            let mut buf = Vec::new();
            let _ = stderr.read_to_end(&mut buf).await;
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

async fn probe_duration(path: &Path) -> Result<f64> {
    let output = Command::new("ffprobe")
        .arg("-v")
        .arg("error")
        .arg("-show_entries")
        .arg("format=duration")
        .arg("-of")
        .arg("default=noprint_wrappers=1:nokey=1")
        .arg(path)
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

pub(crate) fn repair_bitrate(max_file_size: u64, duration: f64) -> String {
    let seconds = duration.max(0.1);
    let bps = ((max_file_size as f64 * 8.0 * 0.85) / seconds) as u64;
    format!("{}k", (bps / 1000).max(32))
}

fn bitrate_bits(raw: &str) -> u64 {
    let raw = raw.trim();
    let Some(unit) = raw.bytes().last() else {
        return 0;
    };
    let number = raw[..raw.len() - 1].parse::<f64>().unwrap_or(0.0);
    let mult = match unit {
        b'k' | b'K' => 1_000.0,
        b'm' | b'M' => 1_000_000.0,
        b'g' | b'G' => 1_000_000_000.0,
        _ => 1.0,
    };
    (number * mult) as u64
}

fn double_bitrate(raw: &str) -> String {
    let raw = raw.trim();
    let Some(unit) = raw.chars().last() else {
        return raw.to_string();
    };
    let number = raw[..raw.len() - unit.len_utf8()]
        .parse::<f64>()
        .unwrap_or(0.0);
    format!("{}{}", number * 2.0, unit)
}

pub(crate) fn is_text_subtitle(codec: &str) -> bool {
    matches!(
        codec,
        "subrip" | "srt" | "ass" | "ssa" | "webvtt" | "mov_text" | "text" | "ttml"
    )
}

fn scaled_width(source_width: i64, source_height: i64, target_height: i64) -> i64 {
    if source_width <= 0 || source_height <= 0 || target_height <= 0 {
        return 0;
    }
    let width = source_width * target_height / source_height;
    width - (width % 2)
}
