use std::collections::HashMap;
use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};
use rusqlite::{params, Connection};

use super::models::{
    bool_to_i64, job_from_row, normalize_job_metadata, segment_from_row, track_from_row,
    DatabaseBackupResult, DbExport, MergeResult, NewJob, ReplaceDatabaseResult, JOB_SELECT_SQL,
    SEGMENT_SELECT_SQL, TRACK_SELECT_SQL,
};
use super::{current_schema_revision, init_db, validate_sqlite_header};

pub fn export_to_dict(conn: &Connection) -> Result<DbExport> {
    let mut jobs_stmt =
        conn.prepare(&(JOB_SELECT_SQL.to_owned() + " ORDER BY created_at ASC, job_id ASC"))?;
    let jobs = jobs_stmt
        .query_map([], job_from_row)?
        .map(|r| r.map_err(Into::into))
        .collect::<Result<Vec<_>>>()?;
    let mut tracks_stmt = conn.prepare(
        &(TRACK_SELECT_SQL.to_owned() + " ORDER BY job_id ASC, track_type ASC, track_index ASC"),
    )?;
    let tracks = tracks_stmt
        .query_map([], track_from_row)?
        .map(|r| r.map_err(Into::into))
        .collect::<Result<Vec<_>>>()?;
    let mut segments_stmt =
        conn.prepare(&(SEGMENT_SELECT_SQL.to_owned() + " ORDER BY job_id ASC, segment_key ASC"))?;
    let segments = segments_stmt
        .query_map([], segment_from_row)?
        .map(|r| r.map_err(Into::into))
        .collect::<Result<Vec<_>>>()?;
    Ok(DbExport {
        version: 1,
        schema_revision: current_schema_revision(conn)?,
        jobs,
        tracks,
        segments,
    })
}

pub fn merge_from_export(
    conn: &mut Connection,
    export: &DbExport,
    bot_index_map: &HashMap<i64, i64>,
) -> Result<MergeResult> {
    let tx = conn.transaction()?;
    let mut merged_jobs = 0;
    let mut merged_segments = 0;
    for job in &export.jobs {
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
        merged_jobs += tx.execute(
            "INSERT OR IGNORE INTO jobs(
                job_id, filename, duration, file_size, video_codec, video_width, video_height, status,
                error, created_at, media_type, series_name, has_thumbnail, is_series, season_number, episode_number, part_number,
                source_path
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
            params![
                new_job.job_id,
                new_job.filename,
                new_job.duration,
                new_job.file_size,
                new_job.video_codec,
                new_job.video_width,
                new_job.video_height,
                new_job.status,
                job.error,
                job.created_at,
                new_job.media_type,
                new_job.series_name,
                bool_to_i64(new_job.has_thumbnail),
                bool_to_i64(new_job.is_series),
                new_job.season_number,
                new_job.episode_number,
                new_job.part_number,
                new_job.source_path,
            ],
        )?;
    }
    for track in &export.tracks {
        let (mode, bitrate_bps) = super::migrations::track_mode_and_bps(&track.bitrate);
        tx.execute(
            "INSERT OR IGNORE INTO tracks(
                job_id, track_type, track_index, codec, language, title, channels,
                width, height, bitrate, original_stream_index, mode, bitrate_bps
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                track.job_id,
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
                bitrate_bps
            ],
        )?;
    }
    for segment in &export.segments {
        let Some(bot_index) = bot_index_map.get(&segment.bot_index) else {
            bail!("missing bot_index_map entry for {}", segment.bot_index);
        };
        let (prefix, name) = split_segment_key(&segment.segment_key);
        merged_segments += tx.execute(
            "INSERT OR IGNORE INTO segments(job_id, segment_key, file_id, bot_index, file_size, duration, is_split, prefix, name)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![segment.job_id, segment.segment_key, segment.file_id, *bot_index, segment.file_size, segment.duration, bool_to_i64(segment.is_split), prefix, name],
        )?;
    }
    tx.commit()?;
    Ok(MergeResult {
        merged_jobs,
        merged_segments,
    })
}

fn split_segment_key(segment_key: &str) -> (&str, &str) {
    segment_key
        .split_once('/')
        .unwrap_or(("legacy", segment_key))
}

pub fn replace_database_file(
    active_path: &Path,
    source_path: &Path,
) -> Result<ReplaceDatabaseResult> {
    validate_sqlite_header(source_path)?;
    {
        let conn = init_db(source_path).context("migrating replacement database")?;
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
            .context("checkpointing replacement database")?;
        conn.close().map_err(|(_, e)| anyhow!(e))?;
    }
    remove_sqlite_sidecars(source_path)?;
    let backup_path = super::backup_path(active_path);
    if active_path.exists() {
        fs::rename(active_path, &backup_path).context("backing up active database")?;
    } else {
        fs::File::create(&backup_path).context("creating empty backup marker")?;
    }
    remove_sqlite_sidecars(active_path)?;
    fs::rename(source_path, active_path).context("installing replacement database")?;
    let conn = init_db(active_path).context("opening replacement database")?;
    let schema_revision = current_schema_revision(&conn)?;
    conn.close().map_err(|(_, e)| anyhow!(e))?;
    Ok(ReplaceDatabaseResult {
        backup_path,
        schema_revision,
    })
}

fn remove_sqlite_sidecars(path: &Path) -> Result<()> {
    for suffix in ["-wal", "-shm"] {
        let sidecar = format!("{}{}", path.display(), suffix);
        match fs::remove_file(&sidecar) {
            Ok(()) => {}
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => return Err(e).with_context(|| format!("removing sqlite sidecar {sidecar}")),
        }
    }
    Ok(())
}

pub fn backup_database_file(conn: &Connection, active_path: &Path) -> Result<DatabaseBackupResult> {
    let schema_revision = current_schema_revision(conn)?;
    if let Err(e) = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);") {
        tracing::warn!(error = %e, "WAL checkpoint failed before backup");
    }
    let backup_path = super::backup_path(active_path);
    if active_path.exists() {
        fs::copy(active_path, &backup_path).context("copying active database backup")?;
    } else {
        fs::File::create(&backup_path).context("creating empty backup marker")?;
    }
    let size_bytes = fs::metadata(&backup_path)
        .context("stat database backup")?
        .len();
    Ok(DatabaseBackupResult {
        backup_path,
        size_bytes,
        schema_revision,
    })
}
