use std::collections::HashMap;
use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};
use rusqlite::{params, Connection};

use super::models::{
    bool_to_i64, external_metadata_from_row, job_from_row, job_metadata_link_from_row,
    media_fingerprint_from_row, media_marker_from_row, normalize_job_metadata,
    playback_progress_from_row, segment_from_row, segment_part_export_from_row,
    series_metadata_link_from_row, track_from_row, DatabaseBackupResult, DbExport,
    ExternalMetadataRow, JobMetadataLinkRow, MediaFingerprintRow, MediaMarkerRow, MergeResult,
    NewJob, PlaybackProgressRow, ReplaceDatabaseResult, SeriesMetadataLinkRow,
    EXTERNAL_METADATA_SELECT_SQL, JOB_SELECT_SQL, SEGMENT_PART_SELECT_SQL, SEGMENT_SELECT_SQL,
    TRACK_SELECT_SQL,
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
    let mut parts_stmt = conn.prepare(
        &(SEGMENT_PART_SELECT_SQL.to_owned()
            + " ORDER BY job_id ASC, segment_key ASC, part_index ASC"),
    )?;
    let segment_parts = parts_stmt
        .query_map([], segment_part_export_from_row)?
        .map(|r| r.map_err(Into::into))
        .collect::<Result<Vec<_>>>()?;
    Ok(DbExport {
        version: 1,
        schema_revision: current_schema_revision(conn)?,
        jobs,
        tracks,
        segments,
        segment_parts,
        external_metadata: export_external_metadata(conn)?,
        job_metadata_links: export_job_metadata_links(conn)?,
        series_metadata_links: export_series_metadata_links(conn)?,
        playback_progress: export_playback_progress(conn)?,
        media_markers: export_media_markers(conn)?,
        media_fingerprints: export_media_fingerprints(conn)?,
    })
}

fn export_external_metadata(conn: &Connection) -> Result<Vec<ExternalMetadataRow>> {
    let mut stmt = conn.prepare(&(EXTERNAL_METADATA_SELECT_SQL.to_owned() + " ORDER BY id ASC"))?;
    let rows = stmt.query_map([], external_metadata_from_row)?;
    rows.map(|r| r.map_err(Into::into)).collect()
}

fn export_job_metadata_links(conn: &Connection) -> Result<Vec<JobMetadataLinkRow>> {
    let mut stmt = conn.prepare(
        "SELECT job_id, metadata_id, role, created_at FROM job_metadata_links ORDER BY job_id ASC",
    )?;
    let rows = stmt.query_map([], job_metadata_link_from_row)?;
    rows.map(|r| r.map_err(Into::into)).collect()
}

fn export_series_metadata_links(conn: &Connection) -> Result<Vec<SeriesMetadataLinkRow>> {
    let mut stmt = conn.prepare(
        "SELECT media_type, series_name, metadata_id, created_at FROM series_metadata_links ORDER BY media_type ASC, series_name ASC",
    )?;
    let rows = stmt.query_map([], series_metadata_link_from_row)?;
    rows.map(|r| r.map_err(Into::into)).collect()
}

fn export_playback_progress(conn: &Connection) -> Result<Vec<PlaybackProgressRow>> {
    let mut stmt = conn.prepare(
        "SELECT client_id, job_id, position_seconds, duration_seconds, progress_pct, completed, updated_at FROM playback_progress ORDER BY client_id ASC, job_id ASC",
    )?;
    let rows = stmt.query_map([], playback_progress_from_row)?;
    rows.map(|r| r.map_err(Into::into)).collect()
}

fn export_media_markers(conn: &Connection) -> Result<Vec<MediaMarkerRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, job_id, marker_type, start_seconds, end_seconds, source, confidence, enabled, created_at, updated_at FROM media_markers ORDER BY id ASC",
    )?;
    let rows = stmt.query_map([], media_marker_from_row)?;
    rows.map(|r| r.map_err(Into::into)).collect()
}

fn export_media_fingerprints(conn: &Connection) -> Result<Vec<MediaFingerprintRow>> {
    let mut stmt = conn.prepare(
        "SELECT job_id, media_type, series_name, season_number, window_type, window_start_seconds, window_duration_seconds, duration_seconds, fingerprint, fingerprint_source, created_at FROM media_fingerprints ORDER BY job_id ASC, window_type ASC",
    )?;
    let rows = stmt.query_map([], media_fingerprint_from_row)?;
    rows.map(|r| r.map_err(Into::into)).collect()
}

pub fn merge_from_export(
    conn: &mut Connection,
    export: &DbExport,
    bot_index_map: &HashMap<i64, i64>,
) -> Result<MergeResult> {
    let tx = conn.transaction()?;
    let mut merged_jobs = 0;
    let mut merged_segments = 0;
    let mut merged_segment_parts = 0;
    let mut metadata_id_map = HashMap::new();
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
    for part in &export.segment_parts {
        let Some(bot_index) = bot_index_map.get(&part.bot_index) else {
            bail!("missing bot_index_map entry for {}", part.bot_index);
        };
        let (prefix, name) = split_segment_key(&part.segment_key);
        merged_segment_parts += tx.execute(
            "INSERT OR IGNORE INTO segment_parts(
                job_id, segment_key, part_index, file_id, bot_index, file_size, prefix, name
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                part.job_id,
                part.segment_key,
                part.part_index,
                part.file_id,
                *bot_index,
                part.file_size,
                prefix,
                name
            ],
        )?;
    }
    for progress in &export.playback_progress {
        tx.execute(
            "INSERT OR REPLACE INTO playback_progress(client_id, job_id, position_seconds, duration_seconds, progress_pct, completed, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![progress.client_id, progress.job_id, progress.position_seconds, progress.duration_seconds, progress.progress_pct, bool_to_i64(progress.completed) as i64, progress.updated_at],
        )?;
    }
    for meta in &export.external_metadata {
        let target_id: i64 = tx.query_row(
            "INSERT INTO external_metadata(provider, provider_id, media_kind, title, original_title, overview, poster_url, backdrop_url, release_date, year, rating, raw_json, fetched_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
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
                fetched_at=excluded.fetched_at
             RETURNING id",
            params![meta.provider, meta.provider_id, meta.media_kind, meta.title, meta.original_title, meta.overview, meta.poster_url, meta.backdrop_url, meta.release_date, meta.year, meta.rating, meta.raw_json, meta.fetched_at],
            |row| row.get(0),
        )?;
        metadata_id_map.insert(meta.id, target_id);
    }
    for link in &export.job_metadata_links {
        let Some(metadata_id) = metadata_id_map.get(&link.metadata_id) else {
            bail!("missing metadata_id map for {}", link.metadata_id);
        };
        tx.execute(
            "INSERT OR IGNORE INTO job_metadata_links(job_id, metadata_id, role, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![link.job_id, *metadata_id, link.role, link.created_at],
        )?;
    }
    for link in &export.series_metadata_links {
        let Some(metadata_id) = metadata_id_map.get(&link.metadata_id) else {
            bail!("missing metadata_id map for {}", link.metadata_id);
        };
        tx.execute(
            "INSERT OR IGNORE INTO series_metadata_links(media_type, series_name, metadata_id, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![link.media_type, link.series_name, *metadata_id, link.created_at],
        )?;
    }
    for marker in &export.media_markers {
        tx.execute(
            "INSERT INTO media_markers(job_id, marker_type, start_seconds, end_seconds, source, confidence, enabled, created_at, updated_at)
             SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9
             WHERE NOT EXISTS (
                SELECT 1 FROM media_markers
                WHERE job_id = ?1 AND marker_type = ?2 AND start_seconds = ?3 AND end_seconds = ?4 AND source = ?5
             )",
            params![marker.job_id, marker.marker_type, marker.start_seconds, marker.end_seconds, marker.source, marker.confidence, bool_to_i64(marker.enabled) as i64, marker.created_at, marker.updated_at],
        )?;
    }
    for fp in &export.media_fingerprints {
        tx.execute(
            "INSERT OR IGNORE INTO media_fingerprints(job_id, media_type, series_name, season_number, window_type, window_start_seconds, window_duration_seconds, duration_seconds, fingerprint, fingerprint_source, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![fp.job_id, fp.media_type, fp.series_name, fp.season_number, fp.window_type, fp.window_start_seconds, fp.window_duration_seconds, fp.duration_seconds, fp.fingerprint, fp.fingerprint_source, fp.created_at],
        )?;
    }
    tx.commit()?;
    Ok(MergeResult {
        merged_jobs,
        merged_segments,
        merged_segment_parts,
    })
}

pub fn export_database_file(conn: &Connection, output_path: &Path) -> Result<DatabaseBackupResult> {
    if output_path.exists() {
        fs::remove_file(output_path).context("removing existing snapshot file")?;
    }
    let schema_revision = current_schema_revision(conn)?;
    let out = output_path.to_str().ok_or_else(|| {
        anyhow!(
            "snapshot path is not valid UTF-8: {}",
            output_path.display()
        )
    })?;
    conn.execute("VACUUM main INTO ?1", params![out])
        .context("creating sqlite snapshot")?;
    validate_sqlite_snapshot(output_path)?;
    let size_bytes = fs::metadata(output_path)
        .context("stat sqlite snapshot")?
        .len();
    Ok(DatabaseBackupResult {
        backup_path: output_path.to_path_buf(),
        size_bytes,
        schema_revision,
    })
}

pub fn merge_from_database_file(conn: &mut Connection, source_path: &Path) -> Result<MergeResult> {
    validate_sqlite_snapshot(source_path)?;
    let source = init_db(source_path).context("opening import database")?;
    let export = export_to_dict(&source)?;
    let map = auto_same_bot_index_map(&export);
    merge_from_export(conn, &export, &map)
}

fn auto_same_bot_index_map(export: &DbExport) -> HashMap<i64, i64> {
    let mut map = HashMap::new();
    for index in export
        .segments
        .iter()
        .map(|s| s.bot_index)
        .chain(export.segment_parts.iter().map(|p| p.bot_index))
    {
        map.insert(index, index);
    }
    map
}

pub fn validate_sqlite_snapshot(path: &Path) -> Result<()> {
    validate_sqlite_header(path)?;
    let conn = Connection::open(path).context("opening sqlite snapshot")?;
    let check: String = conn
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .context("running sqlite integrity_check")?;
    if check != "ok" {
        bail!("sqlite integrity_check failed: {check}");
    }
    Ok(())
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
    // wal_checkpoint(TRUNCATE) returns one row: (busy, log, checkpointed).
    // busy > 0 means readers hold the WAL; log > checkpointed means frames remain.
    // Either case means the .db file alone is an incomplete snapshot — fail rather
    // than silently produce a stale backup.
    let (busy, log, checkpointed): (i64, i64, i64) = conn
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .context("WAL checkpoint before backup")?;
    if busy > 0 || checkpointed < log {
        anyhow::bail!(
            "WAL checkpoint incomplete (busy={busy}, checkpointed={checkpointed}/{log}); \
             retry when no readers hold the WAL"
        );
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
