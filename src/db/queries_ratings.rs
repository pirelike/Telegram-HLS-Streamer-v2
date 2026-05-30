use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};

use super::models::*;

pub fn set_rating(conn: &Connection, user_id: &str, job_id: &str, liked: bool) -> Result<()> {
    conn.execute(
        "INSERT INTO user_ratings(user_id, job_id, liked, rated_at)
         VALUES (?1, ?2, ?3, CURRENT_TIMESTAMP)
         ON CONFLICT(user_id, job_id) DO UPDATE SET
            liked = excluded.liked,
            rated_at = CURRENT_TIMESTAMP",
        params![user_id, job_id, bool_to_i64(liked)],
    )?;
    Ok(())
}

pub fn delete_rating(conn: &Connection, user_id: &str, job_id: &str) -> Result<bool> {
    Ok(conn.execute(
        "DELETE FROM user_ratings WHERE user_id = ?1 AND job_id = ?2",
        params![user_id, job_id],
    )? > 0)
}

pub fn get_rating(conn: &Connection, user_id: &str, job_id: &str) -> Result<Option<RatingRow>> {
    conn.query_row(
        "SELECT user_id, job_id, liked, rated_at
         FROM user_ratings WHERE user_id = ?1 AND job_id = ?2",
        params![user_id, job_id],
        rating_from_row,
    )
    .optional()
    .map_err(Into::into)
}

pub fn list_ratings(conn: &Connection, user_id: &str) -> Result<Vec<RatingRow>> {
    let mut stmt = conn.prepare(
        "SELECT user_id, job_id, liked, rated_at
         FROM user_ratings WHERE user_id = ?1 ORDER BY rated_at DESC",
    )?;
    let rows = stmt.query_map(params![user_id], rating_from_row)?;
    rows.map(|row| row.map_err(Into::into)).collect()
}

pub fn rating_summary(conn: &Connection, job_id: &str) -> Result<(i64, i64)> {
    conn.query_row(
        "SELECT
            COALESCE(SUM(CASE WHEN liked = 1 THEN 1 ELSE 0 END), 0),
            COUNT(*)
         FROM user_ratings WHERE job_id = ?1",
        params![job_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .map_err(Into::into)
}

pub fn list_user_preferences(conn: &Connection, user_id: &str) -> Result<Vec<PreferenceRow>> {
    let mut stmt = conn.prepare(
        "SELECT user_id, key, value
         FROM user_preferences WHERE user_id = ?1 ORDER BY key ASC",
    )?;
    let rows = stmt.query_map(params![user_id], preference_from_row)?;
    rows.map(|row| row.map_err(Into::into)).collect()
}

pub fn set_user_preference(conn: &Connection, user_id: &str, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO user_preferences(user_id, key, value)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(user_id, key) DO UPDATE SET value = excluded.value",
        params![user_id, key, value],
    )?;
    Ok(())
}

pub fn export_ratings(conn: &Connection) -> Result<Vec<RatingRow>> {
    let mut stmt = conn.prepare(
        "SELECT user_id, job_id, liked, rated_at FROM user_ratings ORDER BY user_id ASC, job_id ASC",
    )?;
    let rows = stmt.query_map([], rating_from_row)?;
    rows.map(|row| row.map_err(Into::into)).collect()
}

pub fn export_preferences(conn: &Connection) -> Result<Vec<PreferenceRow>> {
    let mut stmt = conn.prepare(
        "SELECT user_id, key, value FROM user_preferences ORDER BY user_id ASC, key ASC",
    )?;
    let rows = stmt.query_map([], preference_from_row)?;
    rows.map(|row| row.map_err(Into::into)).collect()
}

fn rating_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RatingRow> {
    Ok(RatingRow {
        user_id: row.get(0)?,
        job_id: row.get(1)?,
        liked: row.get::<_, i64>(2)? == 1,
        rated_at: row.get(3)?,
    })
}

fn preference_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<PreferenceRow> {
    Ok(PreferenceRow {
        user_id: row.get(0)?,
        key: row.get(1)?,
        value: row.get(2)?,
    })
}

fn bool_to_i64(value: bool) -> i64 {
    if value {
        1
    } else {
        0
    }
}
