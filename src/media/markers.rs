use crate::db::{MediaFingerprintRow, NewMediaMarker};
use crate::media::MediaAnalysis;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct MarkerDetectionResult {
    pub markers: Vec<NewMediaMarker>,
}

pub async fn detect_markers(
    analysis: &MediaAnalysis,
    _media_type: &str,
    _series_name: &str,
    _season_number: Option<i64>,
    existing_fingerprints: &[MediaFingerprintRow],
    new_fingerprint: Option<&str>,
    cancel: &Arc<AtomicBool>,
) -> anyhow::Result<MarkerDetectionResult> {
    let mut markers = Vec::new();

    if cancel.load(Ordering::Relaxed) {
        anyhow::bail!("cancelled");
    }

    if let Some(chapter_markers) = detect_chapters(analysis) {
        markers.extend(chapter_markers);
    }

    if let Some(new_fp) = new_fingerprint {
        if !existing_fingerprints.is_empty() {
            let chromaprint_markers =
                compare_fingerprints(new_fp, existing_fingerprints, analysis.duration);
            markers.extend(chromaprint_markers);
        }
    }

    Ok(MarkerDetectionResult { markers })
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
        "intro" => duration >= 15.0 && duration <= 120.0,
        "outro" | "credits" => duration >= 15.0 && duration <= 300.0,
        "recap" | "preview" => duration >= 15.0 && duration <= 120.0,
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
        .args([
            "-f",
            "lavfi",
            "-i",
            "anullsrc=r=44100:cl=mono",
            "-t",
            "0.001",
            "-f",
            "chromaprint",
            "-fp_format",
            "raw",
            "-",
        ])
        .output()
        .is_ok()
}

pub async fn generate_fingerprint(
    source_path: &std::path::Path,
    duration: f64,
    _cancel: &Arc<AtomicBool>,
) -> anyhow::Result<String> {
    let dur_secs = (duration * 0.5).min(120.0).max(30.0) as u32;

    let output = tokio::process::Command::new("ffmpeg")
        .arg("-y")
        .arg("-ss")
        .arg("0")
        .arg("-t")
        .arg(dur_secs.to_string())
        .arg("-i")
        .arg(source_path)
        .arg("-map")
        .arg("0:a:0")
        .arg("-f")
        .arg("chromaprint")
        .arg("-fp_format")
        .arg("raw")
        .arg("-")
        .output()
        .await
        .map_err(|e| anyhow::anyhow!("chromaprint ffmpeg: {e}"))?;

    if !output.status.success() {
        anyhow::bail!(
            "chromaprint failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let fp = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if fp.is_empty() || fp.len() < 10 {
        anyhow::bail!("chromaprint fingerprint empty or too short");
    }
    Ok(fp)
}

pub fn compare_fingerprints(
    new_fp: &str,
    existing: &[MediaFingerprintRow],
    new_duration: f64,
) -> Vec<NewMediaMarker> {
    let mut markers = Vec::new();
    for existing_fp in existing {
        let similarity = fingerprint_similarity(new_fp, &existing_fp.fingerprint);
        if similarity < 0.3 {
            continue;
        }
        let window = (new_duration * 0.25).min(600.0);
        if let Some(marker) = score_intro_marker(similarity, window) {
            markers.push(marker);
        }
    }
    markers
}

fn fingerprint_similarity(a: &str, b: &str) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let a_ints: Vec<u32> = a.split(',').filter_map(|s| s.trim().parse().ok()).collect();
    let b_ints: Vec<u32> = b.split(',').filter_map(|s| s.trim().parse().ok()).collect();
    let min_len = a_ints.len().min(b_ints.len());
    if min_len == 0 {
        return 0.0;
    }
    let mut matches = 0_usize;
    for i in 0..min_len {
        if (a_ints[i] as i64 - b_ints[i] as i64).abs() < 0x1000 {
            matches += 1;
        }
    }
    matches as f64 / min_len as f64
}

fn score_intro_marker(similarity: f64, window: f64) -> Option<NewMediaMarker> {
    if similarity < 0.5 {
        return None;
    }
    let confidence = similarity.clamp(0.0, 1.0);
    let intro_duration = 90.0_f64.min(window * 0.15);
    Some(NewMediaMarker {
        marker_type: "intro".to_string(),
        start_seconds: 0.0,
        end_seconds: intro_duration,
        source: "chromaprint".to_string(),
        confidence,
    })
}
