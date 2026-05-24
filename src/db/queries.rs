use std::collections::HashMap;

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};

use super::models::{
    bool_to_i64, external_metadata_from_row, job_filter_sql, job_from_row,
    job_metadata_link_from_row, media_fingerprint_from_row, media_marker_from_row,
    normalize_job_metadata, playback_progress_from_row, segment_from_row,
    series_metadata_link_from_row, track_from_row, BotWorkload, DbBotRow, ExternalMetadataRow,
    JobListFilter, JobMetadataLinkRow, JobRow, MediaFingerprintRow, MediaMarkerRow,
    NewExternalMetadata, NewJob, NewMediaFingerprint, NewMediaMarker, NewPlaybackProgress,
    NewSegment, NewSegmentPart, NewTrack, PlaybackProgressRow, SeasonGroupRow, SegmentLookup,
    SegmentPartLookup, SegmentRow, SeriesGroupRow, SeriesMetadataLinkRow, TrackRow,
    EXTERNAL_METADATA_SELECT_SQL, JOB_SELECT_SQL, SEGMENT_SELECT_SQL, TRACK_SELECT_SQL,
};

fn setting_value_type(key: &str) -> Result<&'static str> {
    let spec = crate::settings_registry::setting_spec(key)
        .ok_or_else(|| anyhow::anyhow!("unknown setting key: {key}"))?;
    Ok(crate::settings_registry::setting_type_name(
        spec.setting_type,
    ))
}

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
            source_path
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
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
            source_path = excluded.source_path",
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
    conn.execute(
        "DELETE FROM jobs WHERE created_at < datetime('now', '-' || ?1 || ' days')",
        params![older_than_days],
    )
    .map_err(Into::into)
}

pub fn list_jobs(conn: &Connection, filter: &JobListFilter) -> Result<Vec<JobRow>> {
    let (where_sql, params) = job_filter_sql(filter);
    let sql = format!(
        "{} {} ORDER BY created_at DESC LIMIT ?{} OFFSET ?{}",
        JOB_SELECT_SQL,
        where_sql,
        params.len() + 1,
        params.len() + 2
    );
    let mut values = params;
    values.push(filter.limit.to_string());
    values.push(filter.offset.to_string());
    let refs: Vec<&dyn rusqlite::ToSql> =
        values.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(refs.as_slice(), job_from_row)?;
    rows.map(|r| r.map_err(Into::into)).collect()
}

pub fn count_jobs(conn: &Connection, filter: &JobListFilter) -> Result<i64> {
    let (where_sql, params) = job_filter_sql(filter);
    let sql = format!("SELECT COUNT(*) FROM jobs {where_sql}");
    let refs: Vec<&dyn rusqlite::ToSql> =
        params.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
    conn.query_row(&sql, refs.as_slice(), |r| r.get(0))
        .map_err(Into::into)
}

pub fn list_series_groups(
    conn: &Connection,
    filter: &JobListFilter,
) -> Result<Vec<SeriesGroupRow>> {
    let (where_sql, params) = job_filter_sql(filter);
    let sql = format!(
        "WITH filtered AS (
            SELECT * FROM jobs {where_sql}
         ),
         grouped AS (
            SELECT series_name, COUNT(*) AS episode_count, MAX(created_at) AS last_updated
            FROM filtered
            WHERE COALESCE(series_name, '') != ''
            GROUP BY series_name
         ),
         reps AS (
            SELECT f.series_name, f.job_id, f.has_thumbnail, f.media_type,
                   ROW_NUMBER() OVER (PARTITION BY f.series_name ORDER BY f.created_at DESC, f.job_id ASC) AS rn
            FROM filtered f
            WHERE COALESCE(f.series_name, '') != ''
          )
          SELECT grouped.series_name, grouped.episode_count, grouped.last_updated, reps.job_id, reps.has_thumbnail, reps.media_type
         FROM grouped
         JOIN reps ON reps.series_name = grouped.series_name AND reps.rn = 1
         ORDER BY grouped.last_updated DESC
         LIMIT ?{} OFFSET ?{}",
        params.len() + 1,
        params.len() + 2
    );
    let mut values = params;
    values.push(filter.limit.to_string());
    values.push(filter.offset.to_string());
    let refs: Vec<&dyn rusqlite::ToSql> =
        values.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(refs.as_slice(), |r| {
        Ok(SeriesGroupRow {
            series_name: r.get(0)?,
            episode_count: r.get(1)?,
            last_updated: r.get(2)?,
            job_id: r.get(3)?,
            has_thumbnail: r.get::<_, i64>(4)? == 1,
            media_type: r.get(5)?,
        })
    })?;
    rows.map(|r| r.map_err(Into::into)).collect()
}

pub fn count_series_groups(conn: &Connection, filter: &JobListFilter) -> Result<i64> {
    let (where_sql, params) = job_filter_sql(filter);
    let sql = if where_sql.is_empty() {
        "SELECT COUNT(*) FROM (SELECT series_name FROM jobs WHERE COALESCE(series_name, '') != '' GROUP BY series_name)".to_string()
    } else {
        format!(
            "SELECT COUNT(*) FROM (SELECT series_name FROM jobs {where_sql} AND COALESCE(series_name, '') != '' GROUP BY series_name)"
        )
    };
    let refs: Vec<&dyn rusqlite::ToSql> =
        params.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
    conn.query_row(&sql, refs.as_slice(), |r| r.get(0))
        .map_err(Into::into)
}

pub fn list_season_groups(
    conn: &Connection,
    filter: &JobListFilter,
) -> Result<Vec<SeasonGroupRow>> {
    let (where_sql, params) = job_filter_sql(filter);
    let sql = format!(
        "WITH filtered AS (
            SELECT * FROM jobs {where_sql}
         ),
         grouped AS (
            SELECT series_name, season_number, COUNT(*) AS episode_count, MAX(created_at) AS last_updated
            FROM filtered
            WHERE COALESCE(series_name, '') != ''
            GROUP BY series_name, season_number
         ),
         reps AS (
            SELECT f.series_name, f.season_number, f.job_id, f.has_thumbnail,
                   ROW_NUMBER() OVER (
                    PARTITION BY f.series_name, f.season_number
                    ORDER BY f.created_at DESC, f.job_id ASC
                   ) AS rn
            FROM filtered f
            WHERE COALESCE(f.series_name, '') != ''
          )
          SELECT grouped.series_name, grouped.season_number, grouped.episode_count,
                grouped.last_updated, reps.job_id, reps.has_thumbnail
         FROM grouped
         JOIN reps ON reps.series_name = grouped.series_name
             AND ((reps.season_number IS NULL AND grouped.season_number IS NULL)
                  OR reps.season_number = grouped.season_number)
             AND reps.rn = 1
         ORDER BY grouped.last_updated DESC, grouped.season_number ASC
         LIMIT ?{} OFFSET ?{}",
        params.len() + 1,
        params.len() + 2
    );
    let mut values = params;
    values.push(filter.limit.to_string());
    values.push(filter.offset.to_string());
    let refs: Vec<&dyn rusqlite::ToSql> =
        values.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(refs.as_slice(), |r| {
        Ok(SeasonGroupRow {
            series_name: r.get(0)?,
            season_number: r.get(1)?,
            episode_count: r.get(2)?,
            last_updated: r.get(3)?,
            job_id: r.get(4)?,
            has_thumbnail: r.get::<_, i64>(5)? == 1,
        })
    })?;
    rows.map(|r| r.map_err(Into::into)).collect()
}

pub fn count_season_groups(conn: &Connection, filter: &JobListFilter) -> Result<i64> {
    let (where_sql, params) = job_filter_sql(filter);
    let sql = if where_sql.is_empty() {
        "SELECT COUNT(*) FROM (SELECT series_name, season_number FROM jobs WHERE COALESCE(series_name, '') != '' GROUP BY series_name, season_number)".to_string()
    } else {
        format!(
            "SELECT COUNT(*) FROM (SELECT series_name, season_number FROM jobs {where_sql} AND COALESCE(series_name, '') != '' GROUP BY series_name, season_number)"
        )
    };
    let refs: Vec<&dyn rusqlite::ToSql> =
        params.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
    conn.query_row(&sql, refs.as_slice(), |r| r.get(0))
        .map_err(Into::into)
}

pub fn count_job_segments(conn: &Connection, job_id: &str) -> Result<i64> {
    conn.query_row(
        "SELECT COUNT(*) FROM segments WHERE job_id = ?1",
        params![job_id],
        |r| r.get(0),
    )
    .map_err(Into::into)
}

pub fn distinct_series_names(conn: &Connection, category: Option<&str>) -> Result<Vec<String>> {
    let mut out = Vec::new();
    match category {
        Some(category) => {
            let mut stmt = conn.prepare(
                "SELECT DISTINCT series_name FROM jobs
                 WHERE media_type = ?1 AND COALESCE(series_name, '') != ''
                 ORDER BY series_name ASC",
            )?;
            let rows = stmt.query_map(params![category], |r| r.get::<_, String>(0))?;
            for row in rows {
                out.push(row?);
            }
        }
        None => {
            let mut stmt = conn.prepare(
                "SELECT DISTINCT series_name FROM jobs
                 WHERE COALESCE(series_name, '') != ''
                 ORDER BY series_name ASC",
            )?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            for row in rows {
                out.push(row?);
            }
        }
    }
    Ok(out)
}

pub fn get_all_settings(conn: &Connection) -> Result<HashMap<String, String>> {
    let mut stmt = conn.prepare("SELECT key, value FROM settings ORDER BY key ASC")?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    let mut out = HashMap::new();
    for row in rows {
        let (k, v) = row?;
        if !k.starts_with('_') {
            out.insert(k, v);
        }
    }
    Ok(out)
}

pub fn set_setting(conn: &Connection, key: &str, value: &str) -> Result<()> {
    super::validate_setting_key(key)?;
    let value_type = setting_value_type(key)?;
    conn.execute(
        "INSERT INTO settings(key, value, value_type, updated_at) VALUES (?1, ?2, ?3, CURRENT_TIMESTAMP)
         ON CONFLICT(key) DO UPDATE SET value=excluded.value, value_type=excluded.value_type, updated_at=CURRENT_TIMESTAMP",
        params![key, value, value_type],
    )?;
    Ok(())
}

pub fn set_settings(conn: &mut Connection, settings: &HashMap<String, String>) -> Result<()> {
    let tx = conn.transaction()?;
    for (key, value) in settings {
        super::validate_setting_key(key)?;
        let value_type = setting_value_type(key)?;
        tx.execute(
            "INSERT INTO settings(key, value, value_type, updated_at) VALUES (?1, ?2, ?3, CURRENT_TIMESTAMP)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value, value_type=excluded.value_type, updated_at=CURRENT_TIMESTAMP",
            params![key, value, value_type],
        )?;
    }
    tx.commit()?;
    Ok(())
}

pub fn delete_setting(conn: &Connection, key: &str) -> Result<bool> {
    super::validate_setting_key(key)?;
    Ok(conn.execute("DELETE FROM settings WHERE key = ?1", params![key])? > 0)
}

pub fn get_last_bot_index(conn: &Connection) -> Result<i64> {
    let value: Option<String> = conn
        .query_row(
            "SELECT value FROM kv_internal WHERE key = '_last_bot_index'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    Ok(value
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(-1)
        .max(-1))
}

pub fn set_last_bot_index(conn: &Connection, index: i64) -> Result<()> {
    let index = index.max(0).to_string();
    conn.execute(
        "INSERT INTO kv_internal(key, value, updated_at) VALUES ('_last_bot_index', ?1, CURRENT_TIMESTAMP)
         ON CONFLICT(key) DO UPDATE SET value=excluded.value, updated_at=CURRENT_TIMESTAMP",
        params![index],
    )?;
    Ok(())
}

pub fn get_internal_value(conn: &Connection, key: &str) -> Result<Option<String>> {
    conn.query_row(
        "SELECT value FROM kv_internal WHERE key = ?1",
        params![key],
        |r| r.get(0),
    )
    .optional()
    .map_err(Into::into)
}

pub fn set_internal_value(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO kv_internal(key, value, updated_at) VALUES (?1, ?2, CURRENT_TIMESTAMP)
         ON CONFLICT(key) DO UPDATE SET value=excluded.value, updated_at=CURRENT_TIMESTAMP",
        params![key, value],
    )?;
    Ok(())
}

pub fn get_all_bots(conn: &Connection) -> Result<Vec<DbBotRow>> {
    let has_enabled =
        crate::db::migrations::column_exists(conn, "bots", "enabled").unwrap_or(false);
    let has_source = crate::db::migrations::column_exists(conn, "bots", "source").unwrap_or(false);
    let sql = if has_enabled && has_source {
        "SELECT id, token, channel_id, label, enabled, source FROM bots ORDER BY id ASC"
    } else {
        "SELECT id, token, channel_id, label, 1, 'db' FROM bots ORDER BY id ASC"
    };
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map([], |r| {
        Ok(DbBotRow {
            id: r.get(0)?,
            token: r.get(1)?,
            channel_id: r.get(2)?,
            label: r.get(3)?,
            enabled: r.get::<_, i64>(4)? != 0,
            source: r.get(5)?,
        })
    })?;
    rows.map(|r| r.map_err(Into::into)).collect()
}

pub fn add_bot(conn: &Connection, token: &str, channel_id: i64, label: &str) -> Result<i64> {
    conn.execute(
        "INSERT INTO bots(token, channel_id, label, enabled, source) VALUES (?1, ?2, ?3, 1, 'db')",
        params![token, channel_id, label],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn delete_bot(conn: &Connection, id: i64) -> Result<bool> {
    Ok(conn.execute("DELETE FROM bots WHERE id = ?1", params![id])? > 0)
}

pub fn bot_exists(conn: &Connection, token: &str) -> Result<bool> {
    let exists: i64 = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM bots WHERE token = ?1)",
        params![token],
        |r| r.get(0),
    )?;
    Ok(exists == 1)
}

pub fn get_bot_workload_stats(conn: &Connection) -> Result<HashMap<i64, BotWorkload>> {
    let mut stmt = conn.prepare(
        "SELECT bot_index, COUNT(*), COALESCE(SUM(file_size), 0) FROM segments GROUP BY bot_index",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            BotWorkload {
                segment_count: r.get(1)?,
                total_bytes: r.get(2)?,
            },
        ))
    })?;
    let mut out = HashMap::new();
    for row in rows {
        let (bot_index, stats) = row?;
        out.insert(bot_index, stats);
    }
    Ok(out)
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
        "INSERT OR IGNORE INTO jobs(job_id, filename, status, created_at) VALUES (?1, ?2, ?3, datetime('now'))",
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

pub fn save_external_metadata(conn: &Connection, meta: &NewExternalMetadata) -> Result<i64> {
    conn.query_row(
        "INSERT INTO external_metadata(provider, provider_id, media_kind, title, original_title, overview, poster_url, backdrop_url, release_date, year, rating, raw_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
         ON CONFLICT(provider, provider_id, media_kind) DO UPDATE SET
            title=excluded.title,
            original_title=excluded.original_title,
            overview=excluded.overview,
            poster_url=excluded.poster_url,
            backdrop_url=excluded.backdrop_url,
            release_date=excluded.release_date,
            year=excluded.year,
            rating=excluded.rating,
            raw_json=excluded.raw_json,
            fetched_at=CURRENT_TIMESTAMP
         RETURNING id",
        params![
            meta.provider,
            meta.provider_id,
            meta.media_kind,
            meta.title,
            meta.original_title,
            meta.overview,
            meta.poster_url,
            meta.backdrop_url,
            meta.release_date,
            meta.year,
            meta.rating,
            meta.raw_json,
        ],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

pub fn get_external_metadata(
    conn: &Connection,
    provider: &str,
    provider_id: &str,
    media_kind: &str,
) -> Result<Option<ExternalMetadataRow>> {
    let sql = format!(
        "{} WHERE provider = ?1 AND provider_id = ?2 AND media_kind = ?3",
        EXTERNAL_METADATA_SELECT_SQL
    );
    conn.query_row(
        &sql,
        params![provider, provider_id, media_kind],
        external_metadata_from_row,
    )
    .optional()
    .map_err(Into::into)
}

pub fn get_external_metadata_by_id(
    conn: &Connection,
    id: i64,
) -> Result<Option<ExternalMetadataRow>> {
    let sql = format!("{} WHERE id = ?1", EXTERNAL_METADATA_SELECT_SQL);
    conn.query_row(&sql, params![id], external_metadata_from_row)
        .optional()
        .map_err(Into::into)
}

pub fn list_external_metadata(conn: &Connection) -> Result<Vec<ExternalMetadataRow>> {
    let mut stmt = conn.prepare(&format!(
        "{} ORDER BY fetched_at DESC",
        EXTERNAL_METADATA_SELECT_SQL
    ))?;
    let rows = stmt.query_map([], external_metadata_from_row)?;
    rows.map(|r| r.map_err(Into::into)).collect()
}

pub fn link_job_metadata(
    conn: &Connection,
    job_id: &str,
    metadata_id: i64,
    role: &str,
) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO job_metadata_links(job_id, metadata_id, role)
         VALUES (?1, ?2, ?3)",
        params![job_id, metadata_id, role],
    )?;
    Ok(())
}

pub fn get_job_metadata_links(
    conn: &Connection,
    job_id: &str,
) -> Result<Vec<(JobMetadataLinkRow, ExternalMetadataRow)>> {
    let mut stmt = conn.prepare(
        "SELECT jml.job_id, jml.metadata_id, jml.role, jml.created_at,
                em.id, em.provider, em.provider_id, em.media_kind, em.title, em.original_title,
                em.overview, em.poster_url, em.backdrop_url, em.release_date, em.year, em.rating,
                em.raw_json, em.fetched_at
         FROM job_metadata_links jml
         JOIN external_metadata em ON em.id = jml.metadata_id
         WHERE jml.job_id = ?1",
    )?;
    let rows = stmt.query_map(params![job_id], |r| {
        Ok((
            job_metadata_link_from_row(r)?,
            external_metadata_from_row_aligned(r, 4)?,
        ))
    })?;
    rows.map(|r| r.map_err(Into::into)).collect()
}

fn external_metadata_from_row_aligned(
    row: &rusqlite::Row<'_>,
    offset: usize,
) -> rusqlite::Result<ExternalMetadataRow> {
    Ok(ExternalMetadataRow {
        id: row.get(offset)?,
        provider: row.get(offset + 1)?,
        provider_id: row.get(offset + 2)?,
        media_kind: row.get(offset + 3)?,
        title: row.get(offset + 4)?,
        original_title: row.get(offset + 5)?,
        overview: row.get(offset + 6)?,
        poster_url: row.get(offset + 7)?,
        backdrop_url: row.get(offset + 8)?,
        release_date: row.get(offset + 9)?,
        year: row.get(offset + 10)?,
        rating: row.get(offset + 11)?,
        raw_json: row.get(offset + 12)?,
        fetched_at: row.get(offset + 13)?,
    })
}

pub fn link_series_metadata(
    conn: &Connection,
    media_type: &str,
    series_name: &str,
    metadata_id: i64,
) -> Result<()> {
    conn.execute(
        "INSERT INTO series_metadata_links(media_type, series_name, metadata_id)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(media_type, series_name) DO UPDATE SET metadata_id = excluded.metadata_id",
        params![media_type, series_name, metadata_id],
    )?;
    Ok(())
}

pub fn get_series_metadata_link(
    conn: &Connection,
    media_type: &str,
    series_name: &str,
) -> Result<Option<(SeriesMetadataLinkRow, ExternalMetadataRow)>> {
    conn.query_row(
        "SELECT sml.media_type, sml.series_name, sml.metadata_id, sml.created_at,
                em.id, em.provider, em.provider_id, em.media_kind, em.title, em.original_title,
                em.overview, em.poster_url, em.backdrop_url, em.release_date, em.year, em.rating,
                em.raw_json, em.fetched_at
         FROM series_metadata_links sml
         JOIN external_metadata em ON em.id = sml.metadata_id
         WHERE sml.media_type = ?1 AND sml.series_name = ?2",
        params![media_type, series_name],
        |r| {
            Ok((
                series_metadata_link_from_row(r)?,
                external_metadata_from_row_aligned(r, 4)?,
            ))
        },
    )
    .optional()
    .map_err(Into::into)
}

pub fn get_job_poster_urls(
    conn: &Connection,
    job_ids: &[String],
) -> Result<HashMap<String, String>> {
    if job_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let placeholders = job_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let sql = format!(
        "SELECT jml.job_id, em.poster_url \
         FROM job_metadata_links jml \
         JOIN external_metadata em ON em.id = jml.metadata_id \
         WHERE jml.job_id IN ({}) AND jml.role = 'primary'",
        placeholders
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(job_ids.iter()), |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })?;
    let mut map = HashMap::new();
    for row in rows {
        let (job_id, poster_url) = row?;
        if !poster_url.is_empty() {
            map.insert(job_id, poster_url);
        }
    }
    Ok(map)
}

pub fn get_series_poster_urls(
    conn: &Connection,
    series_names: &[String],
) -> Result<HashMap<String, String>> {
    if series_names.is_empty() {
        return Ok(HashMap::new());
    }
    let placeholders = series_names
        .iter()
        .map(|_| "?")
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT sml.series_name, em.poster_url \
         FROM series_metadata_links sml \
         JOIN external_metadata em ON em.id = sml.metadata_id \
         WHERE sml.series_name IN ({})",
        placeholders
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(series_names.iter()), |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })?;
    let mut map = HashMap::new();
    for row in rows {
        let (series_name, poster_url) = row?;
        if !poster_url.is_empty() {
            map.insert(series_name, poster_url);
        }
    }
    Ok(map)
}

pub fn get_season_episode_job_ids(
    conn: &Connection,
    series_name: &str,
) -> Result<Vec<(String, i64, i64)>> {
    let mut stmt = conn.prepare(
        "SELECT job_id, season_number, episode_number FROM jobs
         WHERE series_name = ?1 AND season_number IS NOT NULL AND episode_number IS NOT NULL",
    )?;
    let rows = stmt.query_map(params![series_name], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, i64>(2)?,
        ))
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

pub fn rename_series(
    conn: &Connection,
    old_name: &str,
    new_name: &str,
    media_type: &str,
) -> Result<()> {
    conn.execute(
        "UPDATE jobs SET series_name = ?1 WHERE series_name = ?2 AND media_type = ?3",
        params![new_name, old_name, media_type],
    )?;
    // If the new name already exists in series_metadata_links, delete the old entry
    // rather than trying to rename it (the new link was already written by link_series_metadata).
    conn.execute(
        "DELETE FROM series_metadata_links WHERE series_name = ?1 AND media_type = ?2",
        params![old_name, media_type],
    )?;
    conn.execute(
        "UPDATE OR IGNORE media_fingerprints SET series_name = ?1 WHERE series_name = ?2",
        params![new_name, old_name],
    )?;
    Ok(())
}

pub fn set_episode_titles(conn: &Connection, updates: &[(String, String)]) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    for (job_id, title) in updates {
        tx.execute(
            "UPDATE jobs SET episode_title = ?1 WHERE job_id = ?2",
            params![title, job_id],
        )?;
    }
    tx.commit()?;
    Ok(())
}

// --- Playback progress ---

pub fn save_playback_progress(conn: &Connection, progress: &NewPlaybackProgress) -> Result<()> {
    let pct = if progress.duration_seconds > 0.0 {
        ((progress.position_seconds / progress.duration_seconds) * 100.0).round() as i64
    } else {
        0
    };
    let pct = pct.clamp(0, 100);
    let completed = pct >= 95;
    conn.execute(
        "INSERT INTO playback_progress(client_id, job_id, position_seconds, duration_seconds, progress_pct, completed, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, CURRENT_TIMESTAMP)
         ON CONFLICT(client_id, job_id) DO UPDATE SET
            position_seconds=excluded.position_seconds,
            duration_seconds=excluded.duration_seconds,
            progress_pct=excluded.progress_pct,
            completed=excluded.completed,
            updated_at=CURRENT_TIMESTAMP",
        params![
            progress.client_id,
            progress.job_id,
            progress.position_seconds,
            progress.duration_seconds,
            pct,
            bool_to_i64(completed) as i64,
        ],
    )?;
    Ok(())
}

pub fn get_playback_progress(
    conn: &Connection,
    client_id: &str,
    job_id: &str,
) -> Result<Option<PlaybackProgressRow>> {
    conn.query_row(
        "SELECT client_id, job_id, position_seconds, duration_seconds, progress_pct, completed, updated_at
         FROM playback_progress WHERE client_id = ?1 AND job_id = ?2",
        params![client_id, job_id],
        playback_progress_from_row,
    )
    .optional()
    .map_err(Into::into)
}

pub fn list_playback_progress(
    conn: &Connection,
    client_id: &str,
) -> Result<Vec<PlaybackProgressRow>> {
    let mut stmt = conn.prepare(
        "SELECT client_id, job_id, position_seconds, duration_seconds, progress_pct, completed, updated_at
         FROM playback_progress WHERE client_id = ?1 AND completed = 0
         ORDER BY updated_at DESC",
    )?;
    let rows = stmt.query_map(params![client_id], playback_progress_from_row)?;
    rows.map(|r| r.map_err(Into::into)).collect()
}

pub fn delete_playback_progress(conn: &Connection, client_id: &str, job_id: &str) -> Result<bool> {
    Ok(conn.execute(
        "DELETE FROM playback_progress WHERE client_id = ?1 AND job_id = ?2",
        params![client_id, job_id],
    )? > 0)
}

// --- Media markers ---

pub fn save_media_markers(
    conn: &Connection,
    job_id: &str,
    markers: &[NewMediaMarker],
) -> Result<()> {
    for marker in markers {
        conn.execute(
            "INSERT INTO media_markers(job_id, marker_type, start_seconds, end_seconds, source, confidence)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                job_id,
                marker.marker_type,
                marker.start_seconds,
                marker.end_seconds,
                marker.source,
                marker.confidence,
            ],
        )?;
    }
    Ok(())
}

pub fn replace_auto_media_markers(
    conn: &Connection,
    job_id: &str,
    markers: &[NewMediaMarker],
) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "DELETE FROM media_markers WHERE job_id = ?1 AND source != 'manual'",
        params![job_id],
    )?;
    for marker in markers {
        tx.execute(
            "INSERT INTO media_markers(job_id, marker_type, start_seconds, end_seconds, source, confidence)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                job_id,
                marker.marker_type,
                marker.start_seconds,
                marker.end_seconds,
                marker.source,
                marker.confidence,
            ],
        )?;
    }
    tx.commit()?;
    Ok(())
}

pub fn get_media_markers(
    conn: &Connection,
    job_id: &str,
    enabled_only: bool,
) -> Result<Vec<MediaMarkerRow>> {
    let sql = if enabled_only {
        "SELECT id, job_id, marker_type, start_seconds, end_seconds, source, confidence, enabled, created_at, updated_at
         FROM media_markers WHERE job_id = ?1 AND enabled = 1 ORDER BY start_seconds ASC"
    } else {
        "SELECT id, job_id, marker_type, start_seconds, end_seconds, source, confidence, enabled, created_at, updated_at
         FROM media_markers WHERE job_id = ?1 ORDER BY start_seconds ASC"
    };
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map(params![job_id], media_marker_from_row)?;
    rows.map(|r| r.map_err(Into::into)).collect()
}

pub fn delete_media_markers(conn: &Connection, job_id: &str) -> Result<usize> {
    Ok(conn.execute(
        "DELETE FROM media_markers WHERE job_id = ?1",
        params![job_id],
    )?)
}

// --- Media fingerprints ---

pub fn save_media_fingerprint(conn: &Connection, fp: &NewMediaFingerprint) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO media_fingerprints(
            job_id, media_type, series_name, season_number, window_type,
            window_start_seconds, window_duration_seconds, duration_seconds,
            fingerprint, fingerprint_source, created_at
         )
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, CURRENT_TIMESTAMP)",
        params![
            fp.job_id,
            fp.media_type,
            fp.series_name,
            fp.season_number,
            fp.window_type,
            fp.window_start_seconds,
            fp.window_duration_seconds,
            fp.duration_seconds,
            fp.fingerprint,
            fp.fingerprint_source,
        ],
    )?;
    Ok(())
}

pub fn get_media_fingerprint(
    conn: &Connection,
    job_id: &str,
) -> Result<Option<MediaFingerprintRow>> {
    conn.query_row(
        "SELECT job_id, media_type, series_name, season_number, window_type, window_start_seconds, window_duration_seconds, duration_seconds, fingerprint, fingerprint_source, created_at
         FROM media_fingerprints WHERE job_id = ?1 AND window_type = 'intro'",
        params![job_id],
        media_fingerprint_from_row,
    )
    .optional()
    .map_err(Into::into)
}

pub fn get_media_fingerprints_for_series(
    conn: &Connection,
    media_type: &str,
    series_name: &str,
    season_number: Option<i64>,
) -> Result<Vec<MediaFingerprintRow>> {
    get_media_fingerprints_for_series_window(conn, media_type, series_name, season_number, "intro")
}

pub fn get_media_fingerprints_for_series_window(
    conn: &Connection,
    media_type: &str,
    series_name: &str,
    season_number: Option<i64>,
    window_type: &str,
) -> Result<Vec<MediaFingerprintRow>> {
    let mut stmt = match season_number {
        Some(_) => conn.prepare(
            "SELECT job_id, media_type, series_name, season_number, window_type, window_start_seconds, window_duration_seconds, duration_seconds, fingerprint, fingerprint_source, created_at
             FROM media_fingerprints WHERE media_type = ?1 AND series_name = ?2 AND season_number = ?3 AND window_type = ?4 ORDER BY created_at ASC",
        )?,
        None => conn.prepare(
            "SELECT job_id, media_type, series_name, season_number, window_type, window_start_seconds, window_duration_seconds, duration_seconds, fingerprint, fingerprint_source, created_at
             FROM media_fingerprints WHERE media_type = ?1 AND series_name = ?2 AND season_number IS NULL AND window_type = ?3 ORDER BY created_at ASC",
        )?,
    };
    let rows = match season_number {
        Some(sn) => stmt.query_map(
            params![media_type, series_name, sn, window_type],
            media_fingerprint_from_row,
        )?,
        None => stmt.query_map(
            params![media_type, series_name, window_type],
            media_fingerprint_from_row,
        )?,
    };
    rows.map(|r| r.map_err(Into::into)).collect()
}
