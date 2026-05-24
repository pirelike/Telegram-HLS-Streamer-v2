use std::path::PathBuf;

use serde::{Deserialize, Serialize};

// --- Public model types ---

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct JobRow {
    pub job_id: String,
    pub filename: String,
    pub duration: f64,
    pub file_size: i64,
    pub video_codec: String,
    pub video_width: i64,
    pub video_height: i64,
    pub status: String,
    pub created_at: String,
    pub media_type: String,
    pub series_name: String,
    pub has_thumbnail: bool,
    pub is_series: bool,
    pub season_number: Option<i64>,
    pub episode_number: Option<i64>,
    pub part_number: Option<i64>,
    pub error: Option<String>,
    pub source_path: Option<String>,
    pub episode_title: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NewJob {
    pub job_id: String,
    pub filename: String,
    pub duration: f64,
    pub file_size: i64,
    pub video_codec: String,
    pub video_width: i64,
    pub video_height: i64,
    pub status: String,
    pub media_type: String,
    pub series_name: String,
    pub has_thumbnail: bool,
    pub is_series: bool,
    pub season_number: Option<i64>,
    pub episode_number: Option<i64>,
    pub part_number: Option<i64>,
    /// Upload-local filename label only. This must not contain path separators or `..`;
    /// the DB enforces the same invariant so reprocess cannot escape uploads_dir.
    pub source_path: Option<String>,
}

impl NewJob {
    pub fn complete(job_id: impl Into<String>, filename: impl Into<String>) -> Self {
        Self {
            job_id: job_id.into(),
            filename: filename.into(),
            duration: 0.0,
            file_size: 0,
            video_codec: "unknown".into(),
            video_width: 0,
            video_height: 0,
            status: "complete".into(),
            media_type: "Film".into(),
            series_name: String::new(),
            has_thumbnail: false,
            is_series: false,
            season_number: None,
            episode_number: None,
            part_number: None,
            source_path: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TrackRow {
    pub id: i64,
    pub job_id: String,
    pub track_type: String,
    pub track_index: i64,
    pub codec: String,
    pub language: String,
    pub title: String,
    pub channels: i64,
    pub width: i64,
    pub height: i64,
    pub bitrate: String,
    pub original_stream_index: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NewTrack {
    pub track_type: String,
    pub track_index: i64,
    pub codec: String,
    pub language: String,
    pub title: String,
    pub channels: i64,
    pub width: i64,
    pub height: i64,
    pub bitrate: String,
    pub original_stream_index: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SegmentRow {
    pub id: i64,
    pub job_id: String,
    pub segment_key: String,
    pub file_id: String,
    pub bot_index: i64,
    pub file_size: i64,
    pub duration: Option<f64>,
    #[serde(default)]
    pub is_split: bool,
    #[serde(default)]
    pub encryption_nonce: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NewSegment {
    pub segment_key: String,
    pub file_id: String,
    pub bot_index: i64,
    pub file_size: i64,
    pub duration: Option<f64>,
    #[serde(default)]
    pub is_split: bool,
    #[serde(default)]
    pub encryption_nonce: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SegmentLookup {
    pub file_id: String,
    pub bot_index: i64,
    #[serde(default)]
    pub is_split: bool,
    #[serde(default)]
    pub encryption_nonce: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SegmentPartRow {
    pub id: i64,
    pub job_id: String,
    pub segment_key: String,
    pub part_index: i64,
    pub file_id: String,
    pub bot_index: i64,
    pub file_size: i64,
    #[serde(default)]
    pub encryption_nonce: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NewSegmentPart {
    pub job_id: String,
    pub segment_key: String,
    pub part_index: i64,
    pub file_id: String,
    pub bot_index: i64,
    pub file_size: i64,
    #[serde(default)]
    pub encryption_nonce: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SegmentPartLookup {
    pub file_id: String,
    pub bot_index: i64,
    pub file_size: i64,
    pub part_index: i64,
    #[serde(default)]
    pub encryption_nonce: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SegmentPartExportRow {
    pub id: i64,
    pub job_id: String,
    pub segment_key: String,
    pub part_index: i64,
    pub file_id: String,
    pub bot_index: i64,
    pub file_size: i64,
    #[serde(default)]
    pub encryption_nonce: Option<String>,
}

fn double_option<'de, T, D>(de: D) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    Option::<T>::deserialize(de).map(Some)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct JobMetadataUpdate {
    pub filename: Option<String>,
    pub media_type: Option<String>,
    pub series_name: Option<String>,
    pub is_series: Option<bool>,
    #[serde(default, deserialize_with = "double_option")]
    pub season_number: Option<Option<i64>>,
    #[serde(default, deserialize_with = "double_option")]
    pub episode_number: Option<Option<i64>>,
    #[serde(default, deserialize_with = "double_option")]
    pub part_number: Option<Option<i64>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct JobListFilter {
    pub limit: i64,
    pub offset: i64,
    pub search: Option<String>,
    pub category: Option<String>,
    pub series_name: Option<String>,
    pub season_number: Option<i64>,
    pub season_number_is_null: bool,
}

impl Default for JobListFilter {
    fn default() -> Self {
        Self {
            limit: 50,
            offset: 0,
            search: None,
            category: None,
            series_name: None,
            season_number: None,
            season_number_is_null: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SeriesGroupRow {
    pub series_name: String,
    pub media_type: String,
    pub episode_count: i64,
    pub last_updated: String,
    pub job_id: String,
    pub has_thumbnail: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SeasonGroupRow {
    pub series_name: String,
    pub season_number: Option<i64>,
    pub episode_count: i64,
    pub last_updated: String,
    pub job_id: String,
    pub has_thumbnail: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DbBotRow {
    pub id: i64,
    pub token: String,
    pub channel_id: i64,
    pub label: String,
    pub enabled: bool,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BotWorkload {
    pub segment_count: i64,
    pub total_bytes: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DbExport {
    pub version: i64,
    pub schema_revision: i64,
    pub jobs: Vec<JobRow>,
    pub tracks: Vec<TrackRow>,
    pub segments: Vec<SegmentRow>,
    #[serde(default)]
    pub segment_parts: Vec<SegmentPartExportRow>,
    #[serde(default)]
    pub external_metadata: Vec<ExternalMetadataRow>,
    #[serde(default)]
    pub job_metadata_links: Vec<JobMetadataLinkRow>,
    #[serde(default)]
    pub series_metadata_links: Vec<SeriesMetadataLinkRow>,
    #[serde(default)]
    pub playback_progress: Vec<PlaybackProgressRow>,
    #[serde(default)]
    pub media_markers: Vec<MediaMarkerRow>,
    #[serde(default)]
    pub media_fingerprints: Vec<MediaFingerprintRow>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MergeResult {
    pub merged_jobs: usize,
    pub merged_segments: usize,
    pub merged_segment_parts: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReplaceDatabaseResult {
    pub backup_path: PathBuf,
    pub schema_revision: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DatabaseBackupResult {
    pub backup_path: PathBuf,
    pub size_bytes: u64,
    pub schema_revision: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DbSyncSnapshotRow {
    pub id: String,
    pub created_at: String,
    pub schema_revision: i64,
    pub size_bytes: i64,
    pub status: String,
    pub last_error: Option<String>,
}

// --- SQL constants ---

pub(super) const JOB_SELECT_SQL: &str = "SELECT job_id, filename, duration, file_size, video_codec, video_width, video_height, status, created_at, media_type, series_name, has_thumbnail, is_series, season_number, episode_number, part_number, error, source_path, episode_title FROM jobs";
pub(super) const TRACK_SELECT_SQL: &str = "SELECT id, job_id, track_type, track_index, codec, language, title, channels, width, height, bitrate, original_stream_index FROM tracks";
pub(super) const SEGMENT_SELECT_SQL: &str =
    "SELECT id, job_id, segment_key, file_id, bot_index, file_size, duration, is_split, encryption_nonce FROM segments";
pub(super) const SEGMENT_PART_SELECT_SQL: &str =
    "SELECT id, job_id, segment_key, part_index, file_id, bot_index, file_size, encryption_nonce FROM segment_parts";

// --- Row mappers ---

pub(super) fn job_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<JobRow> {
    Ok(JobRow {
        job_id: row.get(0)?,
        filename: row.get(1)?,
        duration: row.get(2)?,
        file_size: row.get(3)?,
        video_codec: row.get(4)?,
        video_width: row.get(5)?,
        video_height: row.get(6)?,
        status: row.get(7)?,
        created_at: row.get(8)?,
        media_type: row.get(9)?,
        series_name: row.get(10)?,
        has_thumbnail: row.get::<_, i64>(11)? == 1,
        is_series: row.get::<_, i64>(12)? == 1,
        season_number: row.get(13)?,
        episode_number: row.get(14)?,
        part_number: row.get(15)?,
        error: row.get(16)?,
        source_path: row.get(17)?,
        episode_title: row.get(18)?,
    })
}

pub(super) fn track_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TrackRow> {
    Ok(TrackRow {
        id: row.get(0)?,
        job_id: row.get(1)?,
        track_type: row.get(2)?,
        track_index: row.get(3)?,
        codec: row.get(4)?,
        language: row.get(5)?,
        title: row.get(6)?,
        channels: row.get(7)?,
        width: row.get(8)?,
        height: row.get(9)?,
        bitrate: row.get(10)?,
        original_stream_index: row.get(11)?,
    })
}

pub(super) fn segment_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SegmentRow> {
    Ok(SegmentRow {
        id: row.get(0)?,
        job_id: row.get(1)?,
        segment_key: row.get(2)?,
        file_id: row.get(3)?,
        bot_index: row.get(4)?,
        file_size: row.get(5)?,
        duration: row.get(6)?,
        is_split: row.get::<_, i64>(7)? == 1,
        encryption_nonce: row.get(8)?,
    })
}

pub(super) fn segment_part_export_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<SegmentPartExportRow> {
    Ok(SegmentPartExportRow {
        id: row.get(0)?,
        job_id: row.get(1)?,
        segment_key: row.get(2)?,
        part_index: row.get(3)?,
        file_id: row.get(4)?,
        bot_index: row.get(5)?,
        file_size: row.get(6)?,
        encryption_nonce: row.get(7)?,
    })
}

pub(super) const EXTERNAL_METADATA_SELECT_SQL: &str =
    "SELECT id, provider, provider_id, media_kind, title, original_title, overview, poster_url, backdrop_url, release_date, year, rating, raw_json, fetched_at FROM external_metadata";

pub(super) fn external_metadata_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<ExternalMetadataRow> {
    Ok(ExternalMetadataRow {
        id: row.get(0)?,
        provider: row.get(1)?,
        provider_id: row.get(2)?,
        media_kind: row.get(3)?,
        title: row.get(4)?,
        original_title: row.get(5)?,
        overview: row.get(6)?,
        poster_url: row.get(7)?,
        backdrop_url: row.get(8)?,
        release_date: row.get(9)?,
        year: row.get(10)?,
        rating: row.get(11)?,
        raw_json: row.get(12)?,
        fetched_at: row.get(13)?,
    })
}

pub(super) fn job_metadata_link_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<JobMetadataLinkRow> {
    Ok(JobMetadataLinkRow {
        job_id: row.get(0)?,
        metadata_id: row.get(1)?,
        role: row.get(2)?,
        created_at: row.get(3)?,
    })
}

pub(super) fn series_metadata_link_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<SeriesMetadataLinkRow> {
    Ok(SeriesMetadataLinkRow {
        media_type: row.get(0)?,
        series_name: row.get(1)?,
        metadata_id: row.get(2)?,
        created_at: row.get(3)?,
    })
}

pub(super) fn playback_progress_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<PlaybackProgressRow> {
    Ok(PlaybackProgressRow {
        client_id: row.get(0)?,
        job_id: row.get(1)?,
        position_seconds: row.get(2)?,
        duration_seconds: row.get(3)?,
        progress_pct: row.get(4)?,
        completed: row.get::<_, i64>(5)? == 1,
        updated_at: row.get(6)?,
    })
}

pub(super) fn media_marker_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<MediaMarkerRow> {
    Ok(MediaMarkerRow {
        id: row.get(0)?,
        job_id: row.get(1)?,
        marker_type: row.get(2)?,
        start_seconds: row.get(3)?,
        end_seconds: row.get(4)?,
        source: row.get(5)?,
        confidence: row.get(6)?,
        enabled: row.get::<_, i64>(7)? == 1,
        created_at: row.get(8)?,
        updated_at: row.get(9)?,
    })
}

pub(super) fn media_fingerprint_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<MediaFingerprintRow> {
    Ok(MediaFingerprintRow {
        job_id: row.get(0)?,
        media_type: row.get(1)?,
        series_name: row.get(2)?,
        season_number: row.get(3)?,
        window_type: row.get(4)?,
        window_start_seconds: row.get(5)?,
        window_duration_seconds: row.get(6)?,
        duration_seconds: row.get(7)?,
        fingerprint: row.get(8)?,
        fingerprint_source: row.get(9)?,
        created_at: row.get(10)?,
    })
}

// --- External metadata ---

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExternalMetadataRow {
    pub id: i64,
    pub provider: String,
    pub provider_id: String,
    pub media_kind: String,
    pub title: String,
    pub original_title: String,
    pub overview: String,
    pub poster_url: String,
    pub backdrop_url: String,
    pub release_date: String,
    pub year: Option<i64>,
    pub rating: Option<f64>,
    pub raw_json: String,
    pub fetched_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NewExternalMetadata {
    pub provider: String,
    pub provider_id: String,
    pub media_kind: String,
    pub title: String,
    pub original_title: String,
    pub overview: String,
    pub poster_url: String,
    pub backdrop_url: String,
    pub release_date: String,
    pub year: Option<i64>,
    pub rating: Option<f64>,
    pub raw_json: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct JobMetadataLinkRow {
    pub job_id: String,
    pub metadata_id: i64,
    pub role: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SeriesMetadataLinkRow {
    pub media_type: String,
    pub series_name: String,
    pub metadata_id: i64,
    pub created_at: String,
}

// --- Playback progress ---

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PlaybackProgressRow {
    pub client_id: String,
    pub job_id: String,
    pub position_seconds: f64,
    pub duration_seconds: f64,
    pub progress_pct: i64,
    pub completed: bool,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NewPlaybackProgress {
    pub client_id: String,
    pub job_id: String,
    pub position_seconds: f64,
    pub duration_seconds: f64,
}

// --- Media markers ---

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MediaMarkerRow {
    pub id: i64,
    pub job_id: String,
    pub marker_type: String,
    pub start_seconds: f64,
    pub end_seconds: f64,
    pub source: String,
    pub confidence: f64,
    pub enabled: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NewMediaMarker {
    pub marker_type: String,
    pub start_seconds: f64,
    pub end_seconds: f64,
    pub source: String,
    pub confidence: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MediaFingerprintRow {
    pub job_id: String,
    pub media_type: String,
    pub series_name: String,
    pub season_number: Option<i64>,
    #[serde(default = "default_intro_window_type")]
    pub window_type: String,
    #[serde(default)]
    pub window_start_seconds: f64,
    #[serde(default)]
    pub window_duration_seconds: f64,
    pub duration_seconds: f64,
    pub fingerprint: String,
    pub fingerprint_source: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NewMediaFingerprint {
    pub job_id: String,
    pub media_type: String,
    pub series_name: String,
    pub season_number: Option<i64>,
    pub window_type: String,
    pub window_start_seconds: f64,
    pub window_duration_seconds: f64,
    pub duration_seconds: f64,
    pub fingerprint: String,
    pub fingerprint_source: String,
}

fn default_intro_window_type() -> String {
    "intro".into()
}

// --- Helpers used across sub-modules ---

pub(super) fn normalize_job_metadata(job: &mut NewJob) {
    if !matches!(
        job.media_type.as_str(),
        "Film" | "Series" | "Anime Film" | "Anime TV" | "Anime"
    ) {
        job.media_type = "Film".into();
    }
    if !job.is_series {
        job.season_number = None;
        job.episode_number = None;
        job.part_number = None;
    }
}

pub(super) fn bool_to_i64(v: bool) -> i64 {
    if v {
        1
    } else {
        0
    }
}

pub(super) fn job_filter_sql(filter: &JobListFilter) -> (String, Vec<String>) {
    let mut clauses = Vec::new();
    let mut params = Vec::new();
    if let Some(search) = filter.search.as_ref().filter(|s| !s.is_empty()) {
        params.push(format!("%{search}%"));
        clauses.push(format!(
            "(filename LIKE ?{} OR series_name LIKE ?{})",
            params.len(),
            params.len()
        ));
    }
    if let Some(category) = filter.category.as_ref().filter(|s| !s.is_empty()) {
        params.push(category.clone());
        clauses.push(format!("media_type = ?{}", params.len()));
    }
    if let Some(series_name) = filter.series_name.as_ref().filter(|s| !s.is_empty()) {
        params.push(series_name.clone());
        clauses.push(format!("series_name = ?{}", params.len()));
    }
    if let Some(season_number) = filter.season_number {
        params.push(season_number.to_string());
        clauses.push(format!("season_number = ?{}", params.len()));
    } else if filter.season_number_is_null {
        clauses.push("season_number IS NULL".to_string());
    }
    if clauses.is_empty() {
        (String::new(), params)
    } else {
        (format!("WHERE {}", clauses.join(" AND ")), params)
    }
}
