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
        if migration.revision == 8 {
            conn.execute_batch("PRAGMA foreign_keys = OFF;")?;
        }
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
        if migration.revision == 8 {
            conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        }
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
        | "segment_parts" => Ok(table),
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
        | "part_index" => Ok(column),
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
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
         );
         INSERT OR IGNORE INTO bots_new(id, token, channel_id, label, created_at)
         SELECT id, token, channel_id, label, created_at FROM bots WHERE channel_id < 0 AND token != '';
         DROP TABLE bots;
         ALTER TABLE bots_new RENAME TO bots;",
    )?;
    Ok(())
}
