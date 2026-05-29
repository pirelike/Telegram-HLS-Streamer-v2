use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use tokio::process::Command;

use super::ffmpeg::run_ffmpeg_cancellable;
use super::models::*;

pub(super) async fn extract_thumbnail(
    analysis: &MediaAnalysis,
    output_dir: &Path,
    cancel: &Arc<AtomicBool>,
    timeout_secs: u64,
) -> Option<PathBuf> {
    let dir = output_dir.join("thumbnail");
    if tokio::fs::create_dir_all(&dir).await.is_err() {
        return None;
    }
    let path = dir.join("thumbnail.jpg");
    let seek = analysis.duration.mul_add(0.10, 0.0).max(2.0).to_string();
    let mut cmd = Command::new("ffmpeg");
    cmd.arg("-y")
        .arg("-nostdin")
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
    match run_ffmpeg_cancellable(&mut cmd, cancel, timeout_secs).await {
        Ok(()) => Some(path),
        Err(e) => {
            tracing::warn!(error = %e, "thumbnail extraction failed");
            None
        }
    }
}

pub(super) async fn collect_segment_durations(output_dir: &Path) -> Result<HashMap<String, f64>> {
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
                probe_duration(&path)
                    .await
                    .with_context(|| format!("probing duration for {}", path.display()))?
            } else {
                0.0
            };
            out.insert(key, duration);
        }
    }
    Ok(out)
}

pub(super) async fn sorted_files_with_ext(dir: &Path, ext: &str) -> Result<Vec<PathBuf>> {
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
    path.to_string_lossy().into_owned()
}

const PROBE_TIMEOUT_SECS: u64 = 30;

pub(super) async fn probe_duration(path: &Path) -> Result<f64> {
    let path_arg = fmp4_input_arg(path);
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(PROBE_TIMEOUT_SECS),
        Command::new("ffprobe")
            .arg("-v")
            .arg("error")
            .arg("-show_entries")
            .arg("format=duration")
            .arg("-of")
            .arg("default=noprint_wrappers=1:nokey=1")
            .arg(&path_arg)
            .output(),
    )
    .await
    .map_err(|_| {
        anyhow::anyhow!("ffprobe probe_duration timed out after {PROBE_TIMEOUT_SECS}s")
    })??;
    if !output.status.success() {
        bail!("ffprobe duration failed");
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .unwrap_or(0.0))
}
