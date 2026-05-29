use crate::db::{MediaFingerprintRow, NewMediaMarker};
use crate::media::MediaAnalysis;
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

#[derive(Debug, Clone)]
pub struct MarkerDetectionResult {
    pub markers: Vec<NewMediaMarker>,
}

#[derive(Debug, Clone)]
pub struct GeneratedFingerprint {
    pub window_type: String,
    pub window_start_seconds: f64,
    pub window_duration_seconds: f64,
    pub fingerprint: String,
}

pub async fn detect_markers(
    analysis: &MediaAnalysis,
    existing_fingerprints: &[MediaFingerprintRow],
    new_fingerprints: &[GeneratedFingerprint],
    source_path: Option<&Path>,
    cancel: &Arc<AtomicBool>,
) -> anyhow::Result<MarkerDetectionResult> {
    if cancel.load(Ordering::Relaxed) {
        anyhow::bail!("cancelled");
    }

    let mut markers = detect_chapters(analysis).unwrap_or_default();

    for generated in new_fingerprints {
        let matching_existing: Vec<_> = existing_fingerprints
            .iter()
            .filter(|row| row.window_type == generated.window_type)
            .cloned()
            .collect();
        markers.extend(compare_fingerprints(generated, &matching_existing));
    }

    let markers = dedupe_markers(markers);
    if let Some(path) = source_path {
        Ok(MarkerDetectionResult {
            markers: snap_markers_to_boundaries(path, markers, cancel).await,
        })
    } else {
        Ok(MarkerDetectionResult { markers })
    }
}

fn detect_chapters(analysis: &MediaAnalysis) -> Option<Vec<NewMediaMarker>> {
    let chapters = parse_chapters_from_ffprobe(analysis)?;
    if chapters.is_empty() {
        return None;
    }

    let mut markers = Vec::new();
    for (title, start, end) in &chapters {
        let lower = title.to_lowercase();
        let marker_type = if lower.contains("intro") || lower.contains("opening") || lower.eq("op")
        {
            Some("intro")
        } else if lower.contains("outro")
            || lower.contains("ending")
            || lower.eq("ed")
            || lower.contains("credits")
        {
            Some("outro")
        } else if lower.contains("recap") {
            Some("recap")
        } else if lower.contains("preview") || lower.contains("next episode") {
            Some("preview")
        } else {
            None
        };

        if let Some(mt) = marker_type {
            let duration = end - start;
            if is_valid_marker_duration(mt, duration) {
                markers.push(NewMediaMarker {
                    marker_type: mt.to_string(),
                    start_seconds: *start,
                    end_seconds: *end,
                    source: "chapter".to_string(),
                    confidence: 0.9,
                });
            }
        }
    }
    if markers.is_empty() {
        None
    } else {
        Some(markers)
    }
}

fn is_valid_marker_duration(marker_type: &str, duration: f64) -> bool {
    match marker_type {
        "intro" => (15.0..=120.0).contains(&duration),
        "outro" | "credits" => (15.0..=300.0).contains(&duration),
        "recap" | "preview" => (15.0..=120.0).contains(&duration),
        _ => true,
    }
}

fn parse_chapters_from_ffprobe(analysis: &MediaAnalysis) -> Option<Vec<(String, f64, f64)>> {
    let raw = analysis.raw_ffprobe_json.as_deref()?;
    let value: serde_json::Value = serde_json::from_str(raw).ok()?;
    let chapters = value["chapters"].as_array()?;
    if chapters.is_empty() {
        return None;
    }
    let mut result = Vec::new();
    for ch in chapters {
        let title = ch["tags"]["title"].as_str().unwrap_or("");
        let start = parse_time(ch["start_time"].as_str().unwrap_or("0"))?;
        let end = parse_time(ch["end_time"].as_str().unwrap_or("0"))?;
        result.push((title.to_string(), start, end));
    }
    Some(result)
}

fn parse_time(s: &str) -> Option<f64> {
    s.parse::<f64>().ok()
}

pub fn chromaprint_available() -> bool {
    std::process::Command::new("ffmpeg")
        .arg("-version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
        && std::process::Command::new("fpcalc")
            .arg("-version")
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
}

pub fn fingerprint_windows(duration: f64) -> Vec<(String, f64, f64)> {
    if !duration.is_finite() || duration < 30.0 {
        return Vec::new();
    }
    let intro_len = (duration * 0.25).clamp(30.0, 600.0).min(duration);
    let outro_len = 600.0_f64.min(duration).max(30.0);
    let outro_start = (duration - outro_len).max(0.0);
    vec![
        ("intro".to_string(), 0.0, intro_len),
        ("outro".to_string(), outro_start, outro_len),
    ]
}

pub async fn generate_fingerprints(
    source_path: &Path,
    duration: f64,
    cancel: &Arc<AtomicBool>,
) -> anyhow::Result<Vec<GeneratedFingerprint>> {
    let mut fingerprints = Vec::new();
    for (window_type, start, window_duration) in fingerprint_windows(duration) {
        if cancel.load(Ordering::Relaxed) {
            anyhow::bail!("cancelled");
        }
        match generate_fingerprint_window(source_path, start, window_duration).await {
            Ok(fingerprint) => fingerprints.push(GeneratedFingerprint {
                window_type,
                window_start_seconds: start,
                window_duration_seconds: window_duration,
                fingerprint,
            }),
            Err(e) => tracing::warn!(
                window_type,
                error = %e,
                "chromaprint window generation skipped"
            ),
        }
    }
    Ok(fingerprints)
}

const FINGERPRINT_TIMEOUT_SECS: u64 = 120;
const SILENCE_BLACK_TIMEOUT_SECS: u64 = 30;

async fn generate_fingerprint_window(
    source_path: &Path,
    start_seconds: f64,
    duration_seconds: f64,
) -> anyhow::Result<String> {
    let audio = tokio::time::timeout(
        std::time::Duration::from_secs(FINGERPRINT_TIMEOUT_SECS),
        Command::new("ffmpeg")
            .arg("-hide_banner")
            .arg("-loglevel")
            .arg("error")
            .arg("-ss")
            .arg(format!("{start_seconds:.3}"))
            .arg("-t")
            .arg(format!("{duration_seconds:.3}"))
            .arg("-i")
            .arg(source_path)
            .arg("-map")
            .arg("0:a:0")
            .arg("-ac")
            .arg("1")
            .arg("-ar")
            .arg("11025")
            .arg("-f")
            .arg("wav")
            .arg("pipe:1")
            .output(),
    )
    .await
    .map_err(|_| {
        anyhow::anyhow!("ffmpeg audio extraction timed out after {FINGERPRINT_TIMEOUT_SECS}s")
    })?
    .map_err(|e| anyhow::anyhow!("extracting audio for chromaprint: {e}"))?;

    if !audio.status.success() {
        anyhow::bail!(
            "audio extraction for chromaprint failed: {}",
            String::from_utf8_lossy(&audio.stderr)
        );
    }

    let mut child = Command::new("fpcalc")
        .arg("-raw")
        .arg("-plain")
        .arg("-length")
        .arg(format!("{:.0}", duration_seconds.ceil()))
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| anyhow::anyhow!("starting fpcalc: {e}"))?;

    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| anyhow::anyhow!("opening fpcalc stdin"))?;
    stdin
        .write_all(&audio.stdout)
        .await
        .map_err(|e| anyhow::anyhow!("writing audio to fpcalc: {e}"))?;
    drop(stdin);

    let output = child
        .wait_with_output()
        .await
        .map_err(|e| anyhow::anyhow!("waiting for fpcalc: {e}"))?;

    if !output.status.success() {
        anyhow::bail!("fpcalc failed: {}", String::from_utf8_lossy(&output.stderr));
    }

    let fp = normalize_fingerprint_output(&String::from_utf8_lossy(&output.stdout));
    if fp.is_empty() || fp.len() < 10 {
        anyhow::bail!("chromaprint fingerprint empty or too short");
    }
    Ok(fp)
}

fn normalize_fingerprint_output(stdout: &str) -> String {
    stdout
        .lines()
        .find_map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                None
            } else if let Some(fp) = trimmed.strip_prefix("FINGERPRINT=") {
                Some(fp.trim().to_string())
            } else {
                Some(trimmed.to_string())
            }
        })
        .unwrap_or_default()
}

// Chromaprint emits one 32-bit int per ~0.1238 seconds.
const POINTS_PER_SECOND: f64 = 1.0 / 0.1238;
const HAMMING_THRESHOLD: u32 = 8;
const MIN_MATCH_POINTS: usize = 100;
const MAX_MISMATCH_GAP_POINTS: usize = 3;
const MAX_OFFSET_POINTS: i64 = 400;
const SNAP_SEARCH_SECONDS: f64 = 12.0;

pub fn compare_fingerprints(
    generated: &GeneratedFingerprint,
    existing: &[MediaFingerprintRow],
) -> Vec<NewMediaMarker> {
    let new_points = parse_fp(&generated.fingerprint);
    if new_points.len() < MIN_MATCH_POINTS {
        return Vec::new();
    }
    let mut markers = Vec::new();
    for row in existing {
        let existing_points = parse_fp(&row.fingerprint);
        if existing_points.len() < MIN_MATCH_POINTS {
            continue;
        }
        if let Some(m) = find_best_match(generated, &new_points, &existing_points) {
            markers.push(m);
        }
    }
    markers
}

fn parse_fp(fp: &str) -> Vec<u32> {
    fp.split(',')
        .filter_map(|s| s.trim().parse().ok())
        .collect()
}

fn find_best_match(
    generated: &GeneratedFingerprint,
    a: &[u32],
    b: &[u32],
) -> Option<NewMediaMarker> {
    let max_offset = MAX_OFFSET_POINTS;

    let mut best_start = 0usize;
    let mut best_len = 0usize;
    let mut best_match_points = 0usize;

    for offset in -max_offset..=max_offset {
        let a_start = (-offset).max(0) as usize;
        let b_start = offset.max(0) as usize;
        if a_start >= a.len() || b_start >= b.len() {
            continue;
        }
        let len = (a.len() - a_start).min(b.len() - b_start);
        if len < MIN_MATCH_POINTS {
            continue;
        }

        let mut run_start = a_start;
        let mut run_len = 0usize;
        let mut run_match_points = 0usize;
        let mut gap_len = 0usize;
        let mut local_best_start = a_start;
        let mut local_best_len = 0usize;
        let mut local_best_match_points = 0usize;

        for i in 0..len {
            if (a[a_start + i] ^ b[b_start + i]).count_ones() <= HAMMING_THRESHOLD {
                if run_len == 0 {
                    run_start = a_start + i;
                    run_match_points = 0;
                }
                run_len += 1;
                run_match_points += 1;
                gap_len = 0;
                if run_match_points > local_best_match_points
                    || (run_match_points == local_best_match_points && run_len > local_best_len)
                {
                    local_best_len = run_len;
                    local_best_start = run_start;
                    local_best_match_points = run_match_points;
                }
            } else if run_len > 0 && gap_len < MAX_MISMATCH_GAP_POINTS {
                run_len += 1;
                gap_len += 1;
            } else {
                run_len = 0;
                run_match_points = 0;
                gap_len = 0;
            }
        }

        if local_best_match_points > best_match_points
            || (local_best_match_points == best_match_points && local_best_len > best_len)
        {
            best_len = local_best_len;
            best_start = local_best_start;
            best_match_points = local_best_match_points;
        }
    }

    if best_match_points < MIN_MATCH_POINTS {
        return None;
    }

    let secs_per_point = 1.0 / POINTS_PER_SECOND;
    let start_secs = generated.window_start_seconds + best_start as f64 * secs_per_point;
    let end_secs = generated.window_start_seconds + (best_start + best_len) as f64 * secs_per_point;
    let marker_type = generated.window_type.as_str();

    if !is_valid_marker_duration(marker_type, end_secs - start_secs) {
        return None;
    }

    Some(NewMediaMarker {
        marker_type: marker_type.to_string(),
        start_seconds: start_secs,
        end_seconds: end_secs,
        source: "chromaprint".to_string(),
        confidence: ((best_match_points as f64 / a.len() as f64) * 2.0).clamp(0.5, 1.0),
    })
}

fn dedupe_markers(mut markers: Vec<NewMediaMarker>) -> Vec<NewMediaMarker> {
    markers.sort_by(|a, b| {
        a.marker_type
            .cmp(&b.marker_type)
            .then_with(|| a.start_seconds.total_cmp(&b.start_seconds))
            .then_with(|| b.confidence.total_cmp(&a.confidence))
    });
    let mut out: Vec<NewMediaMarker> = Vec::new();
    for marker in markers {
        if let Some(existing) = out.iter_mut().find(|m| {
            m.marker_type == marker.marker_type
                && ranges_overlap(
                    m.start_seconds,
                    m.end_seconds,
                    marker.start_seconds,
                    marker.end_seconds,
                )
        }) {
            if marker.confidence > existing.confidence {
                *existing = marker;
            }
        } else {
            out.push(marker);
        }
    }
    out.sort_by(|a, b| a.start_seconds.total_cmp(&b.start_seconds));
    out
}

fn ranges_overlap(a_start: f64, a_end: f64, b_start: f64, b_end: f64) -> bool {
    a_start < b_end && b_start < a_end
}

async fn snap_markers_to_boundaries(
    source_path: &Path,
    markers: Vec<NewMediaMarker>,
    cancel: &Arc<AtomicBool>,
) -> Vec<NewMediaMarker> {
    let mut snapped = Vec::with_capacity(markers.len());
    for marker in markers {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        snapped.push(snap_marker_to_boundaries(source_path, marker).await);
    }
    snapped
}

async fn snap_marker_to_boundaries(source_path: &Path, marker: NewMediaMarker) -> NewMediaMarker {
    if marker.source == "chapter" {
        return marker;
    }
    let start = marker.start_seconds;
    let end = marker.end_seconds;
    let search_start = (start - SNAP_SEARCH_SECONDS).max(0.0);
    let search_end = end + SNAP_SEARCH_SECONDS;
    let mut candidates = Vec::new();

    candidates.extend(
        detect_silence_points(source_path, search_start, search_end - search_start)
            .await
            .unwrap_or_default(),
    );
    candidates.extend(
        detect_black_points(source_path, search_start, search_end - search_start)
            .await
            .unwrap_or_default(),
    );

    if candidates.is_empty() {
        return marker;
    }

    let snapped_start = nearest_point(start, &candidates, SNAP_SEARCH_SECONDS).unwrap_or(start);
    let snapped_end = nearest_point(end, &candidates, SNAP_SEARCH_SECONDS).unwrap_or(end);
    if snapped_end <= snapped_start
        || !is_valid_marker_duration(&marker.marker_type, snapped_end - snapped_start)
    {
        return marker;
    }
    NewMediaMarker {
        start_seconds: snapped_start,
        end_seconds: snapped_end,
        source: if marker.source == "chromaprint" {
            "chromaprint".to_string()
        } else {
            marker.source
        },
        ..marker
    }
}

fn nearest_point(target: f64, candidates: &[f64], max_distance: f64) -> Option<f64> {
    candidates
        .iter()
        .copied()
        .filter(|p| (*p - target).abs() <= max_distance)
        .min_by(|a, b| (a - target).abs().total_cmp(&(b - target).abs()))
}

async fn detect_silence_points(
    source_path: &Path,
    start_seconds: f64,
    duration_seconds: f64,
) -> anyhow::Result<Vec<f64>> {
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(SILENCE_BLACK_TIMEOUT_SECS),
        tokio::process::Command::new("ffmpeg")
            .arg("-hide_banner")
            .arg("-ss")
            .arg(format!("{start_seconds:.3}"))
            .arg("-t")
            .arg(format!("{duration_seconds:.3}"))
            .arg("-i")
            .arg(source_path)
            .arg("-map")
            .arg("0:a:0")
            .arg("-af")
            .arg("silencedetect=noise=-32dB:d=0.25")
            .arg("-f")
            .arg("null")
            .arg("-")
            .output(),
    )
    .await
    .map_err(|_| anyhow::anyhow!("ffmpeg silence detection timed out"))??;
    if !output.status.success() {
        return Ok(Vec::new());
    }
    Ok(parse_detector_points(
        &String::from_utf8_lossy(&output.stderr),
        start_seconds,
        &["silence_start:", "silence_end:"],
    ))
}

async fn detect_black_points(
    source_path: &Path,
    start_seconds: f64,
    duration_seconds: f64,
) -> anyhow::Result<Vec<f64>> {
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(SILENCE_BLACK_TIMEOUT_SECS),
        tokio::process::Command::new("ffmpeg")
            .arg("-hide_banner")
            .arg("-ss")
            .arg(format!("{start_seconds:.3}"))
            .arg("-t")
            .arg(format!("{duration_seconds:.3}"))
            .arg("-i")
            .arg(source_path)
            .arg("-vf")
            .arg("blackdetect=d=0.25:pic_th=0.96")
            .arg("-an")
            .arg("-f")
            .arg("null")
            .arg("-")
            .output(),
    )
    .await
    .map_err(|_| anyhow::anyhow!("ffmpeg black detection timed out"))??;
    if !output.status.success() {
        return Ok(Vec::new());
    }
    Ok(parse_detector_points(
        &String::from_utf8_lossy(&output.stderr),
        start_seconds,
        &["black_start:", "black_end:"],
    ))
}

fn parse_detector_points(stderr: &str, offset: f64, keys: &[&str]) -> Vec<f64> {
    let mut points = Vec::new();
    for line in stderr.lines() {
        for key in keys {
            if let Some(value) = value_after_key(line, key) {
                points.push(offset + value);
            }
        }
    }
    points
}

fn value_after_key(line: &str, key: &str) -> Option<f64> {
    let start = line.find(key)? + key.len();
    let raw = line[start..].trim_start();
    let end = raw
        .find(|c: char| c.is_whitespace() || c == '|')
        .unwrap_or(raw.len());
    raw[..end].parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fp(points: &[u32]) -> String {
        points
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(",")
    }

    fn existing(window_type: &str, points: &[u32]) -> MediaFingerprintRow {
        MediaFingerprintRow {
            job_id: "old".into(),
            media_type: "Anime TV".into(),
            series_name: "Show".into(),
            season_number: Some(1),
            window_type: window_type.into(),
            window_start_seconds: 0.0,
            window_duration_seconds: 60.0,
            duration_seconds: 1200.0,
            fingerprint: fp(points),
            fingerprint_source: "chromaprint".into(),
            created_at: "2026-01-01 00:00:00".into(),
        }
    }

    #[test]
    fn marker_fingerprint_window_emits_outro_marker() {
        let points = vec![42_u32; 180];
        let generated = GeneratedFingerprint {
            window_type: "outro".into(),
            window_start_seconds: 900.0,
            window_duration_seconds: 60.0,
            fingerprint: fp(&points),
        };

        let markers = compare_fingerprints(&generated, &[existing("outro", &points)]);

        assert_eq!(markers.len(), 1);
        assert_eq!(markers[0].marker_type, "outro");
        assert!(markers[0].start_seconds >= 900.0);
        assert!(markers[0].end_seconds > markers[0].start_seconds);
    }

    #[test]
    fn marker_fingerprint_window_accepts_noisier_points() {
        let generated_points = vec![0_u32; 180];
        let existing_points = vec![0xff_u32; 180];
        let generated = GeneratedFingerprint {
            window_type: "intro".into(),
            window_start_seconds: 0.0,
            window_duration_seconds: 60.0,
            fingerprint: fp(&generated_points),
        };

        let markers = compare_fingerprints(&generated, &[existing("intro", &existing_points)]);

        assert_eq!(markers.len(), 1);
        assert_eq!(markers[0].marker_type, "intro");
        assert!(markers[0].end_seconds > markers[0].start_seconds);
    }

    #[test]
    fn marker_fingerprint_window_bridges_short_mismatch_gap() {
        let generated_points = vec![10_u32; 130];
        let mut existing_points = generated_points.clone();
        existing_points[50..53].fill(u32::MAX);
        let generated = GeneratedFingerprint {
            window_type: "intro".into(),
            window_start_seconds: 0.0,
            window_duration_seconds: 60.0,
            fingerprint: fp(&generated_points),
        };

        let markers = compare_fingerprints(&generated, &[existing("intro", &existing_points)]);

        assert_eq!(markers.len(), 1);
        assert_eq!(markers[0].marker_type, "intro");
        assert!(markers[0].end_seconds > markers[0].start_seconds);
    }

    #[test]
    fn marker_fingerprint_window_ignores_wrong_window_type() {
        let points = vec![7_u32; 180];
        let generated = GeneratedFingerprint {
            window_type: "intro".into(),
            window_start_seconds: 0.0,
            window_duration_seconds: 60.0,
            fingerprint: fp(&points),
        };
        let existing = existing("outro", &points);
        let filtered: Vec<_> = [existing]
            .into_iter()
            .filter(|row| row.window_type == generated.window_type)
            .collect();

        assert!(compare_fingerprints(&generated, &filtered).is_empty());
    }

    #[test]
    fn normalize_fingerprint_output_accepts_plain_and_keyed_output() {
        assert_eq!(normalize_fingerprint_output("1,2,3\n"), "1,2,3");
        assert_eq!(normalize_fingerprint_output("FINGERPRINT=4,5,6\n"), "4,5,6");
    }
}
