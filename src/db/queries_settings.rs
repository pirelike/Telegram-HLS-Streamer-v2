use std::collections::HashMap;

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};

use super::models::*;

fn setting_value_type(key: &str) -> Result<&'static str> {
    let spec = crate::settings_registry::setting_spec(key)
        .ok_or_else(|| anyhow::anyhow!("unknown setting key: {key}"))?;
    Ok(crate::settings_registry::setting_type_name(
        spec.setting_type,
    ))
}

pub fn get_all_settings(conn: &Connection) -> Result<HashMap<String, String>> {
    let mut stmt = conn.prepare("SELECT key, value FROM settings ORDER BY key ASC")?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    let mut out = HashMap::new();
    for row in rows {
        let (k, v) = row?;
        if !k.starts_with('_') {
            out.insert(k, v);
        }
    }
    Ok(out)
}

pub fn set_setting(conn: &Connection, key: &str, value: &str) -> Result<()> {
    super::validate_setting_key(key)?;
    let value_type = setting_value_type(key)?;
    conn.execute(
        "INSERT INTO settings(key, value, value_type, updated_at) VALUES (?1, ?2, ?3, CURRENT_TIMESTAMP)
         ON CONFLICT(key) DO UPDATE SET value=excluded.value, value_type=excluded.value_type, updated_at=CURRENT_TIMESTAMP",
        params![key, value, value_type],
    )?;
    Ok(())
}

pub fn set_settings(conn: &mut Connection, settings: &HashMap<String, String>) -> Result<()> {
    let tx = conn.transaction()?;
    for (key, value) in settings {
        super::validate_setting_key(key)?;
        let value_type = setting_value_type(key)?;
        tx.execute(
            "INSERT INTO settings(key, value, value_type, updated_at) VALUES (?1, ?2, ?3, CURRENT_TIMESTAMP)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value, value_type=excluded.value_type, updated_at=CURRENT_TIMESTAMP",
            params![key, value, value_type],
        )?;
    }
    tx.commit()?;
    Ok(())
}

pub fn delete_setting(conn: &Connection, key: &str) -> Result<bool> {
    super::validate_setting_key(key)?;
    Ok(conn.execute("DELETE FROM settings WHERE key = ?1", params![key])? > 0)
}

pub fn get_last_bot_index(conn: &Connection) -> Result<i64> {
    let value: Option<String> = conn
        .query_row(
            "SELECT value FROM kv_internal WHERE key = '_last_bot_index'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    Ok(value
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(-1)
        .max(-1))
}

pub fn set_last_bot_index(conn: &Connection, index: i64) -> Result<()> {
    let index = index.max(0).to_string();
    conn.execute(
        "INSERT INTO kv_internal(key, value, updated_at) VALUES ('_last_bot_index', ?1, CURRENT_TIMESTAMP)
         ON CONFLICT(key) DO UPDATE SET value=excluded.value, updated_at=CURRENT_TIMESTAMP",
        params![index],
    )?;
    Ok(())
}

pub fn get_internal_value(conn: &Connection, key: &str) -> Result<Option<String>> {
    conn.query_row(
        "SELECT value FROM kv_internal WHERE key = ?1",
        params![key],
        |r| r.get(0),
    )
    .optional()
    .map_err(Into::into)
}

pub fn set_internal_value(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO kv_internal(key, value, updated_at) VALUES (?1, ?2, CURRENT_TIMESTAMP)
         ON CONFLICT(key) DO UPDATE SET value=excluded.value, updated_at=CURRENT_TIMESTAMP",
        params![key, value],
    )?;
    Ok(())
}

pub fn get_all_bots(conn: &Connection) -> Result<Vec<DbBotRow>> {
    let has_enabled =
        crate::db::migrations::column_exists(conn, "bots", "enabled").unwrap_or(false);
    let has_source = crate::db::migrations::column_exists(conn, "bots", "source").unwrap_or(false);
    let sql = if has_enabled && has_source {
        "SELECT id, token, channel_id, label, enabled, source FROM bots ORDER BY id ASC"
    } else {
        "SELECT id, token, channel_id, label, 1, 'db' FROM bots ORDER BY id ASC"
    };
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map([], |r| {
        Ok(DbBotRow {
            id: r.get(0)?,
            token: r.get(1)?,
            channel_id: r.get(2)?,
            label: r.get(3)?,
            enabled: r.get::<_, i64>(4)? != 0,
            source: r.get(5)?,
        })
    })?;
    rows.map(|r| r.map_err(Into::into)).collect()
}

pub fn add_bot(conn: &Connection, token: &str, channel_id: i64, label: &str) -> Result<i64> {
    conn.execute(
        "INSERT INTO bots(token, channel_id, label, enabled, source) VALUES (?1, ?2, ?3, 1, 'db')",
        params![token, channel_id, label],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn delete_bot(conn: &Connection, id: i64) -> Result<bool> {
    Ok(conn.execute("DELETE FROM bots WHERE id = ?1", params![id])? > 0)
}

pub fn bot_exists(conn: &Connection, token: &str) -> Result<bool> {
    let exists: i64 = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM bots WHERE token = ?1)",
        params![token],
        |r| r.get(0),
    )?;
    Ok(exists == 1)
}

pub fn get_bot_workload_stats(conn: &Connection) -> Result<HashMap<i64, BotWorkload>> {
    let mut stmt = conn.prepare(
        "SELECT bot_index, COUNT(*), COALESCE(SUM(file_size), 0) FROM segments GROUP BY bot_index",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            BotWorkload {
                segment_count: r.get(1)?,
                total_bytes: r.get(2)?,
            },
        ))
    })?;
    let mut out = HashMap::new();
    for row in rows {
        let (bot_index, stats) = row?;
        out.insert(bot_index, stats);
    }
    Ok(out)
}
