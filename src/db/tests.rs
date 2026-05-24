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

fn assert_connection_pragmas(conn: &Connection) {
    let fk: i64 = conn
        .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
        .unwrap();
    let journal: String = conn
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .unwrap();
    let synchronous: i64 = conn
        .query_row("PRAGMA synchronous", [], |row| row.get(0))
        .unwrap();
    let busy_timeout: i64 = conn
        .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
        .unwrap();
    let temp_store: i64 = conn
        .query_row("PRAGMA temp_store", [], |row| row.get(0))
        .unwrap();
    let cache_size: i64 = conn
        .query_row("PRAGMA cache_size", [], |row| row.get(0))
        .unwrap();

    assert_eq!(fk, 1);
    assert_eq!(journal, "wal");
    assert_eq!(synchronous, 1);
    assert_eq!(busy_timeout, 5000);
    assert_eq!(temp_store, 2);
    assert_eq!(cache_size, -64000);
}

fn test_metadata(provider_id: &str, title: &str) -> NewExternalMetadata {
    NewExternalMetadata {
        provider: "tmdb".into(),
        provider_id: provider_id.into(),
        media_kind: "movie".into(),
        title: title.into(),
        original_title: title.into(),
        overview: String::new(),
        poster_url: String::new(),
        backdrop_url: String::new(),
        release_date: String::new(),
        year: None,
        rating: None,
        raw_json: "{}".into(),
    }
}

#[test]
fn fresh_db_creation_is_revision_9_and_idempotent() {
    let path = temp_db_path("streamer.db");
    let conn = init_db(&path).unwrap();
    assert_schema_rev(&conn, LATEST_SCHEMA_REVISION);
    assert!(migrations::column_exists(&conn, "tracks", "original_stream_index").unwrap());
    assert!(migrations::column_exists(&conn, "segments", "duration").unwrap());
    assert!(migrations::index_exists(&conn, "idx_segments_bot_index").unwrap());
    assert!(migrations::index_exists(&conn, "idx_jobs_media_created").unwrap());
    assert!(migrations::index_exists(&conn, "idx_jobs_series_created").unwrap());
    assert!(migrations::table_exists(&conn, "segment_parts").unwrap());
    assert!(migrations::column_exists(&conn, "jobs", "source_path").unwrap());
    assert!(migrations::column_exists(&conn, "jobs", "created_at_unix").unwrap());
    assert!(migrations::column_exists(&conn, "tracks", "bitrate_bps").unwrap());
    assert!(migrations::column_exists(&conn, "tracks", "mode").unwrap());
    assert!(migrations::column_exists(&conn, "segments", "prefix").unwrap());
    assert!(migrations::column_exists(&conn, "segments", "name").unwrap());
    assert!(migrations::column_exists(&conn, "segments", "encryption_nonce").unwrap());
    assert!(migrations::column_exists(&conn, "segment_parts", "encryption_nonce").unwrap());
    assert!(migrations::column_exists(&conn, "db_sync_uploads", "encryption_nonce").unwrap());
    assert!(migrations::table_exists(&conn, "kv_internal").unwrap());
    assert_connection_pragmas(&conn);
    drop(conn);

    let conn = init_db(&path).unwrap();
    assert_schema_rev(&conn, LATEST_SCHEMA_REVISION);
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM schema_migrations", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, LATEST_SCHEMA_REVISION);
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
    conn.execute(
        "INSERT INTO settings(key, value) VALUES ('HLS_SEGMENT_DURATION', '4')",
        [],
    )
    .unwrap();
    conn.execute("INSERT INTO schema_migrations(revision, name) VALUES (1, 'initial schema (rebuild spec rev 9 shape)')", []).unwrap();
    drop(conn);

    let conn = init_db(&path).unwrap();
    assert_schema_rev(&conn, LATEST_SCHEMA_REVISION);
    let stale_setting_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM settings WHERE key = 'HLS_SEGMENT_DURATION'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(stale_setting_count, 0);
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM schema_migrations", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, LATEST_SCHEMA_REVISION);
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

    std::env::set_var("THLS_BOOTSTRAP_FROM_LEGACY", "1");
    let conn = init_db(&path).unwrap();
    std::env::remove_var("THLS_BOOTSTRAP_FROM_LEGACY");
    assert_schema_rev(&conn, LATEST_SCHEMA_REVISION);
    assert!(get_job(&conn, "job1").unwrap().is_some());
    assert!(get_job(&conn, "job1")
        .unwrap()
        .unwrap()
        .source_path
        .is_none());
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
        "INSERT INTO schema_migrations(revision, name) VALUES (999, 'future')",
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
    assert_schema_rev(&conn, LATEST_SCHEMA_REVISION);
    assert!(PathBuf::from(format!("{}.corrupted", path.display())).exists());
}

#[test]
fn pool_allows_multiple_usable_connections() {
    let path = temp_db_path("pool.db");
    let pool = init_db_pool(&path).unwrap();
    let conn1 = pool.get().unwrap();
    let conn2 = pool.get().unwrap();

    assert_schema_rev(&conn1, LATEST_SCHEMA_REVISION);
    assert_schema_rev(&conn2, LATEST_SCHEMA_REVISION);
    assert_connection_pragmas(&conn1);
    assert_connection_pragmas(&conn2);
    assert!(pool.state().connections >= 2);
}

#[test]
fn sqlite_identifier_helpers_use_allowlists() {
    let path = temp_db_path("allowlist.db");
    let conn = init_db(&path).unwrap();
    assert!(migrations::column_exists(&conn, "jobs", "has_thumbnail").unwrap());
    assert!(migrations::column_exists(&conn, "jobs; DROP TABLE jobs", "has_thumbnail").is_err());
    assert!(migrations::column_exists(&conn, "jobs", "has_thumbnail); DROP TABLE jobs").is_err());
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
        is_split: false,
        encryption_nonce: Some("00112233445566778899aabb".into()),
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
        get_segment(&conn, "job1", "video_0/video_0001.m4s")
            .unwrap()
            .unwrap()
            .encryption_nonce
            .as_deref(),
        Some("00112233445566778899aabb")
    );
    let (mode, bitrate_bps): (String, i64) = conn
        .query_row(
            "SELECT mode, bitrate_bps FROM tracks WHERE job_id = 'job1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(mode, "encode");
    assert_eq!(bitrate_bps, 5_000_000);
    let (prefix, name): (String, String) = conn
        .query_row(
            "SELECT prefix, name FROM segments WHERE job_id = 'job1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(prefix, "video_0");
    assert_eq!(name, "video_0001.m4s");
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
    assert_eq!(
        get_segment(&conn2, "job1", "video_0/video_0001.m4s")
            .unwrap()
            .unwrap()
            .encryption_nonce
            .as_deref(),
        Some("00112233445566778899aabb")
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
        is_split: false,
        encryption_nonce: None,
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
fn save_external_metadata_returns_existing_id_on_upsert() {
    let path = temp_db_path("metadata_upsert.db");
    let conn = init_db(&path).unwrap();

    let first = test_metadata("1", "Original");
    let first_id = save_external_metadata(&conn, &first).unwrap();
    let second_id = save_external_metadata(&conn, &test_metadata("2", "Other")).unwrap();
    assert_ne!(first_id, second_id);

    let mut updated = first;
    updated.title = "Updated".into();
    let returned_id = save_external_metadata(&conn, &updated).unwrap();
    assert_eq!(returned_id, first_id);
    assert_eq!(
        get_external_metadata_by_id(&conn, first_id)
            .unwrap()
            .unwrap()
            .title,
        "Updated"
    );
}

#[test]
fn merge_from_export_remaps_metadata_and_preserves_marker_collisions() {
    let source_path = temp_db_path("metadata_merge_source.db");
    let mut source = init_db(&source_path).unwrap();
    let mut source_job = NewJob::complete("source-job", "episode.mkv");
    source_job.media_type = "Series".into();
    source_job.series_name = "Shared Show".into();
    source_job.is_series = true;
    source_job.season_number = Some(1);
    source_job.episode_number = Some(1);
    save_job(&mut source, &source_job, &[], &[], &[]).unwrap();
    let source_meta_id =
        save_external_metadata(&source, &test_metadata("source", "Source")).unwrap();
    link_job_metadata(&source, "source-job", source_meta_id, "primary").unwrap();
    link_series_metadata(&source, "Series", "Shared Show", source_meta_id).unwrap();
    save_media_markers(
        &source,
        "source-job",
        &[NewMediaMarker {
            marker_type: "intro".into(),
            start_seconds: 10.0,
            end_seconds: 70.0,
            source: "chapter".into(),
            confidence: 1.0,
        }],
    )
    .unwrap();
    let export = export_to_dict(&source).unwrap();

    let target_path = temp_db_path("metadata_merge_target.db");
    let mut target = init_db(&target_path).unwrap();
    save_job(
        &mut target,
        &NewJob::complete("target-job", "other.mkv"),
        &[],
        &[],
        &[],
    )
    .unwrap();
    let target_meta_id =
        save_external_metadata(&target, &test_metadata("target", "Target")).unwrap();
    save_media_markers(
        &target,
        "target-job",
        &[NewMediaMarker {
            marker_type: "intro".into(),
            start_seconds: 5.0,
            end_seconds: 30.0,
            source: "chapter".into(),
            confidence: 1.0,
        }],
    )
    .unwrap();
    let target_marker_id = get_media_markers(&target, "target-job", false).unwrap()[0].id;

    merge_from_export(&mut target, &export, &std::collections::HashMap::new()).unwrap();

    let links = get_job_metadata_links(&target, "source-job").unwrap();
    assert_eq!(links.len(), 1);
    assert_ne!(links[0].0.metadata_id, target_meta_id);
    assert_eq!(links[0].1.provider_id, "source");

    let series_link = get_series_metadata_link(&target, "Series", "Shared Show")
        .unwrap()
        .unwrap();
    assert_eq!(series_link.1.provider_id, "source");

    let source_markers = get_media_markers(&target, "source-job", false).unwrap();
    assert_eq!(source_markers.len(), 1);
    assert_ne!(source_markers[0].id, target_marker_id);
    assert_eq!(source_markers[0].marker_type, "intro");
}

#[test]
fn replace_auto_media_markers_preserves_manual_markers() {
    let path = temp_db_path("replace_auto_markers.db");
    let mut conn = init_db(&path).unwrap();
    save_job(
        &mut conn,
        &NewJob::complete("marker-job", "episode.mkv"),
        &[],
        &[],
        &[],
    )
    .unwrap();
    save_media_markers(
        &conn,
        "marker-job",
        &[
            NewMediaMarker {
                marker_type: "intro".into(),
                start_seconds: 10.0,
                end_seconds: 70.0,
                source: "manual".into(),
                confidence: 1.0,
            },
            NewMediaMarker {
                marker_type: "intro".into(),
                start_seconds: 12.0,
                end_seconds: 72.0,
                source: "chromaprint".into(),
                confidence: 0.7,
            },
        ],
    )
    .unwrap();

    replace_auto_media_markers(
        &conn,
        "marker-job",
        &[NewMediaMarker {
            marker_type: "outro".into(),
            start_seconds: 900.0,
            end_seconds: 960.0,
            source: "chromaprint".into(),
            confidence: 0.8,
        }],
    )
    .unwrap();

    let markers = get_media_markers(&conn, "marker-job", false).unwrap();
    assert_eq!(markers.len(), 2);
    assert!(markers.iter().any(|m| m.source == "manual"));
    assert!(markers
        .iter()
        .any(|m| m.marker_type == "outro" && m.source == "chromaprint"));
}

#[test]
fn playback_progress_marks_completed_near_end() {
    let path = temp_db_path("playback_progress.db");
    let mut conn = init_db(&path).unwrap();
    save_job(
        &mut conn,
        &NewJob::complete("progress-job", "episode.mkv"),
        &[],
        &[],
        &[],
    )
    .unwrap();

    save_playback_progress(
        &conn,
        &NewPlaybackProgress {
            client_id: "client1".into(),
            job_id: "progress-job".into(),
            position_seconds: 960.0,
            duration_seconds: 1000.0,
        },
    )
    .unwrap();

    let progress = get_playback_progress(&conn, "client1", "progress-job")
        .unwrap()
        .unwrap();
    assert!(progress.completed);
    assert_eq!(progress.progress_pct, 96);
    assert!(list_playback_progress(&conn, "client1").unwrap().is_empty());
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
    migrations::ensure_schema_migrations_table(&conn).unwrap();
    migrations::run_migration_1(&conn).unwrap();
    conn.execute(
        "INSERT INTO schema_migrations(revision, name) VALUES (1, 'legacy source')",
        [],
    )
    .unwrap();
    drop(conn);

    let result = replace_database_file(&active, &source).unwrap();
    assert!(result.backup_path.exists());
    assert_eq!(result.schema_revision, LATEST_SCHEMA_REVISION);
    assert!(!source.exists());
    let conn = init_db(&active).unwrap();
    assert_schema_rev(&conn, LATEST_SCHEMA_REVISION);
}

#[test]
fn round_robin_counter_survives_db_reopen() {
    let path = temp_db_path("rr.db");
    {
        let conn = init_db(&path).unwrap();
        set_last_bot_index(&conn, 7).unwrap();
        assert_eq!(get_last_bot_index(&conn).unwrap(), 7);
    }
    // Reopen the same DB file
    {
        let conn = init_db(&path).unwrap();
        assert_eq!(get_last_bot_index(&conn).unwrap(), 7);
    }
}

#[test]
fn processing_marker_creates_row() {
    let path = temp_db_path("crash_recovery_1.db");
    let conn = init_db(&path).unwrap();
    insert_processing_marker(&conn, "job1", "test.mkv").unwrap();
    let status: String = conn
        .query_row("SELECT status FROM jobs WHERE job_id = 'job1'", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(status, "processing");
}

#[test]
fn job_marker_does_not_replace_existing_job_or_cascade_children() {
    let path = temp_db_path("marker_no_replace.db");
    let mut conn = init_db(&path).unwrap();

    let job = NewJob::complete("job1", "complete.mkv");
    let segments = vec![NewSegment {
        segment_key: "video_0/video_0001.m4s".into(),
        file_id: "file-id".into(),
        bot_index: 0,
        file_size: 123,
        duration: Some(4.0),
        is_split: false,
        encryption_nonce: None,
    }];
    let parts = vec![NewSegmentPart {
        job_id: "job1".into(),
        segment_key: "video_0/video_0001.m4s".into(),
        part_index: 0,
        file_id: "part-id".into(),
        bot_index: 0,
        file_size: 50,
        encryption_nonce: None,
    }];
    save_job(&mut conn, &job, &[], &segments, &parts).unwrap();

    insert_job_marker(&conn, "job1", "retry.mkv", "queued").unwrap();

    let saved = get_job(&conn, "job1").unwrap().unwrap();
    assert_eq!(saved.filename, "complete.mkv");
    assert_eq!(saved.status, "complete");
    assert_eq!(get_segments_for_job(&conn, "job1").unwrap().len(), 1);
    assert_eq!(
        get_segment_parts(&conn, "job1", "video_0/video_0001.m4s")
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn get_stuck_processing_jobs_returns_stuck() {
    let path = temp_db_path("crash_recovery_2.db");
    let conn = init_db(&path).unwrap();
    conn.execute(
            "INSERT INTO jobs(job_id, filename, status, created_at) VALUES ('job1', 'test.mkv', 'processing', datetime('now'))",
            [],
        )
        .unwrap();
    let stuck = get_stuck_processing_jobs(&conn).unwrap();
    assert_eq!(stuck, vec!["job1".to_string()]);
}

#[test]
fn mark_job_as_failed_updates_status() {
    let path = temp_db_path("crash_recovery_3.db");
    let conn = init_db(&path).unwrap();
    conn.execute(
            "INSERT INTO jobs(job_id, filename, status, created_at) VALUES ('job1', 'test.mkv', 'processing', datetime('now'))",
            [],
        )
        .unwrap();
    let updated = mark_job_as_failed(&conn, "job1", "test error").unwrap();
    assert!(updated);
    let status: String = conn
        .query_row("SELECT status FROM jobs WHERE job_id = 'job1'", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(status, "error");
}

#[test]
fn foreign_keys_pragma_on_after_init() {
    let path = temp_db_path("fk_test.db");
    let conn = init_db(&path).unwrap();
    let fk_on: i64 = conn
        .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
        .unwrap();
    assert_eq!(fk_on, 1);
    drop(conn);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn insert_split_segment_is_split_true() {
    let path = temp_db_path("split_segment.db");
    let mut conn = init_db(&path).unwrap();

    let job = NewJob::complete("test_job_split", "test_split.mkv");
    let segments = vec![NewSegment {
        segment_key: "video_0/seg_001.ts".into(),
        file_id: "".into(),
        bot_index: 0,
        file_size: 1024,
        duration: Some(10.0),
        is_split: true,
        encryption_nonce: None,
    }];
    save_job(&mut conn, &job, &[], &segments, &[]).unwrap();

    // Verify via get_segment (SegmentLookup)
    let lookup = get_segment(&conn, "test_job_split", "video_0/seg_001.ts")
        .unwrap()
        .expect("split segment should exist");
    assert!(lookup.is_split, "split segment should have is_split=true");
    assert_eq!(
        lookup.file_id, "",
        "split segment should have empty file_id"
    );

    // Verify via get_segments_for_job (SegmentRow)
    let rows = get_segments_for_job(&conn, "test_job_split").unwrap();
    assert_eq!(rows.len(), 1);
    assert!(
        rows[0].is_split,
        "SegmentRow should also have is_split=true"
    );
    assert_eq!(rows[0].file_id, "");
}

#[test]
fn insert_normal_segment_is_split_false() {
    let path = temp_db_path("normal_segment.db");
    let mut conn = init_db(&path).unwrap();

    let job = NewJob::complete("test_job_normal", "test_normal.mkv");
    let segments = vec![NewSegment {
        segment_key: "video_0/seg_001.ts".into(),
        file_id: "some_file_id_123".into(),
        bot_index: 0,
        file_size: 2048,
        duration: Some(10.0),
        is_split: false,
        encryption_nonce: None,
    }];
    save_job(&mut conn, &job, &[], &segments, &[]).unwrap();

    // Verify via get_segment (SegmentLookup)
    let lookup = get_segment(&conn, "test_job_normal", "video_0/seg_001.ts")
        .unwrap()
        .expect("normal segment should exist");
    assert!(
        !lookup.is_split,
        "normal segment should have is_split=false"
    );
    assert_eq!(lookup.file_id, "some_file_id_123");

    // Verify via get_segments_for_job (SegmentRow)
    let rows = get_segments_for_job(&conn, "test_job_normal").unwrap();
    assert_eq!(rows.len(), 1);
    assert!(
        !rows[0].is_split,
        "SegmentRow should also have is_split=false"
    );
    assert_eq!(rows[0].file_id, "some_file_id_123");
}

#[test]
fn update_segment_file_id_replaces_stale_id() {
    let path = temp_db_path("update_fid.db");
    let mut conn = init_db(&path).unwrap();
    let job = NewJob {
        job_id: "test_update_fid".into(),
        filename: "movie.mkv".into(),
        duration: 10.0,
        file_size: 1000,
        video_codec: "h264".into(),
        video_width: 1920,
        video_height: 1080,
        status: "complete".into(),
        media_type: "Film".into(),
        series_name: String::new(),
        has_thumbnail: false,
        is_series: false,
        season_number: None,
        episode_number: None,
        part_number: None,
        source_path: None,
        source_bitrate: 0,
    };
    let segments = vec![NewSegment {
        segment_key: "video_0/video_0001.m4s".into(),
        file_id: "stale_file_id".into(),
        bot_index: 0,
        file_size: 500,
        duration: Some(4.0),
        is_split: false,
        encryption_nonce: None,
    }];
    save_job(&mut conn, &job, &[], &segments, &[]).unwrap();

    let updated = queries::update_segment_file_id(
        &conn,
        "test_update_fid",
        "video_0/video_0001.m4s",
        "fresh_file_id",
        1,
        Some("abcdefabcdefabcdefabcdef"),
    )
    .unwrap();
    assert!(updated);

    let lookup = queries::get_segment(&conn, "test_update_fid", "video_0/video_0001.m4s")
        .unwrap()
        .unwrap();
    assert_eq!(lookup.file_id, "fresh_file_id");
    assert_eq!(lookup.bot_index, 1);
    assert_eq!(
        lookup.encryption_nonce.as_deref(),
        Some("abcdefabcdefabcdefabcdef")
    );
}

#[test]
fn update_segment_part_file_id_replaces_stale_id() {
    let path = temp_db_path("update_part_fid.db");
    let mut conn = init_db(&path).unwrap();
    let job = NewJob {
        job_id: "test_update_part_fid".into(),
        filename: "movie.mkv".into(),
        duration: 10.0,
        file_size: 1000,
        video_codec: "h264".into(),
        video_width: 1920,
        video_height: 1080,
        status: "complete".into(),
        media_type: "Film".into(),
        series_name: String::new(),
        has_thumbnail: false,
        is_series: false,
        season_number: None,
        episode_number: None,
        part_number: None,
        source_path: None,
        source_bitrate: 0,
    };
    let segments = vec![NewSegment {
        segment_key: "video_0/video_0001.m4s".into(),
        file_id: String::new(),
        bot_index: 0,
        file_size: 1000,
        duration: Some(4.0),
        is_split: true,
        encryption_nonce: None,
    }];
    let parts = vec![NewSegmentPart {
        job_id: "test_update_part_fid".into(),
        segment_key: "video_0/video_0001.m4s".into(),
        part_index: 0,
        file_id: "stale_part_id".into(),
        bot_index: 0,
        file_size: 500,
        encryption_nonce: None,
    }];
    save_job(&mut conn, &job, &[], &segments, &parts).unwrap();

    let updated = queries::update_segment_part_file_id(
        &conn,
        "test_update_part_fid",
        "video_0/video_0001.m4s",
        0,
        "fresh_part_id",
        2,
        Some("abcdefabcdefabcdefabcdef"),
    )
    .unwrap();
    assert!(updated);

    let lookup =
        queries::get_segment_parts(&conn, "test_update_part_fid", "video_0/video_0001.m4s")
            .unwrap();
    assert_eq!(lookup[0].file_id, "fresh_part_id");
    assert_eq!(lookup[0].bot_index, 2);
    assert_eq!(
        lookup[0].encryption_nonce.as_deref(),
        Some("abcdefabcdefabcdefabcdef")
    );
}
