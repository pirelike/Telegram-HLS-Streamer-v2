use std::collections::HashMap;

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};

use super::models::{
    bool_to_i64, job_filter_sql, job_from_row, normalize_job_metadata, segment_from_row,
    track_from_row, BotWorkload, DbBotRow, JobListFilter, JobRow, NewJob, NewSegment,
    NewSegmentPart, NewTrack, SeasonGroupRow, SegmentLookup, SegmentPartLookup, SegmentRow,
    SeriesGroupRow, TrackRow, JOB_SELECT_SQL, SEGMENT_SELECT_SQL, TRACK_SELECT_SQL,
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
        "INSERT OR REPLACE INTO jobs(
            job_id, filename, duration, file_size, video_codec, video_width, video_height, status,
            media_type, series_name, has_thumbnail, is_series, season_number, episode_number, part_number,
            source_path
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
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
            "INSERT INTO segments(job_id, segment_key, file_id, bot_index, file_size, duration, is_split, prefix, name)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
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
            ],
        )?;
    }
    for part in segment_parts {
        let (prefix, name) = split_segment_key(&part.segment_key);
        tx.execute(
            "INSERT INTO segment_parts(job_id, segment_key, part_index, file_id, bot_index, file_size, prefix, name)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                job.job_id,
                part.segment_key,
                part.part_index,
                part.file_id,
                part.bot_index,
                part.file_size,
                prefix,
                name,
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
        "SELECT file_id, bot_index, is_split FROM segments WHERE job_id = ?1 AND segment_key = ?2",
        params![job_id, segment_key],
        |r| {
            Ok(SegmentLookup {
                file_id: r.get(0)?,
                bot_index: r.get(1)?,
                is_split: r.get::<_, i64>(2)? == 1,
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
        "SELECT file_id, bot_index, file_size FROM segment_parts WHERE job_id = ?1 AND segment_key = ?2 ORDER BY part_index ASC",
    )?;
    let rows = stmt.query_map(params![job_id, segment_key], |r| {
        Ok(SegmentPartLookup {
            file_id: r.get(0)?,
            bot_index: r.get(1)?,
            file_size: r.get(2)?,
        })
    })?;
    rows.map(|r| r.map_err(Into::into)).collect()
}

pub fn get_segments_for_prefix(
    conn: &Connection,
    job_id: &str,
    prefix: &str,
) -> Result<Vec<SegmentRow>> {
    let like = format!("{prefix}/%");
    let mut stmt = conn.prepare(
        &(SEGMENT_SELECT_SQL.to_owned()
            + " WHERE job_id = ?1 AND segment_key LIKE ?2 ORDER BY segment_key ASC"),
    )?;
    let rows = stmt.query_map(params![job_id, like], segment_from_row)?;
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
            WHERE series_name != ''
            GROUP BY series_name
         ),
         reps AS (
            SELECT f.series_name, f.job_id, f.has_thumbnail,
                   ROW_NUMBER() OVER (PARTITION BY f.series_name ORDER BY f.created_at DESC, f.job_id ASC) AS rn
            FROM filtered f
            WHERE f.series_name != ''
         )
         SELECT grouped.series_name, grouped.episode_count, grouped.last_updated, reps.job_id, reps.has_thumbnail
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
        })
    })?;
    rows.map(|r| r.map_err(Into::into)).collect()
}

pub fn count_series_groups(conn: &Connection, filter: &JobListFilter) -> Result<i64> {
    let (where_sql, params) = job_filter_sql(filter);
    let sql = if where_sql.is_empty() {
        "SELECT COUNT(*) FROM (SELECT series_name FROM jobs WHERE series_name != '' GROUP BY series_name)".to_string()
    } else {
        format!(
            "SELECT COUNT(*) FROM (SELECT series_name FROM jobs {where_sql} AND series_name != '' GROUP BY series_name)"
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
            WHERE series_name != ''
            GROUP BY series_name, season_number
         ),
         reps AS (
            SELECT f.series_name, f.season_number, f.job_id, f.has_thumbnail,
                   ROW_NUMBER() OVER (
                    PARTITION BY f.series_name, f.season_number
                    ORDER BY f.created_at DESC, f.job_id ASC
                   ) AS rn
            FROM filtered f
            WHERE f.series_name != ''
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
        "SELECT COUNT(*) FROM (SELECT series_name, season_number FROM jobs WHERE series_name != '' GROUP BY series_name, season_number)".to_string()
    } else {
        format!(
            "SELECT COUNT(*) FROM (SELECT series_name, season_number FROM jobs {where_sql} AND series_name != '' GROUP BY series_name, season_number)"
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
                 WHERE media_type = ?1 AND series_name != ''
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
                 WHERE series_name != ''
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
) -> Result<bool> {
    Ok(conn.execute(
        "UPDATE segments SET file_id = ?1, bot_index = ?2 WHERE job_id = ?3 AND segment_key = ?4",
        params![new_file_id, new_bot_index, job_id, segment_key],
    )? > 0)
}

pub fn update_segment_part_file_id(
    conn: &Connection,
    job_id: &str,
    segment_key: &str,
    part_index: i64,
    new_file_id: &str,
    new_bot_index: i64,
) -> Result<bool> {
    Ok(conn.execute(
        "UPDATE segment_parts SET file_id = ?1, bot_index = ?2 WHERE job_id = ?3 AND segment_key = ?4 AND part_index = ?5",
        params![new_file_id, new_bot_index, job_id, segment_key, part_index],
    )? > 0)
}
