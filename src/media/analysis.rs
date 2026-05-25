use std::path::Path;

use anyhow::{bail, Context, Result};
use serde_json::Value;
use tokio::process::Command;

use super::models::{AudioStream, MediaAnalysis, SubtitleStream, VideoStream};

pub async fn analyze_media(path: &Path) -> Result<MediaAnalysis> {
    let output = Command::new("ffprobe")
        .arg("-v")
        .arg("error")
        .arg("-show_format")
        .arg("-show_streams")
        .arg("-show_chapters")
        .arg("-print_format")
        .arg("json")
        .arg(path)
        .output()
        .await
        .context("running ffprobe")?;
    if !output.status.success() {
        bail!(
            "ffprobe failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let raw = String::from_utf8_lossy(&output.stdout).to_string();
    let value: Value = serde_json::from_str(&raw).context("parsing ffprobe json")?;
    let mut analysis = analysis_from_ffprobe(path, &value).await?;
    analysis.raw_ffprobe_json = Some(raw);
    Ok(analysis)
}

pub(crate) async fn analysis_from_ffprobe(path: &Path, value: &Value) -> Result<MediaAnalysis> {
    let file_size = tokio::fs::metadata(path)
        .await
        .map(|m| m.len())
        .unwrap_or_else(|_| {
            value["format"]["size"]
                .as_str()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0)
        });
    let duration = value["format"]["duration"]
        .as_str()
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(0.0);
    if duration <= 0.0 {
        bail!("file reports zero or unknown duration");
    }
    let mut video_streams = Vec::new();
    let mut audio_streams = Vec::new();
    let mut subtitle_streams = Vec::new();

    let streams = value["streams"].as_array().cloned().unwrap_or_default();
    let has_real_video = streams.iter().any(|stream| {
        stream["codec_type"].as_str() == Some("video")
            && stream["disposition"]["attached_pic"].as_i64().unwrap_or(0) != 1
            && !matches!(stream["codec_name"].as_str().unwrap_or(""), "mjpeg" | "png")
    });
    for stream in streams {
        let codec_type = stream["codec_type"].as_str().unwrap_or("");
        let codec_name = str_field(&stream, "codec_name", "unknown");
        let tags = &stream["tags"];
        let language = tags["language"].as_str().unwrap_or("und").to_string();
        let title = tags["title"].as_str().unwrap_or("").to_string();
        match codec_type {
            "video" => {
                let attached_pic = stream["disposition"]["attached_pic"].as_i64().unwrap_or(0) == 1;
                let album_art_codec = matches!(codec_name.as_str(), "mjpeg" | "png");
                if has_real_video && (attached_pic || album_art_codec) {
                    continue;
                }
                video_streams.push(VideoStream {
                    index: int_field(&stream, "index", -1),
                    codec_name,
                    width: int_field(&stream, "width", 0),
                    height: int_field(&stream, "height", 0),
                    bit_rate: str_field(&stream, "bit_rate", "0"),
                    language,
                    title,
                });
            }
            "audio" => audio_streams.push(AudioStream {
                index: int_field(&stream, "index", -1),
                codec_name,
                channels: int_field(&stream, "channels", 2),
                sample_rate: str_field(&stream, "sample_rate", "0"),
                bit_rate: str_field(&stream, "bit_rate", "0"),
                channel_layout: str_field(&stream, "channel_layout", ""),
                language,
                title,
            }),
            "subtitle" => subtitle_streams.push(SubtitleStream {
                index: int_field(&stream, "index", -1),
                codec_name,
                language,
                title,
            }),
            _ => {}
        }
    }

    if video_streams.is_empty() {
        bail!("no video stream found");
    }

    Ok(MediaAnalysis {
        file_path: path.to_path_buf(),
        duration,
        file_size,
        video_streams,
        audio_streams,
        subtitle_streams,
        raw_ffprobe_json: None,
    })
}

pub(super) fn int_field(value: &Value, key: &str, default: i64) -> i64 {
    value[key]
        .as_i64()
        .or_else(|| value[key].as_str().and_then(|s| s.parse().ok()))
        .unwrap_or(default)
}

pub(super) fn str_field(value: &Value, key: &str, default: &str) -> String {
    value[key].as_str().unwrap_or(default).to_string()
}
