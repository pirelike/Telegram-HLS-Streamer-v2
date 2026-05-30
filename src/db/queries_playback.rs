use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};

use super::models::*;
use super::row_mapping::*;

pub fn save_playback_progress(conn: &Connection, progress: &NewPlaybackProgress) -> Result<()> {
    let pct = if progress.duration_seconds > 0.0 {
        ((progress.position_seconds / progress.duration_seconds) * 100.0).round() as i64
    } else {
        0
    };
    let pct = pct.clamp(0, 100);
    let completed = pct >= 95;
    if let Some(user_id) = progress.user_id.as_deref() {
        let scoped_client_id = format!("u_{user_id}");
        conn.execute(
            "INSERT INTO playback_progress(client_id, user_id, job_id, position_seconds, duration_seconds, progress_pct, completed, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, CURRENT_TIMESTAMP)
             ON CONFLICT(user_id, job_id) WHERE user_id IS NOT NULL DO UPDATE SET
                client_id=excluded.client_id,
                position_seconds=excluded.position_seconds,
                duration_seconds=excluded.duration_seconds,
                progress_pct=excluded.progress_pct,
                completed=excluded.completed,
                updated_at=CURRENT_TIMESTAMP",
            params![
                scoped_client_id,
                user_id,
                progress.job_id,
                progress.position_seconds,
                progress.duration_seconds,
                pct,
                { bool_to_i64(completed) },
            ],
        )?;
    } else {
        conn.execute(
            "INSERT INTO playback_progress(client_id, user_id, job_id, position_seconds, duration_seconds, progress_pct, completed, updated_at)
             VALUES (?1, NULL, ?2, ?3, ?4, ?5, ?6, CURRENT_TIMESTAMP)
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
                { bool_to_i64(completed) },
            ],
        )?;
    }
    Ok(())
}

pub fn get_playback_progress(
    conn: &Connection,
    client_id: &str,
    user_id: Option<&str>,
    job_id: &str,
) -> Result<Option<PlaybackProgressRow>> {
    if let Some(user_id) = user_id {
        let user_progress = conn
            .query_row(
                "SELECT client_id, user_id, job_id, position_seconds, duration_seconds, progress_pct, completed, updated_at
                 FROM playback_progress WHERE user_id = ?1 AND job_id = ?2",
                params![user_id, job_id],
                playback_progress_from_row,
            )
            .optional()?;
        if user_progress.is_some() {
            return Ok(user_progress);
        }
    }
    conn.query_row(
        "SELECT client_id, user_id, job_id, position_seconds, duration_seconds, progress_pct, completed, updated_at
         FROM playback_progress WHERE client_id = ?1 AND user_id IS NULL AND job_id = ?2",
        params![client_id, job_id],
        playback_progress_from_row,
    )
    .optional()
    .map_err(Into::into)
}

pub fn list_playback_progress(
    conn: &Connection,
    client_id: &str,
    user_id: Option<&str>,
) -> Result<Vec<PlaybackProgressRow>> {
    if let Some(user_id) = user_id {
        let mut stmt = conn.prepare(
            "SELECT client_id, user_id, job_id, position_seconds, duration_seconds, progress_pct, completed, updated_at
             FROM playback_progress WHERE user_id = ?1 AND completed = 0
             ORDER BY updated_at DESC",
        )?;
        let rows = stmt.query_map(params![user_id], playback_progress_from_row)?;
        return rows.map(|r| r.map_err(Into::into)).collect();
    }
    let mut stmt = conn.prepare(
        "SELECT client_id, user_id, job_id, position_seconds, duration_seconds, progress_pct, completed, updated_at
         FROM playback_progress WHERE client_id = ?1 AND user_id IS NULL AND completed = 0
         ORDER BY updated_at DESC",
    )?;
    let rows = stmt.query_map(params![client_id], playback_progress_from_row)?;
    rows.map(|r| r.map_err(Into::into)).collect()
}

pub fn list_watch_history(conn: &Connection, user_id: &str) -> Result<Vec<PlaybackProgressRow>> {
    let mut stmt = conn.prepare(
        "SELECT client_id, user_id, job_id, position_seconds, duration_seconds, progress_pct, completed, updated_at
         FROM playback_progress WHERE user_id = ?1 AND completed = 1
         ORDER BY updated_at DESC",
    )?;
    let rows = stmt.query_map(params![user_id], playback_progress_from_row)?;
    rows.map(|r| r.map_err(Into::into)).collect()
}

pub fn delete_playback_progress(
    conn: &Connection,
    client_id: &str,
    user_id: Option<&str>,
    job_id: &str,
) -> Result<bool> {
    if let Some(user_id) = user_id {
        return Ok(conn.execute(
            "DELETE FROM playback_progress WHERE user_id = ?1 AND job_id = ?2",
            params![user_id, job_id],
        )? > 0);
    }
    Ok(conn.execute(
        "DELETE FROM playback_progress WHERE client_id = ?1 AND user_id IS NULL AND job_id = ?2",
        params![client_id, job_id],
    )? > 0)
}

pub fn next_unwatched_episode(
    conn: &Connection,
    user_id: &str,
    media_type: &str,
    series_name: &str,
) -> Result<Option<JobRow>> {
    let sql = format!(
        "{} WHERE media_type = ?1
              AND series_name = ?2
              AND is_series = 1
              AND status = 'complete'
              AND NOT EXISTS (
                  SELECT 1 FROM playback_progress p
                  WHERE p.user_id = ?3
                    AND p.job_id = jobs.job_id
                    AND p.completed = 1
              )
            ORDER BY
              CASE WHEN season_number IS NULL THEN 1 ELSE 0 END ASC,
              COALESCE(season_number, 0) ASC,
              COALESCE(episode_number, 0) ASC,
              COALESCE(part_number, 0) ASC,
              created_at ASC
            LIMIT 1",
        JOB_SELECT_SQL
    );
    conn.query_row(
        &sql,
        params![media_type, series_name, user_id],
        job_from_row,
    )
    .optional()
    .map_err(Into::into)
}

// --- Media markers ---

pub fn save_media_markers(
    conn: &Connection,
    job_id: &str,
    markers: &[NewMediaMarker],
) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
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
