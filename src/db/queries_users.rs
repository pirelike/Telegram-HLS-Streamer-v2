use anyhow::{anyhow, Context, Result};
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use rand::RngCore;
use rusqlite::{params, Connection, OptionalExtension};

use super::models::*;

const SESSION_TTL_SECONDS: i64 = 30 * 24 * 60 * 60;

pub fn hash_password(password: &str) -> Result<String> {
    let salt = SaltString::generate(&mut argon2::password_hash::rand_core::OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|e| anyhow!("hashing password: {e}"))
}

pub fn verify_password(password: &str, hash: &str) -> Result<bool> {
    let parsed = PasswordHash::new(hash).map_err(|e| anyhow!("parsing password hash: {e}"))?;
    Ok(Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok())
}

pub fn create_user(
    conn: &Connection,
    username: &str,
    password_hash: &str,
    is_admin: bool,
) -> Result<UserRow> {
    let user_id = uuid::Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO users(user_id, username, password_hash, is_admin)
         VALUES (?1, ?2, ?3, ?4)",
        params![
            user_id,
            username.trim(),
            password_hash,
            bool_to_i64(is_admin)
        ],
    )?;
    get_user_by_id(conn, &user_id)?.context("created user row missing")
}

pub fn count_users(conn: &Connection) -> Result<i64> {
    conn.query_row("SELECT COUNT(*) FROM users", [], |row| row.get(0))
        .map_err(Into::into)
}

pub fn list_users(conn: &Connection) -> Result<Vec<UserRow>> {
    let mut stmt = conn.prepare(
        "SELECT user_id, username, password_hash, is_admin, created_at, last_seen_at
         FROM users ORDER BY username COLLATE NOCASE ASC",
    )?;
    let rows = stmt.query_map([], user_from_row)?;
    rows.map(|row| row.map_err(Into::into)).collect()
}

pub fn export_users(conn: &Connection) -> Result<Vec<UserRow>> {
    list_users(conn)
}

pub fn get_user_by_id(conn: &Connection, user_id: &str) -> Result<Option<UserRow>> {
    conn.query_row(
        "SELECT user_id, username, password_hash, is_admin, created_at, last_seen_at
         FROM users WHERE user_id = ?1",
        params![user_id],
        user_from_row,
    )
    .optional()
    .map_err(Into::into)
}

pub fn get_user_by_username(conn: &Connection, username: &str) -> Result<Option<UserRow>> {
    conn.query_row(
        "SELECT user_id, username, password_hash, is_admin, created_at, last_seen_at
         FROM users WHERE username = ?1 COLLATE NOCASE",
        params![username.trim()],
        user_from_row,
    )
    .optional()
    .map_err(Into::into)
}

pub fn delete_user(conn: &Connection, user_id: &str) -> Result<bool> {
    Ok(conn.execute("DELETE FROM users WHERE user_id = ?1", params![user_id])? > 0)
}

pub fn touch_user_seen(conn: &Connection, user_id: &str) -> Result<()> {
    conn.execute(
        "UPDATE users SET last_seen_at = CURRENT_TIMESTAMP WHERE user_id = ?1",
        params![user_id],
    )?;
    Ok(())
}

pub fn create_session(conn: &Connection, user_id: &str, token: &str) -> Result<SessionRow> {
    conn.execute(
        "INSERT INTO user_sessions(token, user_id, expires_at)
         VALUES (?1, ?2, datetime('now', '+' || ?3 || ' seconds'))",
        params![token, user_id, SESSION_TTL_SECONDS],
    )?;
    get_session(conn, token)?.context("created session row missing")
}

pub fn get_session(conn: &Connection, token: &str) -> Result<Option<SessionRow>> {
    conn.query_row(
        "SELECT token, user_id, created_at, expires_at
         FROM user_sessions WHERE token = ?1",
        params![token],
        session_from_row,
    )
    .optional()
    .map_err(Into::into)
}

pub fn get_session_user(conn: &Connection, token: &str) -> Result<Option<UserRow>> {
    delete_expired_sessions(conn)?;
    conn.query_row(
        "SELECT u.user_id, u.username, u.password_hash, u.is_admin, u.created_at, u.last_seen_at
         FROM user_sessions s
         JOIN users u ON u.user_id = s.user_id
         WHERE s.token = ?1 AND s.expires_at > CURRENT_TIMESTAMP",
        params![token],
        user_from_row,
    )
    .optional()
    .map_err(Into::into)
}

pub fn delete_session(conn: &Connection, token: &str) -> Result<bool> {
    Ok(conn.execute("DELETE FROM user_sessions WHERE token = ?1", params![token])? > 0)
}

pub fn delete_expired_sessions(conn: &Connection) -> Result<usize> {
    conn.execute(
        "DELETE FROM user_sessions WHERE expires_at <= CURRENT_TIMESTAMP",
        [],
    )
    .map_err(Into::into)
}

pub fn new_session_token() -> String {
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    hex_encode(&bytes)
}

fn user_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<UserRow> {
    Ok(UserRow {
        user_id: row.get(0)?,
        username: row.get(1)?,
        password_hash: row.get(2)?,
        is_admin: row.get::<_, i64>(3)? == 1,
        created_at: row.get(4)?,
        last_seen_at: row.get(5)?,
    })
}

fn session_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionRow> {
    Ok(SessionRow {
        token: row.get(0)?,
        user_id: row.get(1)?,
        created_at: row.get(2)?,
        expires_at: row.get(3)?,
    })
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn bool_to_i64(value: bool) -> i64 {
    if value {
        1
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_hash_verifies_and_rejects_wrong_password() {
        let hash = hash_password("correct horse").unwrap();
        assert!(verify_password("correct horse", &hash).unwrap());
        assert!(!verify_password("wrong", &hash).unwrap());
    }

    #[test]
    fn session_tokens_are_hex_32_bytes() {
        let token = new_session_token();
        assert_eq!(token.len(), 64);
        assert!(token.bytes().all(|b| b.is_ascii_hexdigit()));
    }
}
