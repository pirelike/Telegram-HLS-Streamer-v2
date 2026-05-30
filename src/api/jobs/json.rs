use std::collections::HashMap;

use anyhow::Result;
use serde_json::{json, Value};

use super::types::{JobState, JobStatus};
use crate::db;

pub(super) fn job_status_json(
    job: &JobState,
    queue_position: Option<usize>,
    queue_depth: Option<usize>,
) -> Value {
    json!({
        "job_id": job.job_id,
        "status": job.status.as_str(),
        "progress": job.progress,
        "step": job.step,
        "total_steps": job.total_steps,
        "description": job.description,
        "queue_position": queue_position,
        "queue_depth": queue_depth,
        "analysis": job.analysis,
        "error": job.error,
        "filename": job.filename,
        "metadata": job.metadata,
    })
}

pub(super) fn queue_position(jobs: &HashMap<String, JobState>, job_id: &str) -> Option<usize> {
    let job = jobs.get(job_id)?;
    if job.status != JobStatus::Queued {
        return None;
    }
    let mut queued: Vec<_> = jobs
        .values()
        .filter(|j| j.status == JobStatus::Queued)
        .collect();
    queued.sort_by_key(|j| j.queued_at);
    queued
        .iter()
        .position(|j| j.job_id == job_id)
        .map(|pos| pos + 1)
}

pub(super) fn queue_depth(jobs: &HashMap<String, JobState>, job_id: &str) -> Option<usize> {
    let job = jobs.get(job_id)?;
    if job.status != JobStatus::Queued {
        return None;
    }
    Some(
        jobs.values()
            .filter(|j| j.status == JobStatus::Queued)
            .count(),
    )
}

pub(crate) fn normalize_category(value: Option<&str>) -> Result<Option<String>, String> {
    match value.map(str::trim).filter(|s| !s.is_empty()) {
        None | Some("all") => Ok(None),
        Some("Film") | Some("Series") | Some("Anime Film") | Some("Anime TV") => {
            Ok(value.map(str::trim).map(ToOwned::to_owned))
        }
        Some(_) => Err("category must be Film, Series, Anime Film, Anime TV, or all".into()),
    }
}

pub fn job_json(job: db::JobRow, poster_url: Option<&str>) -> Value {
    json!({
        "job_id": job.job_id,
        "filename": job.filename,
        "duration": job.duration,
        "file_size": job.file_size,
        "video_codec": job.video_codec,
        "video_width": job.video_width,
        "video_height": job.video_height,
        "status": job.status,
        "created_at": job.created_at,
        "media_type": job.media_type,
        "series_name": job.series_name,
        "has_thumbnail": job.has_thumbnail,
        "is_series": job.is_series,
        "season_number": job.season_number,
        "episode_number": job.episode_number,
        "part_number": job.part_number,
        "episode_title": job.episode_title,
        "poster_url": poster_url.filter(|s| !s.is_empty()),
    })
}

pub fn series_group_json(row: db::SeriesGroupRow, poster_url: Option<&str>) -> Value {
    json!({
        "series_name": row.series_name,
        "media_type": row.media_type,
        "episode_count": row.episode_count,
        "last_updated": row.last_updated,
        "job_id": row.job_id,
        "has_thumbnail": row.has_thumbnail,
        "poster_url": poster_url.filter(|s| !s.is_empty()),
    })
}

pub fn season_group_json(row: db::SeasonGroupRow, poster_url: Option<&str>) -> Value {
    json!({
        "series_name": row.series_name,
        "season_number": row.season_number,
        "episode_count": row.episode_count,
        "last_updated": row.last_updated,
        "job_id": row.job_id,
        "has_thumbnail": row.has_thumbnail,
        "poster_url": poster_url.filter(|s| !s.is_empty()),
    })
}

pub(super) fn non_empty_string(key: &str, value: Value) -> Result<String, String> {
    let Value::String(value) = value else {
        return Err(format!("{key} must be a string"));
    };
    let value = value.trim().to_string();
    if value.is_empty() {
        return Err(format!("{key} must not be empty"));
    }
    Ok(value)
}

pub(super) fn nullable_string(key: &str, value: Value) -> Result<String, String> {
    match value {
        Value::Null => Ok(String::new()),
        Value::String(value) => Ok(value.trim().to_string()),
        _ => Err(format!("{key} must be a string or null")),
    }
}

pub(super) fn bool_value(key: &str, value: Value) -> Result<bool, String> {
    match value {
        Value::Bool(v) => Ok(v),
        Value::Number(n) => match n.as_i64() {
            Some(0) => Ok(false),
            Some(1) => Ok(true),
            _ => Err(format!("{key} must be boolean, 0, or 1")),
        },
        _ => Err(format!("{key} must be boolean, 0, or 1")),
    }
}

pub(super) fn nullable_i64(key: &str, value: Value) -> Result<Option<i64>, String> {
    match value {
        Value::Null => Ok(None),
        Value::Number(n) => n
            .as_i64()
            .map(Some)
            .ok_or_else(|| format!("{key} must be an integer or null")),
        Value::String(s) if s.trim().is_empty() => Ok(None),
        Value::String(s) => s
            .trim()
            .parse::<i64>()
            .map(Some)
            .map_err(|_| format!("{key} must be an integer or null")),
        _ => Err(format!("{key} must be an integer or null")),
    }
}
