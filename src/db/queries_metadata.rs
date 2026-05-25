use std::collections::HashMap;

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};

use super::models::*;
use super::row_mapping::*;

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
    media_type: &str,
) -> Result<Vec<(String, i64, i64)>> {
    let mut stmt = conn.prepare(
        "SELECT job_id, season_number, episode_number FROM jobs
         WHERE series_name = ?1 AND media_type = ?2 AND season_number IS NOT NULL AND episode_number IS NOT NULL",
    )?;
    let rows = stmt.query_map(params![series_name, media_type], |row| {
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
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "UPDATE jobs SET series_name = ?1 WHERE series_name = ?2 AND media_type = ?3",
        params![new_name, old_name, media_type],
    )?;
    // If the new name already exists in series_metadata_links, delete the old entry
    // rather than trying to rename it (the new link was already written by link_series_metadata).
    tx.execute(
        "DELETE FROM series_metadata_links WHERE series_name = ?1 AND media_type = ?2",
        params![old_name, media_type],
    )?;
    tx.execute(
        "UPDATE OR IGNORE media_fingerprints SET series_name = ?1 WHERE series_name = ?2 AND media_type = ?3",
        params![new_name, old_name, media_type],
    )?;
    tx.commit()?;
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
