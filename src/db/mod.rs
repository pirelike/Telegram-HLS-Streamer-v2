#![allow(dead_code)]

mod migrations;
mod models;
mod queries;
mod transfer;

pub use models::*;
pub use queries::*;
pub use transfer::*;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rusqlite::Connection;

pub const LATEST_SCHEMA_REVISION: i64 = 10;

pub fn init_db(path: &Path) -> Result<Connection> {
    let mut conn = open_with_integrity(path)?;
    apply_connection_pragmas(&conn)?;

    if !migrations::table_exists(&conn, "schema_migrations")? {
        migrations::bootstrap_schema_migrations(&conn)
            .context("bootstrapping schema_migrations")?;
    }
    migrations::ensure_schema_migrations_table(&conn)?;

    let current = current_schema_revision(&conn)?;
    if current > LATEST_SCHEMA_REVISION {
        anyhow::bail!("db schema revision {current} is newer than supported {LATEST_SCHEMA_REVISION}; refusing to start");
    }

    migrations::apply_migrations(&mut conn, current)?;
    Ok(conn)
}

pub fn current_schema_revision(conn: &Connection) -> Result<i64> {
    if !migrations::table_exists(conn, "schema_migrations")? {
        return Ok(0);
    }
    let rev: Option<i64> =
        conn.query_row("SELECT MAX(revision) FROM schema_migrations", [], |row| {
            row.get::<_, Option<i64>>(0)
        })?;
    Ok(rev.unwrap_or(0))
}

fn apply_connection_pragmas(conn: &Connection) -> Result<()> {
    conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")?;
    Ok(())
}

fn open_with_integrity(path: &Path) -> Result<Connection> {
    if !path.exists() {
        return Connection::open(path).context("opening fresh db");
    }
    let conn = Connection::open(path).context("opening existing db")?;
    let result: String = conn
        .query_row("PRAGMA quick_check", [], |row| row.get(0))
        .unwrap_or_else(|_| "error".into());
    if result == "ok" {
        return Ok(conn);
    }
    drop(conn);
    let target = next_corrupted_path(path);
    tracing::warn!(?path, ?target, check = %result, "db integrity check failed; renaming corrupt file and starting fresh");
    std::fs::rename(path, &target).context("renaming corrupt db file")?;
    Connection::open(path).context("opening fresh db after corruption")
}

fn next_corrupted_path(path: &Path) -> PathBuf {
    let base: PathBuf = format!("{}.corrupted", path.display()).into();
    if !base.exists() {
        return base;
    }
    let mut n: u32 = 1;
    loop {
        let p: PathBuf = format!("{}.corrupted.{n}", path.display()).into();
        if !p.exists() {
            return p;
        }
        n += 1;
    }
}

fn validate_setting_key(key: &str) -> Result<()> {
    use crate::settings_registry;
    if key.starts_with('_') || settings_registry::is_public_setting_key(key) {
        Ok(())
    } else {
        anyhow::bail!("unknown setting key: {key}");
    }
}

fn validate_sqlite_header(path: &Path) -> Result<()> {
    let header = std::fs::read(path)
        .with_context(|| format!("reading sqlite header from {}", path.display()))?;
    if header.len() >= 16 && &header[..16] == b"SQLite format 3\0" {
        Ok(())
    } else {
        anyhow::bail!("replacement database is not a SQLite file");
    }
}

fn backup_path(active_path: &Path) -> PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    PathBuf::from(format!("{}.backup_{stamp}", active_path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    pub(crate) fn temp_db_path(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("thls_db_tests_{unique}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    fn assert_schema_rev(conn: &Connection, rev: i64) {
        assert_eq!(current_schema_revision(conn).unwrap(), rev);
    }

    #[test]
    fn fresh_db_creation_is_revision_9_and_idempotent() {
        let path = temp_db_path("streamer.db");
        let conn = init_db(&path).unwrap();
        assert_schema_rev(&conn, 10);
        assert!(migrations::column_exists(&conn, "tracks", "original_stream_index").unwrap());
        assert!(migrations::column_exists(&conn, "segments", "duration").unwrap());
        assert!(migrations::index_exists(&conn, "idx_segments_bot_index").unwrap());
        assert!(migrations::table_exists(&conn, "segment_parts").unwrap());
        drop(conn);

        let conn = init_db(&path).unwrap();
        assert_schema_rev(&conn, 10);
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM schema_migrations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 10);
    }

    #[test]
    fn existing_shortcut_revision_1_schema_upgrades_to_revision_9() {
        let path = temp_db_path("shortcut.db");
        let conn = Connection::open(&path).unwrap();
        apply_connection_pragmas(&conn).unwrap();
        migrations::ensure_schema_migrations_table(&conn).unwrap();
        migrations::run_migration_1(&conn).unwrap();
        migrations::run_migration_2(&conn).unwrap();
        migrations::run_migration_3(&conn).unwrap();
        migrations::run_migration_4(&conn).unwrap();
        migrations::run_migration_5(&conn).unwrap();
        migrations::run_migration_6(&conn).unwrap();
        migrations::run_migration_7(&conn).unwrap();
        migrations::run_migration_8(&conn).unwrap();
        migrations::run_migration_9(&conn).unwrap();
        migrations::run_migration_10(&conn).unwrap();
        conn.execute("INSERT INTO schema_migrations(revision, name) VALUES (1, 'initial schema (rebuild spec rev 9 shape)')", []).unwrap();
        drop(conn);

        let conn = init_db(&path).unwrap();
        assert_schema_rev(&conn, 10);
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM schema_migrations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 10);
    }

    #[test]
    fn legacy_db_without_migrations_bootstraps_and_preserves_rows() {
        let path = temp_db_path("legacy.db");
        let conn = Connection::open(&path).unwrap();
        migrations::run_migration_1(&conn).unwrap();
        conn.execute(
            "INSERT INTO jobs(job_id, filename) VALUES ('job1', 'movie.mkv')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tracks(job_id, track_type, track_index) VALUES ('job1', 'video', 0)",
            [],
        )
        .unwrap();
        conn.execute("INSERT INTO segments(job_id, segment_key, file_id, bot_index) VALUES ('job1', 'video_0/video_0001.ts', 'file', 0)", []).unwrap();
        drop(conn);

        let conn = init_db(&path).unwrap();
        assert_schema_rev(&conn, 10);
        assert!(get_job(&conn, "job1").unwrap().is_some());
        assert_eq!(
            get_segment(&conn, "job1", "video_0/video_0001.ts")
                .unwrap()
                .unwrap()
                .file_id,
            "file"
        );
    }

    #[test]
    fn newer_schema_revision_refuses_startup() {
        let path = temp_db_path("newer.db");
        let conn = Connection::open(&path).unwrap();
        migrations::ensure_schema_migrations_table(&conn).unwrap();
        conn.execute(
            "INSERT INTO schema_migrations(revision, name) VALUES (11, 'future')",
            [],
        )
        .unwrap();
        drop(conn);

        let err = init_db(&path).unwrap_err().to_string();
        assert!(err.contains("newer than supported"));
    }

    #[test]
    fn corrupt_db_is_renamed_and_recreated() {
        let path = temp_db_path("corrupt.db");
        std::fs::write(&path, b"not sqlite").unwrap();
        let conn = init_db(&path).unwrap();
        assert_schema_rev(&conn, 10);
        assert!(PathBuf::from(format!("{}.corrupted", path.display())).exists());
    }

    #[test]
    fn sqlite_identifier_helpers_use_allowlists() {
        let path = temp_db_path("allowlist.db");
        let conn = init_db(&path).unwrap();
        assert!(migrations::column_exists(&conn, "jobs", "has_thumbnail").unwrap());
        assert!(
            migrations::column_exists(&conn, "jobs; DROP TABLE jobs", "has_thumbnail").is_err()
        );
        assert!(
            migrations::column_exists(&conn, "jobs", "has_thumbnail); DROP TABLE jobs").is_err()
        );
    }

    #[test]
    fn disk_database_backup_copies_active_db() {
        let path = temp_db_path("backup.db");
        let conn = init_db(&path).unwrap();
        let result = backup_database_file(&conn, &path).unwrap();
        assert!(result.backup_path.exists());
        assert!(result.size_bytes > 0);
        assert_eq!(result.schema_revision, LATEST_SCHEMA_REVISION);
        assert!(path.exists());
    }

    #[test]
    fn typed_apis_cover_core_db_behaviour() {
        let path = temp_db_path("api.db");
        let mut conn = init_db(&path).unwrap();
        let mut job = NewJob::complete("job1", "movie.mkv");
        job.duration = 10.5;
        let tracks = vec![NewTrack {
            track_type: "video".into(),
            track_index: 0,
            codec: "h264".into(),
            language: "und".into(),
            title: "".into(),
            channels: 0,
            width: 1920,
            height: 1080,
            bitrate: "5M".into(),
            original_stream_index: 0,
        }];
        let segments = vec![NewSegment {
            segment_key: "video_0/video_0001.m4s".into(),
            file_id: "file-id".into(),
            bot_index: 2,
            file_size: 123,
            duration: Some(4.0),
        }];
        save_job(&mut conn, &job, &tracks, &segments, &[]).unwrap();

        assert_eq!(
            get_job(&conn, "job1").unwrap().unwrap().filename,
            "movie.mkv"
        );
        assert_eq!(
            get_job_tracks(&conn, "job1", Some("video")).unwrap().len(),
            1
        );
        assert_eq!(
            get_segment(&conn, "job1", "video_0/video_0001.m4s")
                .unwrap()
                .unwrap()
                .bot_index,
            2
        );
        assert_eq!(
            get_segments_for_prefix(&conn, "job1", "video_0")
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            get_bot_workload_stats(&conn)
                .unwrap()
                .get(&2)
                .unwrap()
                .total_bytes,
            123
        );

        set_setting(&conn, "MAX_CONCURRENT_JOBS", "2").unwrap();
        set_last_bot_index(&conn, 3).unwrap();
        assert_eq!(
            get_all_settings(&conn)
                .unwrap()
                .get("MAX_CONCURRENT_JOBS")
                .unwrap(),
            "2"
        );
        assert_eq!(get_last_bot_index(&conn).unwrap(), 3);
        let bot_id = add_bot(
            &conn,
            "12345678:abcdefghijklmnopqrstuvwxyzabcdefghi",
            -100,
            "main",
        )
        .unwrap();
        assert!(bot_exists(&conn, "12345678:abcdefghijklmnopqrstuvwxyzabcdefghi").unwrap());
        assert_eq!(get_all_bots(&conn).unwrap()[0].id, bot_id);

        let export = export_to_dict(&conn).unwrap();
        assert_eq!(export.version, 1);
        let path2 = temp_db_path("import.db");
        let mut conn2 = init_db(&path2).unwrap();
        let mut map = std::collections::HashMap::new();
        map.insert(2, 5);
        let merged = merge_from_export(&mut conn2, &export, &map).unwrap();
        assert_eq!(merged.merged_jobs, 1);
        assert_eq!(merged.merged_segments, 1);
        assert_eq!(
            get_segment(&conn2, "job1", "video_0/video_0001.m4s")
                .unwrap()
                .unwrap()
                .bot_index,
            5
        );
        assert!(delete_job(&conn2, "job1").unwrap());
        assert!(get_segment(&conn2, "job1", "video_0/video_0001.m4s")
            .unwrap()
            .is_none());
    }

    #[test]
    fn merge_from_export_requires_explicit_bot_index_map_entry() {
        let source_path = temp_db_path("merge_source.db");
        let mut source = init_db(&source_path).unwrap();
        let job = NewJob::complete("job1", "movie.mkv");
        let segments = vec![NewSegment {
            segment_key: "video_0/video_0001.m4s".into(),
            file_id: "file-id".into(),
            bot_index: 1,
            file_size: 42,
            duration: Some(4.0),
        }];
        save_job(&mut source, &job, &[], &segments, &[]).unwrap();
        let export = export_to_dict(&source).unwrap();

        let target_path = temp_db_path("merge_target.db");
        let mut target = init_db(&target_path).unwrap();
        let missing_map = std::collections::HashMap::from([(0, 0)]);
        let err = merge_from_export(&mut target, &export, &missing_map)
            .unwrap_err()
            .to_string();
        assert!(err.contains("missing bot_index_map entry for 1"));

        let remap = std::collections::HashMap::from([(1, 0)]);
        let merged = merge_from_export(&mut target, &export, &remap).unwrap();
        assert_eq!(merged.merged_jobs, 1);
        assert_eq!(merged.merged_segments, 1);
        assert_eq!(
            get_segment(&target, "job1", "video_0/video_0001.m4s")
                .unwrap()
                .unwrap()
                .bot_index,
            0
        );
    }

    #[test]
    fn listing_helpers_filter_and_group_library_rows() {
        let path = temp_db_path("library.db");
        let mut conn = init_db(&path).unwrap();
        for (job_id, filename, media_type, series, season, episode) in [
            ("film1", "Film One", "Film", "", None, None),
            ("show1", "Episode 1", "Series", "My Show", Some(1), Some(1)),
            ("show2", "Episode 2", "Series", "My Show", Some(1), Some(2)),
            ("special", "Special", "Series", "My Show", None, Some(1)),
            (
                "anime1",
                "Anime Ep",
                "Anime TV",
                "Ani Show",
                Some(1),
                Some(1),
            ),
        ] {
            let mut job = NewJob::complete(job_id, filename);
            job.media_type = media_type.into();
            job.series_name = series.into();
            job.is_series = !series.is_empty();
            job.season_number = season;
            job.episode_number = episode;
            save_job(&mut conn, &job, &[], &[], &[]).unwrap();
        }

        let filter = JobListFilter {
            category: Some("Series".into()),
            search: Some("Episode".into()),
            ..Default::default()
        };
        assert_eq!(list_jobs(&conn, &filter).unwrap().len(), 2);
        assert_eq!(count_jobs(&conn, &filter).unwrap(), 2);

        let filter = JobListFilter {
            category: Some("Series".into()),
            ..Default::default()
        };
        let series = list_series_groups(&conn, &filter).unwrap();
        assert_eq!(series.len(), 1);
        assert_eq!(series[0].series_name, "My Show");
        assert_eq!(series[0].episode_count, 3);

        let seasons = list_season_groups(&conn, &filter).unwrap();
        assert_eq!(seasons.len(), 2);
        assert!(seasons.iter().any(|s| s.season_number == Some(1)));
        assert!(seasons.iter().any(|s| s.season_number.is_none()));

        let filter = JobListFilter {
            category: Some("Series".into()),
            series_name: Some("My Show".into()),
            season_number_is_null: true,
            ..Default::default()
        };
        assert_eq!(list_jobs(&conn, &filter).unwrap()[0].job_id, "special");
    }

    #[test]
    fn live_database_replacement_backs_up_and_migrates() {
        let active = temp_db_path("active.db");
        let source = temp_db_path("source.db");
        let conn = init_db(&active).unwrap();
        drop(conn);
        let conn = Connection::open(&source).unwrap();
        migrations::run_migration_1(&conn).unwrap();
        drop(conn);

        let result = replace_database_file(&active, &source).unwrap();
        assert!(result.backup_path.exists());
        assert_eq!(result.schema_revision, 10);
        assert!(!source.exists());
        let conn = init_db(&active).unwrap();
        assert_schema_rev(&conn, 10);
    }

    #[test]
    fn round_robin_counter_survives_db_reopen() {
        let path = temp_db_path("rr.db");
        {
            let mut conn = init_db(&path).unwrap();
            set_last_bot_index(&mut conn, 7).unwrap();
            assert_eq!(get_last_bot_index(&conn).unwrap(), 7);
        }
        // Reopen the same DB file
        {
            let conn = init_db(&path).unwrap();
            assert_eq!(get_last_bot_index(&conn).unwrap(), 7);
        }
    }
}
