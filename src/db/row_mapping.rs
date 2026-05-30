use super::models::*;

pub(super) const JOB_SELECT_SQL: &str = "SELECT job_id, filename, duration, file_size, video_codec, video_width, video_height, status, created_at, media_type, series_name, has_thumbnail, is_series, season_number, episode_number, part_number, error, source_path, episode_title, source_bitrate FROM jobs";
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
        source_bitrate: row.get(19)?,
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
        user_id: row.get(1)?,
        job_id: row.get(2)?,
        position_seconds: row.get(3)?,
        duration_seconds: row.get(4)?,
        progress_pct: row.get(5)?,
        completed: row.get::<_, i64>(6)? == 1,
        updated_at: row.get(7)?,
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
