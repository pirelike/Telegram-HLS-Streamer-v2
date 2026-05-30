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
    pub source_bitrate: i64,
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
    pub source_bitrate: i64,
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
            source_bitrate: 0,
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
    #[serde(default)]
    pub users: Vec<UserRow>,
    #[serde(default)]
    pub user_favorites: Vec<FavoriteRow>,
    #[serde(default)]
    pub user_watchlist: Vec<WatchlistRow>,
    #[serde(default)]
    pub user_ratings: Vec<RatingRow>,
    #[serde(default)]
    pub user_preferences: Vec<PreferenceRow>,
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
    #[serde(default)]
    pub user_id: Option<String>,
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
    pub user_id: Option<String>,
    pub job_id: String,
    pub position_seconds: f64,
    pub duration_seconds: f64,
}

// --- Users and per-user data ---

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UserRow {
    pub user_id: String,
    pub username: String,
    pub password_hash: String,
    pub is_admin: bool,
    pub created_at: String,
    pub last_seen_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionRow {
    pub token: String,
    pub user_id: String,
    pub created_at: String,
    pub expires_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FavoriteRow {
    pub user_id: String,
    pub job_id: String,
    pub added_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WatchlistRow {
    pub user_id: String,
    pub job_id: String,
    pub added_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RatingRow {
    pub user_id: String,
    pub job_id: String,
    pub liked: bool,
    pub rated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PreferenceRow {
    pub user_id: String,
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UserJobRow {
    pub job: JobRow,
    pub marked_at: String,
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
        params.push(format!("%{}%", escape_like(search)));
        clauses.push(format!(
            "(filename LIKE ?{} ESCAPE '\\' OR series_name LIKE ?{} ESCAPE '\\')",
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

fn escape_like(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        if matches!(c, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}
