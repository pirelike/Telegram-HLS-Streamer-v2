#![allow(dead_code)]

mod migrations;
mod models;
mod queries;
mod queries_library;
mod queries_metadata;
mod queries_playback;
mod queries_ratings;
mod queries_settings;
mod queries_user_lists;
mod queries_users;
mod row_mapping;
mod transfer;

pub use models::*;
pub use queries::*;
pub use queries_library::*;
pub use queries_metadata::*;
pub use queries_playback::*;
pub use queries_ratings::*;
pub use queries_settings::*;
pub use queries_user_lists::*;
pub use queries_users::*;
pub use transfer::*;

use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::Connection;

pub const LATEST_SCHEMA_REVISION: i64 = 34;

pub type DbPool = Pool<SqliteConnectionManager>;
pub type DbConn = r2d2::PooledConnection<SqliteConnectionManager>;

pub fn init_db_pool(path: &Path) -> Result<DbPool> {
    let conn = init_db(path)?;
    conn.close().map_err(|(_, e)| anyhow::anyhow!(e))?;

    let manager = sqlite_connection_manager(path);
    Pool::builder()
        .max_size(8)
        .build(manager)
        .context("creating sqlite connection pool")
}

pub(crate) fn init_db_pool_lazy(path: &Path) -> DbPool {
    let manager = sqlite_connection_manager(path);
    Pool::builder()
        .max_size(8)
        .min_idle(Some(0))
        .build_unchecked(manager)
}

fn sqlite_connection_manager(path: &Path) -> SqliteConnectionManager {
    SqliteConnectionManager::file(path)
        .with_init(|conn| conn.execute_batch(SQLITE_CONNECTION_PRAGMAS))
}

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

    // Safety net: verify foreign keys are enabled after all migrations.
    let fk_on: i64 = conn.query_row("PRAGMA foreign_keys", [], |row| row.get(0))?;
    if fk_on != 1 {
        tracing::error!("foreign_keys pragma is OFF after migrations; forcing ON");
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
    }

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
    conn.execute_batch(SQLITE_CONNECTION_PRAGMAS)?;
    Ok(())
}

const SQLITE_CONNECTION_PRAGMAS: &str = "\
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;
PRAGMA synchronous = NORMAL;
PRAGMA busy_timeout = 5000;
PRAGMA temp_store = MEMORY;
PRAGMA cache_size = -64000;
PRAGMA mmap_size = 268435456;";

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
    if settings_registry::is_public_setting_key(key) {
        Ok(())
    } else {
        anyhow::bail!("unknown setting key: {key}");
    }
}

fn validate_sqlite_header(path: &Path) -> Result<()> {
    let mut file = std::fs::File::open(path)
        .with_context(|| format!("opening sqlite header from {}", path.display()))?;
    let mut header = [0u8; 16];
    file.read_exact(&mut header)
        .with_context(|| format!("reading sqlite header from {}", path.display()))?;
    if &header == b"SQLite format 3\0" {
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
mod tests;
