use anyhow::{bail, Result};
use rusqlite::{params, Connection};

pub(super) type MigrationFn = fn(&Connection) -> Result<()>;

pub(super) struct Migration {
    pub(super) revision: i64,
    pub(super) name: &'static str,
    pub(super) run: MigrationFn,
}

pub(super) const MIGRATIONS: &[Migration] = &[
    Migration {
        revision: 1,
        name: "create_base_schema",
        run: run_migration_1,
    },
    Migration {
        revision: 2,
        name: "add_track_dimensions_and_stream_index",
        run: run_migration_2,
    },
    Migration {
        revision: 3,
        name: "add_segment_duration",
        run: run_migration_3,
    },
    Migration {
        revision: 4,
        name: "add_media_metadata",
        run: run_migration_4,
    },
    Migration {
        revision: 5,
        name: "add_series_episode_metadata",
        run: run_migration_5,
    },
    Migration {
        revision: 6,
        name: "create_settings_and_bots_tables",
        run: run_migration_6,
    },
    Migration {
        revision: 7,
        name: "add_listing_performance_indexes",
        run: run_migration_7,
    },
    Migration {
        revision: 8,
        name: "enforce_data_constraints",
        run: run_migration_8,
    },
    Migration {
        revision: 9,
        name: "add_bot_index_segment_index",
        run: run_migration_9,
    },
    Migration {
        revision: 10,
        name: "add_segment_parts",
        run: run_migration_10,
    },
    Migration {
        revision: 11,
        name: "add_is_split_column",
        run: run_migration_11,
    },
    Migration {
        revision: 12,
        name: "add_job_listing_composite_indexes",
        run: run_migration_12,
    },
    Migration {
        revision: 13,
        name: "add_jobs_error_column",
        run: run_migration_13,
    },
    Migration {
        revision: 14,
        name: "add_jobs_source_path_column",
        run: run_migration_14,
    },
    Migration {
        revision: 15,
        name: "enforce_jobs_constraints",
        run: run_migration_15,
    },
    Migration {
        revision: 16,
        name: "enforce_segments_is_split_check",
        run: run_migration_16,
    },
    Migration {
        revision: 17,
        name: "add_data_model_metadata_tables",
        run: run_migration_17,
    },
    Migration {
        revision: 18,
        name: "add_normalized_media_columns",
        run: run_migration_18,
    },
    Migration {
        revision: 19,
        name: "add_db_sync_tables",
        run: run_migration_19,
    },
    Migration {
        revision: 20,
        name: "remove_public_hls_segment_duration",
        run: run_migration_20,
    },
    Migration {
        revision: 21,
        name: "add_external_metadata_tables",
        run: run_migration_21,
    },
    Migration {
        revision: 22,
        name: "add_playback_progress_table",
        run: run_migration_22,
    },
    Migration {
        revision: 23,
        name: "add_media_markers_tables",
        run: run_migration_23,
    },
    Migration {
        revision: 24,
        name: "add_episode_title_column",
        run: run_migration_24,
    },
    Migration {
        revision: 25,
        name: "add_marker_fingerprint_windows",
        run: run_migration_25,
    },
    Migration {
        revision: 26,
        name: "add_telegram_encryption_nonces",
        run: run_migration_26,
    },
    Migration {
        revision: 27,
        name: "add_jobs_source_bitrate",
        run: run_migration_27,
    },
];

// Public wrappers for test access
pub(crate) fn run_migration_1(conn: &Connection) -> Result<()> {
    migration_1_create_base_schema(conn)
}
pub(crate) fn run_migration_2(conn: &Connection) -> Result<()> {
    migration_2_add_track_dimensions_and_stream_index(conn)
}
pub(crate) fn run_migration_3(conn: &Connection) -> Result<()> {
    migration_3_add_segment_duration(conn)
}
pub(crate) fn run_migration_4(conn: &Connection) -> Result<()> {
    migration_4_add_media_metadata(conn)
}
pub(crate) fn run_migration_5(conn: &Connection) -> Result<()> {
    migration_5_add_series_episode_metadata(conn)
}
pub(crate) fn run_migration_6(conn: &Connection) -> Result<()> {
    migration_6_create_settings_and_bots_tables(conn)
}
pub(crate) fn run_migration_7(conn: &Connection) -> Result<()> {
    migration_7_add_listing_performance_indexes(conn)
}
pub(crate) fn run_migration_8(conn: &Connection) -> Result<()> {
    migration_8_enforce_data_constraints(conn)
}
pub(crate) fn run_migration_9(conn: &Connection) -> Result<()> {
    migration_9_add_bot_index_segment_index(conn)
}
pub(crate) fn run_migration_10(conn: &Connection) -> Result<()> {
    migration_10_add_segment_parts(conn)
}
pub(crate) fn run_migration_11(conn: &Connection) -> Result<()> {
    migration_11_add_is_split_column(conn)
}
pub(crate) fn run_migration_12(conn: &Connection) -> Result<()> {
    migration_12_add_job_listing_composite_indexes(conn)
}
pub(crate) fn run_migration_13(conn: &Connection) -> Result<()> {
    migration_13_add_jobs_error_column(conn)
}
pub(crate) fn run_migration_14(conn: &Connection) -> Result<()> {
    migration_14_add_jobs_source_path_column(conn)
}
pub(crate) fn run_migration_15(conn: &Connection) -> Result<()> {
    migration_15_enforce_jobs_constraints(conn)
}
pub(crate) fn run_migration_16(conn: &Connection) -> Result<()> {
    migration_16_enforce_segments_is_split_check(conn)
}
pub(crate) fn run_migration_17(conn: &Connection) -> Result<()> {
    migration_17_add_data_model_metadata_tables(conn)
}
pub(crate) fn run_migration_18(conn: &Connection) -> Result<()> {
    migration_18_add_normalized_media_columns(conn)
}
pub(crate) fn run_migration_19(conn: &Connection) -> Result<()> {
    migration_19_add_db_sync_tables(conn)
}
pub(crate) fn run_migration_20(conn: &Connection) -> Result<()> {
    migration_20_remove_public_hls_segment_duration(conn)
}
pub(crate) fn run_migration_21(conn: &Connection) -> Result<()> {
    migration_21_add_external_metadata_tables(conn)
}
pub(crate) fn run_migration_22(conn: &Connection) -> Result<()> {
    migration_22_add_playback_progress_table(conn)
}
pub(crate) fn run_migration_23(conn: &Connection) -> Result<()> {
    migration_23_add_media_markers_tables(conn)
}
pub(crate) fn run_migration_24(conn: &Connection) -> Result<()> {
    migration_24_add_episode_title_column(conn)
}
pub(crate) fn run_migration_25(conn: &Connection) -> Result<()> {
    migration_25_add_marker_fingerprint_windows(conn)
}
pub(crate) fn run_migration_26(conn: &Connection) -> Result<()> {
    migration_26_add_telegram_encryption_nonces(conn)
}
pub(crate) fn run_migration_27(conn: &Connection) -> Result<()> {
    migration_27_add_jobs_source_bitrate(conn)
}

fn table_sql_contains(conn: &Connection, table: &str, needle: &str) -> Result<bool> {
    let sql: Option<String> = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type='table' AND name=?1",
            rusqlite::params![table],
            |row| row.get(0),
        )
        .ok()
        .flatten();
    Ok(sql.is_some_and(|s| s.contains(needle)))
}

pub(super) fn ensure_schema_migrations_table(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
             revision INTEGER PRIMARY KEY,
             name TEXT NOT NULL,
             applied_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
         );",
    )?;
    Ok(())
}

pub(super) fn bootstrap_schema_migrations(conn: &Connection) -> Result<()> {
    if !has_app_tables(conn)? {
        ensure_schema_migrations_table(conn)?;
        return Ok(());
    }
    if std::env::var_os("THLS_BOOTSTRAP_FROM_LEGACY").is_none() {
        bail!(
            "database has THLS tables but no schema_migrations; set THLS_BOOTSTRAP_FROM_LEGACY=1 to bootstrap a legacy database"
        );
    }
    let rev = detect_legacy_revision(conn)?;
    ensure_schema_migrations_table(conn)?;
    for migration in MIGRATIONS.iter().filter(|m| m.revision <= rev) {
        conn.execute(
            "INSERT OR IGNORE INTO schema_migrations(revision, name) VALUES (?1, ?2)",
            params![migration.revision, migration.name],
        )?;
    }
    Ok(())
}

pub(super) fn apply_migrations(conn: &mut Connection, current: i64) -> Result<()> {
    use anyhow::Context;
    let mut applied = 0;
    for migration in MIGRATIONS.iter().filter(|m| m.revision > current) {
        if matches!(migration.revision, 8 | 15 | 16) {
            conn.execute_batch("PRAGMA foreign_keys = OFF;")?;
        }
        let result = (|| -> Result<()> {
            let tx = conn.transaction()?;
            (migration.run)(&tx).with_context(|| {
                format!(
                    "running migration {} ({})",
                    migration.revision, migration.name
                )
            })?;
            tx.execute(
                "INSERT OR IGNORE INTO schema_migrations(revision, name) VALUES (?1, ?2)",
                params![migration.revision, migration.name],
            )?;
            tx.commit()?;
            Ok(())
        })();
        if matches!(migration.revision, 8 | 15 | 16) {
            conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        }
        result?;
        tracing::info!(
            revision = migration.revision,
            name = migration.name,
            "applied migration"
        );
        applied += 1;
    }
    if applied == 0 {
        tracing::info!(revision = current, "db schema up to date");
    }
    Ok(())
}

// --- Schema helpers ---

pub(crate) fn table_exists(conn: &Connection, table: &str) -> Result<bool> {
    let exists: i64 = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
        params![table],
        |row| row.get(0),
    )?;
    Ok(exists == 1)
}

pub(crate) fn index_exists(conn: &Connection, index: &str) -> Result<bool> {
    let exists: i64 = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='index' AND name=?1)",
        params![index],
        |row| row.get(0),
    )?;
    Ok(exists == 1)
}

pub(crate) fn column_exists(conn: &Connection, table: &str, column: &str) -> Result<bool> {
    let table = validate_table_name(table)?;
    let column = validate_column_name(column)?;
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(1))?;
    for row in rows {
        if row? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

fn add_column_if_missing(
    conn: &Connection,
    table: &str,
    column: &str,
    definition: &str,
) -> Result<()> {
    if !column_exists(conn, table, column)? {
        let table = validate_table_name(table)?;
        let column = validate_column_name(column)?;
        conn.execute_batch(&format!(
            "ALTER TABLE {table} ADD COLUMN {column} {definition};"
        ))?;
    }
    Ok(())
}

fn validate_table_name(table: &str) -> Result<&str> {
    match table {
        "jobs" | "tracks" | "segments" | "settings" | "bots" | "schema_migrations"
        | "segment_parts" | "kv_internal" | "db_sync_uploads" => Ok(table),
        _ => bail!("unknown sqlite table: {table}"),
    }
}

fn validate_column_name(column: &str) -> Result<&str> {
    match column {
        "id"
        | "revision"
        | "name"
        | "applied_at"
        | "job_id"
        | "filename"
        | "duration"
        | "file_size"
        | "video_codec"
        | "video_width"
        | "video_height"
        | "status"
        | "created_at"
        | "media_type"
        | "series_name"
        | "has_thumbnail"
        | "is_series"
        | "season_number"
        | "episode_number"
        | "part_number"
        | "track_type"
        | "track_index"
        | "codec"
        | "language"
        | "title"
        | "channels"
        | "width"
        | "height"
        | "bitrate"
        | "original_stream_index"
        | "segment_key"
        | "file_id"
        | "bot_index"
        | "key"
        | "value"
        | "updated_at"
        | "token"
        | "channel_id"
        | "label"
        | "part_index"
        | "is_split"
        | "error"
        | "source_path"
        | "enabled"
        | "source"
        | "value_type"
        | "bitrate_bps"
        | "mode"
        | "created_at_unix"
        | "prefix"
        | "episode_title"
        | "source_bitrate"
        | "encryption_nonce" => Ok(column),
        _ => bail!("unknown sqlite column: {column}"),
    }
}

fn has_app_tables(conn: &Connection) -> Result<bool> {
    Ok(table_exists(conn, "jobs")?
        || table_exists(conn, "tracks")?
        || table_exists(conn, "segments")?
        || table_exists(conn, "settings")?
        || table_exists(conn, "bots")?)
}

fn detect_legacy_revision(conn: &Connection) -> Result<i64> {
    if !(table_exists(conn, "jobs")?
        && table_exists(conn, "tracks")?
        && table_exists(conn, "segments")?)
    {
        bail!("database has partial THLS tables but no schema_migrations; refusing to bootstrap unknown schema");
    }
    let mut rev = 1;
    if column_exists(conn, "tracks", "width")?
        && column_exists(conn, "tracks", "height")?
        && column_exists(conn, "tracks", "bitrate")?
        && column_exists(conn, "tracks", "original_stream_index")?
    {
        rev = 2;
    } else {
        return Ok(rev);
    }
    if column_exists(conn, "segments", "duration")? {
        rev = 3;
    } else {
        return Ok(rev);
    }
    if column_exists(conn, "jobs", "media_type")?
        && column_exists(conn, "jobs", "series_name")?
        && column_exists(conn, "jobs", "has_thumbnail")?
    {
        rev = 4;
    } else {
        return Ok(rev);
    }
    if column_exists(conn, "jobs", "is_series")?
        && column_exists(conn, "jobs", "season_number")?
        && column_exists(conn, "jobs", "episode_number")?
        && column_exists(conn, "jobs", "part_number")?
    {
        rev = 5;
    } else {
        return Ok(rev);
    }
    if table_exists(conn, "settings")? && table_exists(conn, "bots")? {
        rev = 6;
    } else {
        return Ok(rev);
    }
    if index_exists(conn, "idx_tracks_job_type")? && index_exists(conn, "idx_jobs_series")? {
        rev = 7;
    } else {
        return Ok(rev);
    }
    if index_exists(conn, "idx_jobs_media_type")? && index_exists(conn, "idx_jobs_created_at")? {
        rev = 8;
    } else {
        return Ok(rev);
    }
    if index_exists(conn, "idx_segments_bot_index")? {
        rev = 9;
    } else {
        return Ok(rev);
    }
    if table_exists(conn, "segment_parts")? {
        rev = 10;
    } else {
        return Ok(rev);
    }
    if column_exists(conn, "segments", "is_split")? {
        rev = 11;
    } else {
        return Ok(rev);
    }
    if index_exists(conn, "idx_jobs_media_created")?
        && index_exists(conn, "idx_jobs_series_created")?
    {
        rev = 12;
    } else {
        return Ok(rev);
    }
    if column_exists(conn, "jobs", "error")? {
        rev = 13;
    } else {
        return Ok(rev);
    }
    if column_exists(conn, "jobs", "source_path")? {
        rev = 14;
    } else {
        return Ok(rev);
    }
    if table_sql_contains(conn, "jobs", "status IN")? {
        rev = 15;
    } else {
        return Ok(rev);
    }
    if table_sql_contains(conn, "segments", "is_split IN")? {
        rev = 16;
    } else {
        return Ok(rev);
    }
    if table_exists(conn, "kv_internal")?
        && column_exists(conn, "bots", "enabled")?
        && column_exists(conn, "bots", "source")?
        && column_exists(conn, "settings", "value_type")?
        && table_sql_contains(conn, "segment_parts", "part_index >= 0")?
    {
        rev = 17;
    } else {
        return Ok(rev);
    }
    if column_exists(conn, "tracks", "bitrate_bps")?
        && column_exists(conn, "tracks", "mode")?
        && column_exists(conn, "jobs", "created_at_unix")?
        && column_exists(conn, "segments", "prefix")?
        && column_exists(conn, "segments", "name")?
        && column_exists(conn, "segment_parts", "prefix")?
        && column_exists(conn, "segment_parts", "name")?
    {
        rev = 18;
    }
    Ok(rev)
}

// --- Individual migration functions ---

fn migration_1_create_base_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS jobs (
            job_id TEXT PRIMARY KEY,
            filename TEXT NOT NULL,
            duration REAL DEFAULT 0,
            file_size INTEGER DEFAULT 0,
            video_codec TEXT NOT NULL DEFAULT 'unknown',
            video_width INTEGER DEFAULT 0,
            video_height INTEGER DEFAULT 0,
            status TEXT DEFAULT 'complete',
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
        );
        CREATE TABLE IF NOT EXISTS tracks (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            job_id TEXT NOT NULL REFERENCES jobs(job_id) ON DELETE CASCADE,
            track_type TEXT NOT NULL,
            track_index INTEGER NOT NULL,
            codec TEXT NOT NULL DEFAULT 'unknown',
            language TEXT DEFAULT 'und',
            title TEXT DEFAULT '',
            channels INTEGER DEFAULT 2,
            UNIQUE (job_id, track_type, track_index)
        );
        CREATE TABLE IF NOT EXISTS segments (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            job_id TEXT NOT NULL REFERENCES jobs(job_id) ON DELETE CASCADE,
            segment_key TEXT NOT NULL,
            file_id TEXT NOT NULL,
            bot_index INTEGER NOT NULL,
            file_size INTEGER DEFAULT 0,
            UNIQUE (job_id, segment_key)
        );
        CREATE INDEX IF NOT EXISTS idx_segments_job ON segments(job_id);
        CREATE INDEX IF NOT EXISTS idx_tracks_job ON tracks(job_id);",
    )?;
    Ok(())
}

fn migration_2_add_track_dimensions_and_stream_index(conn: &Connection) -> Result<()> {
    add_column_if_missing(conn, "tracks", "width", "INTEGER DEFAULT 0")?;
    add_column_if_missing(conn, "tracks", "height", "INTEGER DEFAULT 0")?;
    add_column_if_missing(conn, "tracks", "bitrate", "TEXT NOT NULL DEFAULT '0'")?;
    add_column_if_missing(
        conn,
        "tracks",
        "original_stream_index",
        "INTEGER NOT NULL DEFAULT -1",
    )?;
    Ok(())
}

fn migration_3_add_segment_duration(conn: &Connection) -> Result<()> {
    add_column_if_missing(conn, "segments", "duration", "REAL DEFAULT NULL")?;
    Ok(())
}

fn migration_4_add_media_metadata(conn: &Connection) -> Result<()> {
    add_column_if_missing(conn, "jobs", "media_type", "TEXT NOT NULL DEFAULT 'Film'")?;
    add_column_if_missing(conn, "jobs", "series_name", "TEXT DEFAULT ''")?;
    add_column_if_missing(conn, "jobs", "has_thumbnail", "INTEGER NOT NULL DEFAULT 0")?;
    Ok(())
}

fn migration_5_add_series_episode_metadata(conn: &Connection) -> Result<()> {
    add_column_if_missing(conn, "jobs", "is_series", "INTEGER NOT NULL DEFAULT 0")?;
    add_column_if_missing(conn, "jobs", "season_number", "INTEGER DEFAULT NULL")?;
    add_column_if_missing(conn, "jobs", "episode_number", "INTEGER DEFAULT NULL")?;
    add_column_if_missing(conn, "jobs", "part_number", "INTEGER DEFAULT NULL")?;
    Ok(())
}

fn migration_6_create_settings_and_bots_tables(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS settings (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL,
            updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
        );
        CREATE TABLE IF NOT EXISTS bots (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            token TEXT NOT NULL UNIQUE,
            channel_id INTEGER NOT NULL,
            label TEXT DEFAULT '',
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
        );",
    )?;
    Ok(())
}

fn migration_7_add_listing_performance_indexes(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_tracks_job_type ON tracks(job_id, track_type);
         CREATE INDEX IF NOT EXISTS idx_jobs_series ON jobs(series_name);",
    )?;
    Ok(())
}

fn migration_8_enforce_data_constraints(conn: &Connection) -> Result<()> {
    use crate::settings_registry;
    conn.execute_batch(
        "UPDATE jobs SET
            duration = COALESCE(duration, 0),
            file_size = COALESCE(file_size, 0),
            video_codec = COALESCE(NULLIF(video_codec, ''), 'unknown'),
            video_width = COALESCE(video_width, 0),
            video_height = COALESCE(video_height, 0),
            status = COALESCE(NULLIF(status, ''), 'complete'),
            media_type = CASE WHEN media_type IN ('Film','Series','Anime Film','Anime TV','Anime') THEN media_type ELSE 'Film' END,
            series_name = COALESCE(series_name, ''),
            has_thumbnail = CASE WHEN has_thumbnail = 1 THEN 1 ELSE 0 END,
            is_series = CASE WHEN is_series = 1 THEN 1 ELSE 0 END;
         UPDATE jobs SET season_number = NULL, episode_number = NULL, part_number = NULL WHERE is_series != 1;
         UPDATE tracks SET
            track_type = CASE WHEN track_type IN ('video','audio','subtitle') THEN track_type ELSE 'video' END,
            codec = COALESCE(NULLIF(codec, ''), 'unknown'),
            language = COALESCE(NULLIF(language, ''), 'und'),
            title = COALESCE(title, ''),
            channels = COALESCE(channels, 2),
            width = COALESCE(width, 0),
            height = COALESCE(height, 0),
            bitrate = COALESCE(NULLIF(bitrate, ''), '0'),
            original_stream_index = COALESCE(original_stream_index, -1);
         UPDATE segments SET
            segment_key = CASE WHEN instr(segment_key, '/') > 0 THEN segment_key ELSE 'legacy/' || segment_key END,
            file_id = COALESCE(NULLIF(file_id, ''), 'unknown'),
            bot_index = CASE WHEN bot_index >= 0 THEN bot_index ELSE 0 END,
            file_size = COALESCE(file_size, 0);
         UPDATE bots SET label = COALESCE(label, '');",
    )?;

    rebuild_jobs_table(conn)?;
    rebuild_tracks_table(conn)?;
    rebuild_segments_table(conn)?;
    rebuild_bots_table(conn)?;

    let mut unknown = Vec::new();
    {
        let mut stmt =
            conn.prepare("SELECT key FROM settings WHERE key NOT LIKE '\\_%' ESCAPE '\\'")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        for row in rows {
            let key = row?;
            if !settings_registry::is_public_setting_key(&key) {
                unknown.push(key);
            }
        }
    }
    for key in unknown {
        conn.execute("DELETE FROM settings WHERE key = ?1", params![key])?;
    }
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_segments_job ON segments(job_id);
         CREATE INDEX IF NOT EXISTS idx_tracks_job ON tracks(job_id);
         CREATE INDEX IF NOT EXISTS idx_tracks_job_type ON tracks(job_id, track_type);
         CREATE INDEX IF NOT EXISTS idx_jobs_series ON jobs(series_name);
         CREATE INDEX IF NOT EXISTS idx_jobs_media_type ON jobs(media_type);
         CREATE INDEX IF NOT EXISTS idx_jobs_created_at ON jobs(created_at DESC);",
    )?;
    Ok(())
}

fn migration_9_add_bot_index_segment_index(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_segments_bot_index ON segments(bot_index);",
    )?;
    Ok(())
}

fn migration_10_add_segment_parts(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS segment_parts (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            job_id TEXT NOT NULL REFERENCES jobs(job_id) ON DELETE CASCADE,
            segment_key TEXT NOT NULL,
            part_index INTEGER NOT NULL,
            file_id TEXT NOT NULL,
            bot_index INTEGER NOT NULL CHECK (bot_index >= 0),
            file_size INTEGER NOT NULL,
            UNIQUE (job_id, segment_key, part_index)
        );
        CREATE INDEX IF NOT EXISTS idx_segment_parts_segment ON segment_parts(job_id, segment_key);",
    )?;
    Ok(())
}

fn migration_11_add_is_split_column(conn: &Connection) -> Result<()> {
    add_column_if_missing(conn, "segments", "is_split", "INTEGER NOT NULL DEFAULT 0")?;
    conn.execute_batch("UPDATE segments SET is_split = 1 WHERE file_id = 'split';")?;
    Ok(())
}

fn migration_12_add_job_listing_composite_indexes(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_jobs_media_created ON jobs(media_type, created_at DESC);
         CREATE INDEX IF NOT EXISTS idx_jobs_series_created ON jobs(series_name, created_at DESC);",
    )?;
    Ok(())
}

fn migration_16_enforce_segments_is_split_check(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "DROP TABLE IF EXISTS segments_new;
         CREATE TABLE segments_new (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            job_id TEXT NOT NULL REFERENCES jobs(job_id) ON DELETE CASCADE,
            segment_key TEXT NOT NULL CHECK (segment_key GLOB '*/*'),
            file_id TEXT NOT NULL,
            bot_index INTEGER NOT NULL CHECK (bot_index >= 0),
            file_size INTEGER DEFAULT 0,
            duration REAL DEFAULT NULL,
            is_split INTEGER NOT NULL DEFAULT 0 CHECK (is_split IN (0,1)),
            UNIQUE (job_id, segment_key)
         );
         INSERT OR IGNORE INTO segments_new(id, job_id, segment_key, file_id, bot_index, file_size, duration, is_split)
         SELECT id, job_id, segment_key, file_id, bot_index, file_size, duration, is_split FROM segments;
         DROP TABLE segments;
         ALTER TABLE segments_new RENAME TO segments;
         CREATE INDEX IF NOT EXISTS idx_segments_job ON segments(job_id);
         CREATE INDEX IF NOT EXISTS idx_segments_bot_index ON segments(bot_index);",
    )?;
    Ok(())
}

fn migration_13_add_jobs_error_column(conn: &Connection) -> Result<()> {
    add_column_if_missing(conn, "jobs", "error", "TEXT")
}

fn migration_14_add_jobs_source_path_column(conn: &Connection) -> Result<()> {
    add_column_if_missing(conn, "jobs", "source_path", "TEXT DEFAULT NULL")?;
    migration_12_add_job_listing_composite_indexes(conn)
}

fn migration_15_enforce_jobs_constraints(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "UPDATE jobs SET status = 'complete'
           WHERE status IS NULL
              OR status NOT IN ('queued','downloading','analyzing','processing','uploading','complete','error','cancelled');
         UPDATE jobs SET error = NULL WHERE status != 'error';
         UPDATE jobs SET error = 'unknown error' WHERE status = 'error' AND error IS NULL;
         UPDATE jobs SET source_path = NULL
           WHERE source_path IS NOT NULL
             AND (source_path GLOB '*[/\\\\]*' OR source_path LIKE '%..%');",
    )?;
    conn.execute_batch(
        "DROP TABLE IF EXISTS jobs_new;
         CREATE TABLE jobs_new (
            job_id TEXT PRIMARY KEY,
            filename TEXT NOT NULL,
            duration REAL DEFAULT 0,
            file_size INTEGER DEFAULT 0,
            video_codec TEXT NOT NULL DEFAULT 'unknown',
            video_width INTEGER DEFAULT 0,
            video_height INTEGER DEFAULT 0,
            status TEXT NOT NULL DEFAULT 'complete' CHECK (status IN ('queued','downloading','analyzing','processing','uploading','complete','error','cancelled')),
            error TEXT DEFAULT NULL,
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
            media_type TEXT NOT NULL DEFAULT 'Film' CHECK (media_type IN ('Film','Series','Anime Film','Anime TV','Anime')),
            series_name TEXT DEFAULT '',
            has_thumbnail INTEGER NOT NULL DEFAULT 0 CHECK (has_thumbnail IN (0,1)),
            is_series INTEGER NOT NULL DEFAULT 0 CHECK (is_series IN (0,1)),
            season_number INTEGER DEFAULT NULL,
            episode_number INTEGER DEFAULT NULL,
            part_number INTEGER DEFAULT NULL,
            source_path TEXT DEFAULT NULL CHECK (source_path IS NULL OR (source_path NOT GLOB '*[/\\\\]*' AND source_path NOT LIKE '%..%')),
            CHECK (is_series = 1 OR (season_number IS NULL AND episode_number IS NULL AND part_number IS NULL)),
            CHECK ((status='error') = (error IS NOT NULL))
         );
         INSERT INTO jobs_new(job_id, filename, duration, file_size, video_codec, video_width, video_height, status, error, created_at, media_type, series_name, has_thumbnail, is_series, season_number, episode_number, part_number, source_path)
         SELECT job_id, filename, duration, file_size, video_codec, video_width, video_height, status, error, created_at, media_type, series_name, has_thumbnail, is_series, season_number, episode_number, part_number, source_path FROM jobs;
         DROP TABLE jobs;
         ALTER TABLE jobs_new RENAME TO jobs;
         CREATE INDEX IF NOT EXISTS idx_jobs_series ON jobs(series_name);
         CREATE INDEX IF NOT EXISTS idx_jobs_media_type ON jobs(media_type);
         CREATE INDEX IF NOT EXISTS idx_jobs_created_at ON jobs(created_at DESC);
         CREATE INDEX IF NOT EXISTS idx_jobs_media_created ON jobs(media_type, created_at DESC);
         CREATE INDEX IF NOT EXISTS idx_jobs_series_created ON jobs(series_name, created_at DESC);",
    )?;
    Ok(())
}

fn rebuild_jobs_table(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "DROP TABLE IF EXISTS jobs_new;
         CREATE TABLE jobs_new (
            job_id TEXT PRIMARY KEY,
            filename TEXT NOT NULL,
            duration REAL DEFAULT 0,
            file_size INTEGER DEFAULT 0,
            video_codec TEXT NOT NULL DEFAULT 'unknown',
            video_width INTEGER DEFAULT 0,
            video_height INTEGER DEFAULT 0,
            status TEXT DEFAULT 'complete',
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
            media_type TEXT NOT NULL DEFAULT 'Film' CHECK (media_type IN ('Film','Series','Anime Film','Anime TV','Anime')),
            series_name TEXT DEFAULT '',
            has_thumbnail INTEGER NOT NULL DEFAULT 0 CHECK (has_thumbnail IN (0,1)),
            is_series INTEGER NOT NULL DEFAULT 0 CHECK (is_series IN (0,1)),
            season_number INTEGER DEFAULT NULL,
            episode_number INTEGER DEFAULT NULL,
            part_number INTEGER DEFAULT NULL,
            CHECK (is_series = 1 OR (season_number IS NULL AND episode_number IS NULL AND part_number IS NULL))
         );
         INSERT INTO jobs_new(job_id, filename, duration, file_size, video_codec, video_width, video_height, status, created_at, media_type, series_name, has_thumbnail, is_series, season_number, episode_number, part_number)
         SELECT job_id, filename, duration, file_size, video_codec, video_width, video_height, status, created_at, media_type, series_name, has_thumbnail, is_series, season_number, episode_number, part_number FROM jobs;
         DROP TABLE jobs;
         ALTER TABLE jobs_new RENAME TO jobs;",
    )?;
    Ok(())
}

fn rebuild_tracks_table(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "DROP TABLE IF EXISTS tracks_new;
         CREATE TABLE tracks_new (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            job_id TEXT NOT NULL REFERENCES jobs(job_id) ON DELETE CASCADE,
            track_type TEXT NOT NULL CHECK (track_type IN ('video','audio','subtitle')),
            track_index INTEGER NOT NULL,
            codec TEXT NOT NULL DEFAULT 'unknown',
            language TEXT DEFAULT 'und',
            title TEXT DEFAULT '',
            channels INTEGER DEFAULT 2,
            width INTEGER DEFAULT 0,
            height INTEGER DEFAULT 0,
            bitrate TEXT NOT NULL DEFAULT '0',
            original_stream_index INTEGER NOT NULL DEFAULT -1 CHECK (original_stream_index >= -1),
            UNIQUE (job_id, track_type, track_index)
         );
         INSERT OR IGNORE INTO tracks_new(id, job_id, track_type, track_index, codec, language, title, channels, width, height, bitrate, original_stream_index)
         SELECT id, job_id, track_type, track_index, codec, language, title, channels, width, height, bitrate, original_stream_index FROM tracks;
         DROP TABLE tracks;
         ALTER TABLE tracks_new RENAME TO tracks;",
    )?;
    Ok(())
}

fn rebuild_segments_table(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "DROP TABLE IF EXISTS segments_new;
         CREATE TABLE segments_new (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            job_id TEXT NOT NULL REFERENCES jobs(job_id) ON DELETE CASCADE,
            segment_key TEXT NOT NULL CHECK (segment_key GLOB '*/*'),
            file_id TEXT NOT NULL,
            bot_index INTEGER NOT NULL CHECK (bot_index >= 0),
            file_size INTEGER DEFAULT 0,
            duration REAL DEFAULT NULL,
            UNIQUE (job_id, segment_key)
         );
         INSERT OR IGNORE INTO segments_new(id, job_id, segment_key, file_id, bot_index, file_size, duration)
         SELECT id, job_id, segment_key, file_id, bot_index, file_size, duration FROM segments;
         DROP TABLE segments;
         ALTER TABLE segments_new RENAME TO segments;",
    )?;
    Ok(())
}

fn rebuild_bots_table(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "DROP TABLE IF EXISTS bots_new;
         CREATE TABLE bots_new (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            token TEXT NOT NULL UNIQUE,
            channel_id INTEGER NOT NULL CHECK (channel_id < 0),
            label TEXT DEFAULT '',
            enabled INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0,1)),
            source TEXT NOT NULL DEFAULT 'db' CHECK (source IN ('env','db')),
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
         );
          INSERT OR IGNORE INTO bots_new(id, token, channel_id, label, enabled, source, created_at)
          SELECT id, token, channel_id, label, 1, 'db', created_at FROM bots WHERE channel_id < 0 AND token != '';
          DROP TABLE bots;
          ALTER TABLE bots_new RENAME TO bots;",
    )?;
    Ok(())
}

fn migration_17_add_data_model_metadata_tables(conn: &Connection) -> Result<()> {
    use crate::settings_registry::{setting_type_name, SETTINGS};

    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS kv_internal (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL,
            updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
         );
         INSERT OR IGNORE INTO kv_internal(key, value)
         SELECT key, value FROM settings WHERE key = '_last_bot_index';
         DELETE FROM settings WHERE key LIKE '\\_%' ESCAPE '\\';",
    )?;

    if !column_exists(conn, "settings", "value_type")? {
        conn.execute_batch(
            "ALTER TABLE settings ADD COLUMN value_type TEXT NOT NULL DEFAULT 'str'
             CHECK (value_type IN ('int','bool','str','list','tiers'));",
        )?;
    }
    for spec in SETTINGS {
        conn.execute(
            "UPDATE settings SET value_type = ?2 WHERE key = ?1",
            params![spec.key, setting_type_name(spec.setting_type)],
        )?;
    }
    conn.execute(
        "INSERT INTO kv_internal(key, value) VALUES ('settings_schema_version', '1')
         ON CONFLICT(key) DO UPDATE SET value='1', updated_at=CURRENT_TIMESTAMP",
        [],
    )?;

    add_column_if_missing(
        conn,
        "bots",
        "enabled",
        "INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0,1))",
    )?;
    add_column_if_missing(
        conn,
        "bots",
        "source",
        "TEXT NOT NULL DEFAULT 'db' CHECK (source IN ('env','db'))",
    )?;
    conn.execute_batch(
        "UPDATE bots SET enabled = CASE WHEN enabled = 0 THEN 0 ELSE 1 END;
         UPDATE bots SET source = 'db' WHERE source NOT IN ('env','db');",
    )?;

    conn.execute_batch(
        "DROP TABLE IF EXISTS segment_parts_new;
         CREATE TABLE segment_parts_new (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            job_id TEXT NOT NULL REFERENCES jobs(job_id) ON DELETE CASCADE,
            segment_key TEXT NOT NULL CHECK (segment_key GLOB '*/*'),
            part_index INTEGER NOT NULL CHECK (part_index >= 0),
            file_id TEXT NOT NULL,
            bot_index INTEGER NOT NULL CHECK (bot_index >= 0),
            file_size INTEGER NOT NULL CHECK (file_size >= 0),
            UNIQUE (job_id, segment_key, part_index)
         );
         INSERT OR IGNORE INTO segment_parts_new(id, job_id, segment_key, part_index, file_id, bot_index, file_size)
         SELECT id, job_id, segment_key, part_index, file_id, bot_index, file_size
           FROM segment_parts
          WHERE instr(segment_key, '/') > 0 AND part_index >= 0 AND bot_index >= 0 AND file_size >= 0;
         DROP TABLE segment_parts;
         ALTER TABLE segment_parts_new RENAME TO segment_parts;
         CREATE INDEX IF NOT EXISTS idx_segment_parts_segment ON segment_parts(job_id, segment_key);",
    )?;
    Ok(())
}

fn migration_18_add_normalized_media_columns(conn: &Connection) -> Result<()> {
    add_column_if_missing(
        conn,
        "tracks",
        "mode",
        "TEXT NOT NULL DEFAULT 'encode' CHECK (mode IN ('encode','copy'))",
    )?;
    add_column_if_missing(
        conn,
        "tracks",
        "bitrate_bps",
        "INTEGER NOT NULL DEFAULT 0 CHECK (bitrate_bps >= 0)",
    )?;
    let mut tracks = Vec::new();
    {
        let mut stmt = conn.prepare("SELECT id, bitrate FROM tracks")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
        for row in rows {
            tracks.push(row?);
        }
    }
    for (id, bitrate) in tracks {
        let (mode, bps) = track_mode_and_bps(&bitrate);
        conn.execute(
            "UPDATE tracks SET mode = ?2, bitrate_bps = ?3 WHERE id = ?1",
            params![id, mode, bps],
        )?;
    }

    add_column_if_missing(conn, "jobs", "created_at_unix", "INTEGER")?;
    conn.execute_batch(
        "UPDATE jobs
            SET created_at_unix = COALESCE(
                created_at_unix,
                unixepoch(created_at),
                unixepoch(replace(created_at, 'T', ' ')),
                unixepoch()
            );
         CREATE INDEX IF NOT EXISTS idx_jobs_created_at_unix ON jobs(created_at_unix DESC);",
    )?;

    add_column_if_missing(conn, "segments", "prefix", "TEXT")?;
    add_column_if_missing(conn, "segments", "name", "TEXT")?;
    add_column_if_missing(conn, "segment_parts", "prefix", "TEXT")?;
    add_column_if_missing(conn, "segment_parts", "name", "TEXT")?;
    backfill_segment_key_parts(conn, "segments")?;
    backfill_segment_key_parts(conn, "segment_parts")?;
    conn.execute_batch(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_segments_job_prefix_name
            ON segments(job_id, prefix, name);
         CREATE INDEX IF NOT EXISTS idx_segments_job_prefix
            ON segments(job_id, prefix);
         CREATE UNIQUE INDEX IF NOT EXISTS idx_segment_parts_job_prefix_name_part
            ON segment_parts(job_id, prefix, name, part_index);
         CREATE INDEX IF NOT EXISTS idx_segment_parts_job_prefix
            ON segment_parts(job_id, prefix);",
    )?;
    Ok(())
}

fn migration_19_add_db_sync_tables(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS db_sync_snapshots (
            id TEXT PRIMARY KEY,
            created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
            schema_revision INTEGER NOT NULL,
            size_bytes INTEGER NOT NULL CHECK (size_bytes >= 0),
            status TEXT NOT NULL CHECK (status IN ('pending','complete','partial','failed')),
            last_error TEXT
         );
         CREATE TABLE IF NOT EXISTS db_sync_uploads (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            snapshot_id TEXT NOT NULL REFERENCES db_sync_snapshots(id) ON DELETE CASCADE,
            bot_index INTEGER NOT NULL CHECK (bot_index >= 0),
            part_index INTEGER NOT NULL CHECK (part_index >= 0),
            file_id TEXT NOT NULL,
            file_size INTEGER NOT NULL CHECK (file_size >= 0),
            uploaded_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
            status TEXT NOT NULL CHECK (status IN ('complete','failed')),
            error TEXT,
            UNIQUE(snapshot_id, bot_index, part_index)
         );
         CREATE INDEX IF NOT EXISTS idx_db_sync_uploads_snapshot ON db_sync_uploads(snapshot_id);
         CREATE INDEX IF NOT EXISTS idx_db_sync_snapshots_created ON db_sync_snapshots(created_at DESC);",
    )?;
    Ok(())
}

fn migration_20_remove_public_hls_segment_duration(conn: &Connection) -> Result<()> {
    conn.execute(
        "DELETE FROM settings WHERE key = 'HLS_SEGMENT_DURATION'",
        [],
    )?;
    Ok(())
}

fn migration_21_add_external_metadata_tables(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS external_metadata (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            provider TEXT NOT NULL CHECK (provider IN ('tmdb','anilist','mal')),
            provider_id TEXT NOT NULL,
            media_kind TEXT NOT NULL CHECK (media_kind IN ('movie','tv','anime','manga')),
            title TEXT NOT NULL DEFAULT '',
            original_title TEXT NOT NULL DEFAULT '',
            overview TEXT NOT NULL DEFAULT '',
            poster_url TEXT NOT NULL DEFAULT '',
            backdrop_url TEXT NOT NULL DEFAULT '',
            release_date TEXT NOT NULL DEFAULT '',
            year INTEGER,
            rating REAL,
            raw_json TEXT NOT NULL DEFAULT '{}',
            fetched_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
            UNIQUE(provider, provider_id, media_kind)
        );
        CREATE TABLE IF NOT EXISTS job_metadata_links (
            job_id TEXT NOT NULL REFERENCES jobs(job_id) ON DELETE CASCADE,
            metadata_id INTEGER NOT NULL REFERENCES external_metadata(id) ON DELETE CASCADE,
            role TEXT NOT NULL CHECK (role IN ('primary','episode')),
            created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
            PRIMARY KEY(job_id, role)
        );
        CREATE TABLE IF NOT EXISTS series_metadata_links (
            media_type TEXT NOT NULL,
            series_name TEXT NOT NULL,
            metadata_id INTEGER NOT NULL REFERENCES external_metadata(id) ON DELETE CASCADE,
            created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
            PRIMARY KEY(media_type, series_name)
        );",
    )?;
    Ok(())
}

fn migration_22_add_playback_progress_table(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS playback_progress (
            client_id TEXT NOT NULL,
            job_id TEXT NOT NULL REFERENCES jobs(job_id) ON DELETE CASCADE,
            position_seconds REAL NOT NULL CHECK (position_seconds >= 0),
            duration_seconds REAL NOT NULL CHECK (duration_seconds >= 0),
            progress_pct INTEGER NOT NULL CHECK (progress_pct >= 0 AND progress_pct <= 100),
            completed INTEGER NOT NULL DEFAULT 0 CHECK (completed IN (0,1)),
            updated_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
            PRIMARY KEY(client_id, job_id)
        );
        CREATE INDEX IF NOT EXISTS idx_playback_progress_client_updated
        ON playback_progress(client_id, updated_at DESC);
        CREATE INDEX IF NOT EXISTS idx_playback_progress_job
        ON playback_progress(job_id);",
    )?;
    Ok(())
}

fn migration_23_add_media_markers_tables(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS media_markers (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            job_id TEXT NOT NULL REFERENCES jobs(job_id) ON DELETE CASCADE,
            marker_type TEXT NOT NULL CHECK (marker_type IN ('intro','outro','recap','preview','credits')),
            start_seconds REAL NOT NULL CHECK (start_seconds >= 0),
            end_seconds REAL NOT NULL CHECK (end_seconds > start_seconds),
            source TEXT NOT NULL CHECK (source IN ('chapter','chromaprint','silence','manual')),
            confidence REAL NOT NULL DEFAULT 1.0 CHECK (confidence >= 0 AND confidence <= 1),
            enabled INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0,1)),
            created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
            updated_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
        );
        CREATE INDEX IF NOT EXISTS idx_media_markers_job_type
        ON media_markers(job_id, marker_type);
        CREATE TABLE IF NOT EXISTS media_fingerprints (
            job_id TEXT PRIMARY KEY REFERENCES jobs(job_id) ON DELETE CASCADE,
            media_type TEXT NOT NULL,
            series_name TEXT NOT NULL,
            season_number INTEGER,
            duration_seconds REAL NOT NULL,
            fingerprint TEXT NOT NULL,
            fingerprint_source TEXT NOT NULL DEFAULT 'chromaprint',
            created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
        );
        CREATE INDEX IF NOT EXISTS idx_media_fingerprints_series
        ON media_fingerprints(media_type, series_name, season_number);",
    )?;
    Ok(())
}

fn migration_24_add_episode_title_column(conn: &Connection) -> Result<()> {
    if !column_exists(conn, "jobs", "episode_title")? {
        conn.execute_batch("ALTER TABLE jobs ADD COLUMN episode_title TEXT DEFAULT NULL;")?;
    }
    Ok(())
}

fn migration_25_add_marker_fingerprint_windows(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "ALTER TABLE media_markers RENAME TO media_markers_old;
         CREATE TABLE media_markers (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            job_id TEXT NOT NULL REFERENCES jobs(job_id) ON DELETE CASCADE,
            marker_type TEXT NOT NULL CHECK (marker_type IN ('intro','outro','recap','preview','credits')),
            start_seconds REAL NOT NULL CHECK (start_seconds >= 0),
            end_seconds REAL NOT NULL CHECK (end_seconds > start_seconds),
            source TEXT NOT NULL CHECK (source IN ('chapter','chromaprint','silence','blackframe','manual')),
            confidence REAL NOT NULL DEFAULT 1.0 CHECK (confidence >= 0 AND confidence <= 1),
            enabled INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0,1)),
            created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
            updated_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
         );
         INSERT INTO media_markers(id, job_id, marker_type, start_seconds, end_seconds, source, confidence, enabled, created_at, updated_at)
            SELECT id, job_id, marker_type, start_seconds, end_seconds, source, confidence, enabled, created_at, updated_at
            FROM media_markers_old;
         DROP TABLE media_markers_old;
         CREATE INDEX IF NOT EXISTS idx_media_markers_job_type
            ON media_markers(job_id, marker_type);

         ALTER TABLE media_fingerprints RENAME TO media_fingerprints_old;
         CREATE TABLE media_fingerprints (
            job_id TEXT NOT NULL REFERENCES jobs(job_id) ON DELETE CASCADE,
            media_type TEXT NOT NULL,
            series_name TEXT NOT NULL,
            season_number INTEGER,
            window_type TEXT NOT NULL DEFAULT 'intro' CHECK (window_type IN ('intro','outro')),
            window_start_seconds REAL NOT NULL DEFAULT 0,
            window_duration_seconds REAL NOT NULL DEFAULT 0,
            duration_seconds REAL NOT NULL,
            fingerprint TEXT NOT NULL,
            fingerprint_source TEXT NOT NULL DEFAULT 'chromaprint',
            created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
            PRIMARY KEY(job_id, window_type)
         );
         INSERT OR IGNORE INTO media_fingerprints(
            job_id, media_type, series_name, season_number, window_type,
            window_start_seconds, window_duration_seconds, duration_seconds,
            fingerprint, fingerprint_source, created_at
         )
            SELECT job_id, media_type, series_name, season_number, 'intro',
                   0, duration_seconds, duration_seconds,
                   fingerprint, fingerprint_source, created_at
            FROM media_fingerprints_old;
         DROP TABLE media_fingerprints_old;
         CREATE INDEX IF NOT EXISTS idx_media_fingerprints_series
            ON media_fingerprints(media_type, series_name, season_number, window_type);",
    )?;
    Ok(())
}

fn migration_26_add_telegram_encryption_nonces(conn: &Connection) -> Result<()> {
    add_column_if_missing(conn, "segments", "encryption_nonce", "TEXT DEFAULT NULL")?;
    add_column_if_missing(
        conn,
        "segment_parts",
        "encryption_nonce",
        "TEXT DEFAULT NULL",
    )?;
    add_column_if_missing(
        conn,
        "db_sync_uploads",
        "encryption_nonce",
        "TEXT DEFAULT NULL",
    )?;
    Ok(())
}

fn migration_27_add_jobs_source_bitrate(conn: &Connection) -> Result<()> {
    add_column_if_missing(conn, "jobs", "source_bitrate", "INTEGER NOT NULL DEFAULT 0")
}

fn backfill_segment_key_parts(conn: &Connection, table: &str) -> Result<()> {
    let table = validate_table_name(table)?;
    conn.execute_batch(&format!(
        "UPDATE {table}
            SET prefix = substr(segment_key, 1, instr(segment_key, '/') - 1),
                name = substr(segment_key, instr(segment_key, '/') + 1)
          WHERE prefix IS NULL OR name IS NULL;"
    ))?;
    Ok(())
}

pub(crate) fn track_mode_and_bps(bitrate: &str) -> (&'static str, i64) {
    let value = bitrate.trim();
    if value.eq_ignore_ascii_case("copy") {
        return ("copy", 0);
    }
    ("encode", parse_bitrate_bps(value).unwrap_or(0))
}

fn parse_bitrate_bps(value: &str) -> Option<i64> {
    let lower = value.trim().to_ascii_lowercase();
    let (digits, multiplier) = if let Some(raw) = lower.strip_suffix("kbps") {
        (raw, 1_000)
    } else if let Some(raw) = lower.strip_suffix('k') {
        (raw, 1_000)
    } else if let Some(raw) = lower.strip_suffix("mbps") {
        (raw, 1_000_000)
    } else if let Some(raw) = lower.strip_suffix('m') {
        (raw, 1_000_000)
    } else if let Some(raw) = lower.strip_suffix("gbps") {
        (raw, 1_000_000_000)
    } else if let Some(raw) = lower.strip_suffix('g') {
        (raw, 1_000_000_000)
    } else if let Some(raw) = lower.strip_suffix("bps") {
        (raw, 1)
    } else {
        (lower.as_str(), 1)
    };
    digits.trim().parse::<i64>().ok()?.checked_mul(multiplier)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_migration_11_add_is_split() {
        let conn = Connection::open_in_memory().unwrap();

        run_migration_1(&conn).unwrap();
        run_migration_2(&conn).unwrap();
        run_migration_3(&conn).unwrap();
        run_migration_4(&conn).unwrap();
        run_migration_5(&conn).unwrap();
        run_migration_6(&conn).unwrap();
        run_migration_7(&conn).unwrap();
        run_migration_8(&conn).unwrap();
        run_migration_9(&conn).unwrap();
        run_migration_10(&conn).unwrap();

        conn.execute(
            "INSERT INTO jobs (job_id, filename) VALUES (?1, ?2)",
            rusqlite::params!["job1", "test.mkv"],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO jobs (job_id, filename) VALUES (?1, ?2)",
            rusqlite::params!["job2", "test2.mkv"],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO segments (job_id, segment_key, file_id, bot_index, file_size) VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params!["job1", "seg/1", "split", 0, 0],
        ).unwrap();
        conn.execute(
            "INSERT INTO segments (job_id, segment_key, file_id, bot_index, file_size) VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params!["job1", "seg/2", "normal.ts", 0, 100],
        ).unwrap();
        conn.execute(
            "INSERT INTO segments (job_id, segment_key, file_id, bot_index, file_size) VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params!["job2", "seg/3", "video.ts", 1, 200],
        ).unwrap();

        run_migration_11(&conn).unwrap();

        assert!(crate::db::migrations::column_exists(&conn, "segments", "is_split").unwrap());

        let split_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM segments WHERE is_split = 1 AND file_id = 'split'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(split_count, 1);

        let normal_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM segments WHERE is_split = 0 AND file_id != 'split'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(normal_count, 2);

        let mismatch: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM segments WHERE is_split = 1 AND file_id != 'split'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(mismatch, 0);
    }

    #[test]
    fn test_migration_12_adds_composite_listing_indexes() {
        let conn = Connection::open_in_memory().unwrap();

        run_migration_1(&conn).unwrap();
        run_migration_2(&conn).unwrap();
        run_migration_3(&conn).unwrap();
        run_migration_4(&conn).unwrap();
        run_migration_5(&conn).unwrap();
        run_migration_6(&conn).unwrap();
        run_migration_7(&conn).unwrap();
        run_migration_8(&conn).unwrap();
        run_migration_9(&conn).unwrap();
        run_migration_10(&conn).unwrap();
        run_migration_11(&conn).unwrap();

        run_migration_12(&conn).unwrap();

        assert!(index_exists(&conn, "idx_jobs_media_created").unwrap());
        assert!(index_exists(&conn, "idx_jobs_series_created").unwrap());
    }

    #[test]
    fn test_migration_26_adds_encryption_nonce_columns() {
        let conn = Connection::open_in_memory().unwrap();
        for migration in MIGRATIONS.iter().filter(|m| m.revision < 26) {
            (migration.run)(&conn).unwrap();
        }

        run_migration_26(&conn).unwrap();

        assert!(column_exists(&conn, "segments", "encryption_nonce").unwrap());
        assert!(column_exists(&conn, "segment_parts", "encryption_nonce").unwrap());
        assert!(column_exists(&conn, "db_sync_uploads", "encryption_nonce").unwrap());
    }

    #[test]
    fn test_migration_15_enforce_jobs_constraints() {
        let conn = Connection::open_in_memory().unwrap();
        run_migration_1(&conn).unwrap();
        run_migration_2(&conn).unwrap();
        run_migration_3(&conn).unwrap();
        run_migration_4(&conn).unwrap();
        run_migration_5(&conn).unwrap();
        run_migration_6(&conn).unwrap();
        run_migration_7(&conn).unwrap();
        run_migration_8(&conn).unwrap();
        run_migration_9(&conn).unwrap();
        run_migration_10(&conn).unwrap();
        run_migration_11(&conn).unwrap();
        run_migration_12(&conn).unwrap();
        run_migration_13(&conn).unwrap();
        run_migration_14(&conn).unwrap();

        // Seed a valid job
        conn.execute(
            "INSERT INTO jobs(job_id, filename, status) VALUES ('j1', 'f.mkv', 'complete')",
            [],
        )
        .unwrap();

        run_migration_15(&conn).unwrap();

        // Verify CHECK rejects invalid status
        assert!(conn
            .execute(
                "INSERT INTO jobs(job_id, filename, status) VALUES ('j2', 'f2.mkv', 'bogus')",
                [],
            )
            .is_err());

        // Verify CHECK rejects status='error' with NULL error
        assert!(conn.execute(
            "INSERT INTO jobs(job_id, filename, status, error) VALUES ('j3', 'f3.mkv', 'error', NULL)",
            [],
        ).is_err());

        // Verify CHECK rejects status='complete' with non-NULL error
        assert!(conn.execute(
            "INSERT INTO jobs(job_id, filename, status, error) VALUES ('j4', 'f4.mkv', 'complete', 'oops')",
            [],
        ).is_err());

        // Verify CHECK rejects source_path with /
        assert!(conn.execute(
            "INSERT INTO jobs(job_id, filename, status, source_path) VALUES ('j5', 'f5.mkv', 'complete', 'dir/file.mkv')",
            [],
        ).is_err());

        // Verify CHECK rejects source_path with ..
        assert!(conn.execute(
            "INSERT INTO jobs(job_id, filename, status, source_path) VALUES ('j6', 'f6.mkv', 'complete', '../escape.mkv')",
            [],
        ).is_err());

        // Verify valid inserts succeed
        conn.execute(
            "INSERT INTO jobs(job_id, filename, status) VALUES ('j7', 'f7.mkv', 'processing')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO jobs(job_id, filename, status, error) VALUES ('j8', 'f8.mkv', 'error', 'failed')",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO jobs(job_id, filename, status, source_path) VALUES ('j9', 'f9.mkv', 'complete', 'movie.mkv')",
            [],
        ).unwrap();
    }

    #[test]
    fn test_migration_16_enforce_segments_is_split_check() {
        let conn = Connection::open_in_memory().unwrap();
        run_migration_1(&conn).unwrap();
        run_migration_2(&conn).unwrap();
        run_migration_3(&conn).unwrap();
        run_migration_4(&conn).unwrap();
        run_migration_5(&conn).unwrap();
        run_migration_6(&conn).unwrap();
        run_migration_7(&conn).unwrap();
        run_migration_8(&conn).unwrap();
        run_migration_9(&conn).unwrap();
        run_migration_10(&conn).unwrap();
        run_migration_11(&conn).unwrap();
        run_migration_12(&conn).unwrap();
        run_migration_13(&conn).unwrap();
        run_migration_14(&conn).unwrap();
        run_migration_15(&conn).unwrap();

        // Seed a job + valid segment with is_split=0
        conn.execute(
            "INSERT INTO jobs(job_id, filename, status) VALUES ('j1', 'f.mkv', 'complete')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO segments(job_id, segment_key, file_id, bot_index, is_split) VALUES ('j1', 'vid/seg.m4s', 'fid', 1, 0)",
            [],
        ).unwrap();

        run_migration_16(&conn).unwrap();

        // is_split=0 ok, is_split=1 ok, is_split=2 fails
        conn.execute(
            "INSERT INTO segments(job_id, segment_key, file_id, bot_index, is_split) VALUES ('j1', 'vid/s2.m4s', 'fid2', 1, 0)",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO segments(job_id, segment_key, file_id, bot_index, is_split) VALUES ('j1', 'vid/s3.m4s', 'fid3', 1, 1)",
            [],
        ).unwrap();
        assert!(conn.execute(
            "INSERT INTO segments(job_id, segment_key, file_id, bot_index, is_split) VALUES ('j1', 'vid/s4.m4s', 'fid4', 1, 2)",
            [],
        ).is_err());
    }
}
