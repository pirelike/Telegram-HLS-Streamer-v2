use anyhow::Result;
use rusqlite::{params, Connection};

use super::models::*;
use super::row_mapping::job_from_row;

pub fn toggle_favorite(conn: &Connection, user_id: &str, job_id: &str) -> Result<bool> {
    toggle_user_job(conn, "user_favorites", user_id, job_id)
}

pub fn list_favorites(conn: &Connection, user_id: &str) -> Result<Vec<UserJobRow>> {
    list_user_jobs(conn, "user_favorites", user_id)
}

pub fn favorite_exists(conn: &Connection, user_id: &str, job_id: &str) -> Result<bool> {
    user_job_exists(conn, "user_favorites", user_id, job_id)
}

pub fn toggle_watchlist(conn: &Connection, user_id: &str, job_id: &str) -> Result<bool> {
    toggle_user_job(conn, "user_watchlist", user_id, job_id)
}

pub fn list_watchlist(conn: &Connection, user_id: &str) -> Result<Vec<UserJobRow>> {
    list_user_jobs(conn, "user_watchlist", user_id)
}

pub fn watchlist_exists(conn: &Connection, user_id: &str, job_id: &str) -> Result<bool> {
    user_job_exists(conn, "user_watchlist", user_id, job_id)
}

fn toggle_user_job(conn: &Connection, table: &str, user_id: &str, job_id: &str) -> Result<bool> {
    let table = list_table_name(table)?;
    let exists = user_job_exists(conn, table, user_id, job_id)?;
    if exists {
        conn.execute(
            &format!("DELETE FROM {table} WHERE user_id = ?1 AND job_id = ?2"),
            params![user_id, job_id],
        )?;
        Ok(false)
    } else {
        conn.execute(
            &format!("INSERT INTO {table}(user_id, job_id) VALUES (?1, ?2)"),
            params![user_id, job_id],
        )?;
        Ok(true)
    }
}

fn user_job_exists(conn: &Connection, table: &str, user_id: &str, job_id: &str) -> Result<bool> {
    let table = list_table_name(table)?;
    let count: i64 = conn.query_row(
        &format!("SELECT COUNT(*) FROM {table} WHERE user_id = ?1 AND job_id = ?2"),
        params![user_id, job_id],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

fn list_user_jobs(conn: &Connection, table: &str, user_id: &str) -> Result<Vec<UserJobRow>> {
    let table = list_table_name(table)?;
    let sql = format!(
        "SELECT jobs.job_id, jobs.filename, jobs.duration, jobs.file_size, jobs.video_codec,
                jobs.video_width, jobs.video_height, jobs.status, jobs.created_at,
                jobs.media_type, jobs.series_name, jobs.has_thumbnail, jobs.is_series,
                jobs.season_number, jobs.episode_number, jobs.part_number, jobs.error,
                jobs.source_path, jobs.episode_title, jobs.source_bitrate,
                uj.added_at
         FROM {table} uj
         JOIN jobs ON jobs.job_id = uj.job_id
         WHERE uj.user_id = ?1
         ORDER BY uj.added_at DESC"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![user_id], |row| {
        Ok(UserJobRow {
            job: job_from_row(row)?,
            marked_at: row.get(20)?,
        })
    })?;
    rows.map(|row| row.map_err(Into::into)).collect()
}

fn list_table_name(table: &str) -> Result<&str> {
    match table {
        "user_favorites" | "user_watchlist" => Ok(table),
        _ => anyhow::bail!("unknown user list table: {table}"),
    }
}

pub fn export_favorites(conn: &Connection) -> Result<Vec<FavoriteRow>> {
    let mut stmt = conn.prepare(
        "SELECT user_id, job_id, added_at FROM user_favorites ORDER BY user_id ASC, job_id ASC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(FavoriteRow {
            user_id: row.get(0)?,
            job_id: row.get(1)?,
            added_at: row.get(2)?,
        })
    })?;
    rows.map(|row| row.map_err(Into::into)).collect()
}

pub fn export_watchlist(conn: &Connection) -> Result<Vec<WatchlistRow>> {
    let mut stmt = conn.prepare(
        "SELECT user_id, job_id, added_at FROM user_watchlist ORDER BY user_id ASC, job_id ASC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(WatchlistRow {
            user_id: row.get(0)?,
            job_id: row.get(1)?,
            added_at: row.get(2)?,
        })
    })?;
    rows.map(|row| row.map_err(Into::into)).collect()
}
