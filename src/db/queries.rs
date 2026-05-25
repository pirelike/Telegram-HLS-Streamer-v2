use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};

use super::models::*;
use super::row_mapping::*;

pub fn save_job(
    conn: &mut Connection,
    job: &NewJob,
    tracks: &[NewTrack],
    segments: &[NewSegment],
    segment_parts: &[NewSegmentPart],
) -> Result<()> {
    let tx = conn.transaction()?;
    let mut job = job.clone();
    normalize_job_metadata(&mut job);
    tx.execute(
        "INSERT INTO jobs(
            job_id, filename, duration, file_size, video_codec, video_width, video_height, status,
            media_type, series_name, has_thumbnail, is_series, season_number, episode_number, part_number,
            source_path, source_bitrate, created_at_unix
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, strftime('%s','now'))
         ON CONFLICT(job_id) DO UPDATE SET
            filename = excluded.filename,
            duration = excluded.duration,
            file_size = excluded.file_size,
            video_codec = excluded.video_codec,
            video_width = excluded.video_width,
            video_height = excluded.video_height,
            status = excluded.status,
            media_type = excluded.media_type,
            series_name = excluded.series_name,
            has_thumbnail = excluded.has_thumbnail,
            is_series = excluded.is_series,
            season_number = excluded.season_number,
            episode_number = excluded.episode_number,
            part_number = excluded.part_number,
            source_path = excluded.source_path,
            source_bitrate = excluded.source_bitrate",
        params![
            job.job_id,
            job.filename,
            job.duration,
            job.file_size,
            job.video_codec,
            job.video_width,
            job.video_height,
            job.status,
            job.media_type,
            job.series_name,
            bool_to_i64(job.has_thumbnail),
            bool_to_i64(job.is_series),
            job.season_number,
            job.episode_number,
            job.part_number,
            job.source_path,
            job.source_bitrate,
        ],
    )?;
    tx.execute("DELETE FROM tracks WHERE job_id = ?1", params![job.job_id])?;
    tx.execute(
        "DELETE FROM segments WHERE job_id = ?1",
        params![job.job_id],
    )?;
    tx.execute(
        "DELETE FROM segment_parts WHERE job_id = ?1",
        params![job.job_id],
    )?;

    for track in tracks {
        let (mode, bitrate_bps) = super::migrations::track_mode_and_bps(&track.bitrate);
        tx.execute(
            "INSERT INTO tracks(
                job_id, track_type, track_index, codec, language, title, channels,
                width, height, bitrate, original_stream_index, mode, bitrate_bps
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                job.job_id,
                track.track_type,
                track.track_index,
                track.codec,
                track.language,
                track.title,
                track.channels,
                track.width,
                track.height,
                track.bitrate,
                track.original_stream_index,
                mode,
                bitrate_bps,
            ],
        )?;
    }
    for segment in segments {
        let (prefix, name) = split_segment_key(&segment.segment_key);
        tx.execute(
            "INSERT INTO segments(job_id, segment_key, file_id, bot_index, file_size, duration, is_split, prefix, name, encryption_nonce)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                job.job_id,
                segment.segment_key,
                segment.file_id,
                segment.bot_index,
                segment.file_size,
                segment.duration,
                bool_to_i64(segment.is_split),
                prefix,
                name,
                segment.encryption_nonce,
            ],
        )?;
    }
    for part in segment_parts {
        let (prefix, name) = split_segment_key(&part.segment_key);
        tx.execute(
            "INSERT INTO segment_parts(job_id, segment_key, part_index, file_id, bot_index, file_size, prefix, name, encryption_nonce)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                job.job_id,
                part.segment_key,
                part.part_index,
                part.file_id,
                part.bot_index,
                part.file_size,
                prefix,
                name,
                part.encryption_nonce,
            ],
        )?;
    }
    tx.commit()?;
    Ok(())
}

fn split_segment_key(segment_key: &str) -> (&str, &str) {
    segment_key
        .split_once('/')
        .unwrap_or(("legacy", segment_key))
}

pub fn update_job_metadata(
    conn: &mut Connection,
    job_id: &str,
    update: &super::models::JobMetadataUpdate,
) -> Result<bool> {
    let tx = conn.transaction()?;
    let Some(mut job) = get_job(&tx, job_id)? else {
        return Ok(false);
    };
    if let Some(v) = &update.filename {
        job.filename = v.clone();
    }
    if let Some(v) = &update.media_type {
        job.media_type = v.clone();
    }
    if let Some(v) = &update.series_name {
        job.series_name = v.clone();
    }
    if let Some(v) = update.is_series {
        job.is_series = v;
    }
    if let Some(v) = update.season_number {
        job.season_number = v;
    }
    if let Some(v) = update.episode_number {
        job.episode_number = v;
    }
    if let Some(v) = update.part_number {
        job.part_number = v;
    }
    if !job.is_series {
        job.season_number = None;
        job.episode_number = None;
        job.part_number = None;
    }
    let n = tx.execute(
        "UPDATE jobs SET filename=?2, media_type=?3, series_name=?4, is_series=?5,
         season_number=?6, episode_number=?7, part_number=?8 WHERE job_id=?1",
        params![
            job_id,
            job.filename,
            job.media_type,
            job.series_name,
            bool_to_i64(job.is_series),
            job.season_number,
            job.episode_number,
            job.part_number
        ],
    )?;
    tx.commit()?;
    Ok(n > 0)
}

#[allow(clippy::too_many_arguments)] // mirrors the flat metadata columns on the jobs table
pub fn update_job_metadata_fields(
    conn: &mut Connection,
    job_id: &str,
    filename: Option<String>,
    media_type: Option<String>,
    series_name: Option<String>,
    is_series: Option<bool>,
    season_number: Option<Option<i64>>,
    episode_number: Option<Option<i64>>,
    part_number: Option<Option<i64>>,
) -> Result<Option<JobRow>> {
    let tx = conn.transaction()?;
    let Some(mut job) = get_job(&tx, job_id)? else {
        return Ok(None);
    };
    if let Some(v) = filename {
        job.filename = v;
    }
    if let Some(v) = media_type {
        job.media_type = v;
    }
    if let Some(v) = series_name {
        job.series_name = v;
    }
    if let Some(v) = is_series {
        job.is_series = v;
    }
    if let Some(v) = season_number {
        job.season_number = v;
    }
    if let Some(v) = episode_number {
        job.episode_number = v;
    }
    if let Some(v) = part_number {
        job.part_number = v;
    }
    let mut new_job = NewJob {
        job_id: job.job_id.clone(),
        filename: job.filename.clone(),
        duration: job.duration,
        file_size: job.file_size,
        video_codec: job.video_codec.clone(),
        video_width: job.video_width,
        video_height: job.video_height,
        status: job.status.clone(),
        media_type: job.media_type.clone(),
        series_name: job.series_name.clone(),
        has_thumbnail: job.has_thumbnail,
        is_series: job.is_series,
        season_number: job.season_number,
        episode_number: job.episode_number,
        part_number: job.part_number,
        source_path: job.source_path.clone(),
        source_bitrate: job.source_bitrate,
    };
    normalize_job_metadata(&mut new_job);
    tx.execute(
        "UPDATE jobs SET filename=?2, media_type=?3, series_name=?4, is_series=?5,
         season_number=?6, episode_number=?7, part_number=?8 WHERE job_id=?1",
        params![
            job_id,
            new_job.filename,
            new_job.media_type,
            new_job.series_name,
            bool_to_i64(new_job.is_series),
            new_job.season_number,
            new_job.episode_number,
            new_job.part_number
        ],
    )?;
    let result = get_job(&tx, job_id);
    tx.commit()?;
    result
}

pub fn get_job_source_bitrate_by_filename(
    conn: &Connection,
    filename: &str,
) -> Result<Option<i64>> {
    conn.query_row(
        "SELECT source_bitrate FROM jobs WHERE filename = ?1 AND status = 'complete' ORDER BY created_at DESC LIMIT 1",
        params![filename],
        |row| row.get(0),
    )
    .optional()
    .map_err(Into::into)
}

pub fn update_job_thumbnail(conn: &Connection, job_id: &str) -> Result<bool> {
    Ok(conn.execute(
        "UPDATE jobs SET has_thumbnail = 1 WHERE job_id = ?1",
        params![job_id],
    )? > 0)
}

pub fn get_job(conn: &Connection, job_id: &str) -> Result<Option<JobRow>> {
    let sql = JOB_SELECT_SQL.to_owned() + " WHERE job_id = ?1";
    conn.query_row(&sql, params![job_id], job_from_row)
        .optional()
        .map_err(Into::into)
}

pub fn get_job_tracks(
    conn: &Connection,
    job_id: &str,
    track_type: Option<&str>,
) -> Result<Vec<TrackRow>> {
    let sql = match track_type {
        Some(_) => {
            TRACK_SELECT_SQL.to_owned()
                + " WHERE job_id = ?1 AND track_type = ?2 ORDER BY track_index ASC"
        }
        None => {
            TRACK_SELECT_SQL.to_owned()
                + " WHERE job_id = ?1 ORDER BY track_type ASC, track_index ASC"
        }
    };
    let mut stmt = conn.prepare(&sql)?;
    let mapped = match track_type {
        Some(t) => stmt.query_map(params![job_id, t], track_from_row)?,
        None => stmt.query_map(params![job_id], track_from_row)?,
    };
    mapped.map(|r| r.map_err(Into::into)).collect()
}

pub fn get_segment(
    conn: &Connection,
    job_id: &str,
    segment_key: &str,
) -> Result<Option<SegmentLookup>> {
    conn.query_row(
        "SELECT file_id, bot_index, is_split, encryption_nonce FROM segments WHERE job_id = ?1 AND segment_key = ?2",
        params![job_id, segment_key],
        |r| {
            Ok(SegmentLookup {
                file_id: r.get(0)?,
                bot_index: r.get(1)?,
                is_split: r.get::<_, i64>(2)? == 1,
                encryption_nonce: r.get(3)?,
            })
        },
    )
    .optional()
    .map_err(Into::into)
}

pub fn get_segment_parts(
    conn: &Connection,
    job_id: &str,
    segment_key: &str,
) -> Result<Vec<SegmentPartLookup>> {
    let mut stmt = conn.prepare(
        "SELECT file_id, bot_index, file_size, part_index, encryption_nonce FROM segment_parts WHERE job_id = ?1 AND segment_key = ?2 ORDER BY part_index ASC",
    )?;
    let rows = stmt.query_map(params![job_id, segment_key], |r| {
        Ok(SegmentPartLookup {
            file_id: r.get(0)?,
            bot_index: r.get(1)?,
            file_size: r.get(2)?,
            part_index: r.get(3)?,
            encryption_nonce: r.get(4)?,
        })
    })?;
    rows.map(|r| r.map_err(Into::into)).collect()
}

pub fn get_segments_for_prefix(
    conn: &Connection,
    job_id: &str,
    prefix: &str,
) -> Result<Vec<SegmentRow>> {
    let mut stmt = conn.prepare(
        &(SEGMENT_SELECT_SQL.to_owned()
            + " WHERE job_id = ?1 AND prefix = ?2 ORDER BY segment_key ASC"),
    )?;
    let rows = stmt.query_map(params![job_id, prefix], segment_from_row)?;
    rows.map(|r| r.map_err(Into::into)).collect()
}

pub fn get_segments_for_job(conn: &Connection, job_id: &str) -> Result<Vec<SegmentRow>> {
    let mut stmt = conn.prepare(
        &(SEGMENT_SELECT_SQL.to_owned() + " WHERE job_id = ?1 ORDER BY segment_key ASC"),
    )?;
    let rows = stmt.query_map(params![job_id], segment_from_row)?;
    rows.map(|r| r.map_err(Into::into)).collect()
}

pub fn delete_job(conn: &Connection, job_id: &str) -> Result<bool> {
    Ok(conn.execute("DELETE FROM jobs WHERE job_id = ?1", params![job_id])? > 0)
}

pub fn delete_old_jobs(conn: &Connection, older_than_days: i64) -> Result<usize> {
    if older_than_days < 0 {
        anyhow::bail!("older_than_days must be non-negative");
    }
    conn.execute(
        "DELETE FROM jobs WHERE created_at < datetime('now', '-' || ?1 || ' days')",
        params![older_than_days],
    )
    .map_err(Into::into)
}

/// Write a lightweight processing marker for crash recovery.
/// This row gets overwritten by save_job() on completion.
pub fn insert_processing_marker(conn: &Connection, job_id: &str, filename: &str) -> Result<()> {
    insert_job_marker(conn, job_id, filename, "processing")
}

pub fn insert_job_marker(
    conn: &Connection,
    job_id: &str,
    filename: &str,
    status: &str,
) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO jobs(job_id, filename, status, created_at, created_at_unix) VALUES (?1, ?2, ?3, datetime('now'), strftime('%s','now'))",
        rusqlite::params![job_id, filename, status],
    )?;
    Ok(())
}

pub fn job_exists_active_by_filename(conn: &Connection, filename: &str) -> Result<bool> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM jobs WHERE filename = ?1 AND status IN ('queued','downloading','analyzing','processing','uploading')",
        params![filename],
        |r| r.get(0),
    )?;
    Ok(count > 0)
}

pub fn get_stuck_processing_jobs(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT job_id FROM jobs WHERE status IN ('queued','downloading','analyzing','processing','uploading')",
    )?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
    rows.map(|r| r.map_err(Into::into)).collect()
}

pub fn mark_job_as_failed(conn: &Connection, job_id: &str, error: &str) -> Result<bool> {
    Ok(conn.execute(
        "UPDATE jobs SET status = 'error', error = ?2 WHERE job_id = ?1",
        rusqlite::params![job_id, error],
    )? > 0)
}

pub fn mark_job_as_cancelled(conn: &Connection, job_id: &str) -> Result<bool> {
    Ok(conn.execute(
        "UPDATE jobs SET status = 'cancelled', error = NULL WHERE job_id = ?1",
        rusqlite::params![job_id],
    )? > 0)
}

pub fn mark_non_terminal_jobs_failed(conn: &Connection, error: &str) -> Result<usize> {
    Ok(conn.execute(
        "UPDATE jobs SET status = 'error', error = ?1 WHERE status IN ('queued','downloading','analyzing','processing','uploading')",
        rusqlite::params![error],
    )?)
}

pub fn update_segment_file_id(
    conn: &Connection,
    job_id: &str,
    segment_key: &str,
    new_file_id: &str,
    new_bot_index: i64,
    encryption_nonce: Option<&str>,
) -> Result<bool> {
    Ok(conn.execute(
        "UPDATE segments SET file_id = ?1, bot_index = ?2, encryption_nonce = ?3 WHERE job_id = ?4 AND segment_key = ?5",
        params![new_file_id, new_bot_index, encryption_nonce, job_id, segment_key],
    )? > 0)
}

pub fn update_segment_part_file_id(
    conn: &Connection,
    job_id: &str,
    segment_key: &str,
    part_index: i64,
    new_file_id: &str,
    new_bot_index: i64,
    encryption_nonce: Option<&str>,
) -> Result<bool> {
    Ok(conn.execute(
        "UPDATE segment_parts SET file_id = ?1, bot_index = ?2, encryption_nonce = ?3 WHERE job_id = ?4 AND segment_key = ?5 AND part_index = ?6",
        params![new_file_id, new_bot_index, encryption_nonce, job_id, segment_key, part_index],
    )? > 0)
}

pub fn record_db_sync_snapshot(
    conn: &Connection,
    snapshot_id: &str,
    schema_revision: i64,
    size_bytes: u64,
    status: &str,
    error: Option<&str>,
) -> Result<()> {
    conn.execute(
        "INSERT INTO db_sync_snapshots(id, schema_revision, size_bytes, status, last_error)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(id) DO UPDATE SET
            schema_revision=excluded.schema_revision,
            size_bytes=excluded.size_bytes,
            status=excluded.status,
            last_error=excluded.last_error",
        params![
            snapshot_id,
            schema_revision,
            size_bytes as i64,
            status,
            error
        ],
    )?;
    Ok(())
}

pub fn record_db_sync_upload(
    conn: &Connection,
    snapshot_id: &str,
    bot_index: i64,
    part_index: i64,
    file_id: &str,
    file_size: u64,
    encryption_nonce: Option<&str>,
) -> Result<()> {
    conn.execute(
        "INSERT INTO db_sync_uploads(snapshot_id, bot_index, part_index, file_id, file_size, encryption_nonce, status)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'complete')
         ON CONFLICT(snapshot_id, bot_index, part_index) DO UPDATE SET
            file_id=excluded.file_id,
            file_size=excluded.file_size,
            encryption_nonce=excluded.encryption_nonce,
            uploaded_at=CURRENT_TIMESTAMP,
            status='complete',
            error=NULL",
        params![
            snapshot_id,
            bot_index,
            part_index,
            file_id,
            file_size as i64,
            encryption_nonce
        ],
    )?;
    Ok(())
}

// --- External metadata ---
