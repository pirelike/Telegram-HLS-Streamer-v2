use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaAnalysis {
    pub file_path: std::path::PathBuf,
    pub duration: f64,
    pub file_size: u64,
    pub video_streams: Vec<VideoStream>,
    pub audio_streams: Vec<AudioStream>,
    pub subtitle_streams: Vec<SubtitleStream>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoStream {
    pub index: i64,
    pub codec_name: String,
    pub width: i64,
    pub height: i64,
    pub bit_rate: String,
    pub language: String,
    pub title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioStream {
    pub index: i64,
    pub codec_name: String,
    pub channels: i64,
    pub sample_rate: String,
    pub bit_rate: String,
    pub channel_layout: String,
    pub language: String,
    pub title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubtitleStream {
    pub index: i64,
    pub codec_name: String,
    pub language: String,
    pub title: String,
}

#[derive(Debug, Clone)]
pub struct ProcessingResult {
    pub job_id: String,
    pub output_dir: std::path::PathBuf,
    pub video_playlists: Vec<VideoPlaylist>,
    pub audio_playlists: Vec<AudioPlaylist>,
    pub subtitle_files: Vec<SubtitleFile>,
    pub segment_durations: std::collections::HashMap<String, f64>,
    pub thumbnail_path: Option<std::path::PathBuf>,
    pub oversized_segments_repaired: usize,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct VideoPlaylist {
    pub playlist_path: std::path::PathBuf,
    pub tier_dir: String,
    pub width: i64,
    pub height: i64,
    pub bitrate: String,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct AudioPlaylist {
    pub playlist_path: std::path::PathBuf,
    pub audio_dir: String,
    pub language: String,
    pub title: String,
    pub channels: i64,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct SubtitleFile {
    pub vtt_path: std::path::PathBuf,
    pub sub_dir: String,
    pub language: String,
    pub title: String,
    pub enum_idx: usize,
    pub original_stream_idx: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoTier {
    pub index: usize,
    pub height: i64,
    pub bitrate: String,
    pub copy: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedEncoder {
    pub name: String,
    pub vaapi_device: Option<String>,
}
