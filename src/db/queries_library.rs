use anyhow::Result;
use rusqlite::{params, Connection};

use super::models::*;
use super::row_mapping::*;

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
    let sql = format!(
        "WITH filtered AS (SELECT * FROM jobs {where_sql})
         SELECT COUNT(*) FROM (
            SELECT series_name FROM filtered
            WHERE COALESCE(series_name, '') != ''
            GROUP BY series_name
         )"
    );
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
    let sql = format!(
        "WITH filtered AS (SELECT * FROM jobs {where_sql})
         SELECT COUNT(*) FROM (
            SELECT series_name, season_number FROM filtered
            WHERE COALESCE(series_name, '') != ''
            GROUP BY series_name, season_number
         )"
    );
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
