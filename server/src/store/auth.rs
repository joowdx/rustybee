//! Admin password, sessions, API keys and pairing codes (spec 005).

use super::{get_setting, opt, set_setting};
use crate::util::{new_id, random_token, sha256};
use argon2::Argon2;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use rusqlite::{Connection, params};

pub const SESSION_TTL_MS: i64 = 30 * 24 * 3600 * 1000;
pub const SESSION_BUMP_MS: i64 = 3600 * 1000;
pub const PAIRING_TTL_MS: i64 = 10 * 60 * 1000;
pub const MIN_PASSWORD: usize = 10;

pub fn hash_password(pw: &str) -> String {
    let salt = SaltString::generate(&mut argon2::password_hash::rand_core::OsRng);
    Argon2::default()
        .hash_password(pw.as_bytes(), &salt)
        .expect("argon2 hash")
        .to_string()
}

pub fn verify_password(hash: &str, pw: &str) -> bool {
    PasswordHash::new(hash)
        .map(|h| Argon2::default().verify_password(pw.as_bytes(), &h).is_ok())
        .unwrap_or(false)
}

pub fn password_hash(c: &Connection) -> rusqlite::Result<Option<String>> {
    get_setting(c, "admin_password")
}

pub fn set_password_hash(c: &Connection, hash: &str) -> rusqlite::Result<()> {
    set_setting(c, "admin_password", hash)
}

// ---- sessions ----

#[derive(Debug, Clone)]
pub struct SessionRow {
    pub id: i64,
    pub created_at: i64,
    pub last_seen_at: i64,
    pub expires_at: i64,
    pub user_agent: Option<String>,
}

/// Create a session; returns `(row id, cookie token)`.
pub fn create_session(
    c: &Connection,
    ua: Option<&str>,
    now: i64,
) -> rusqlite::Result<(i64, String)> {
    let token = random_token();
    c.execute(
        "INSERT INTO sessions(token_sha256, created_at, last_seen_at, expires_at, user_agent)
         VALUES(?1,?2,?2,?3,?4)",
        params![sha256(&token), now, now + SESSION_TTL_MS, ua],
    )?;
    Ok((c.last_insert_rowid(), token))
}

pub fn session_by_token(
    c: &Connection,
    token: &str,
    now: i64,
) -> rusqlite::Result<Option<SessionRow>> {
    opt(c.query_row(
        "SELECT id, created_at, last_seen_at, expires_at, user_agent FROM sessions
         WHERE token_sha256=?1 AND expires_at>?2",
        params![sha256(token), now],
        |r| {
            Ok(SessionRow {
                id: r.get(0)?,
                created_at: r.get(1)?,
                last_seen_at: r.get(2)?,
                expires_at: r.get(3)?,
                user_agent: r.get(4)?,
            })
        },
    ))
}

pub fn bump_session(c: &Connection, id: i64, now: i64) -> rusqlite::Result<()> {
    c.execute(
        "UPDATE sessions SET last_seen_at=?2, expires_at=?3 WHERE id=?1",
        params![id, now, now + SESSION_TTL_MS],
    )?;
    Ok(())
}

pub fn list_sessions(c: &Connection, now: i64) -> rusqlite::Result<Vec<SessionRow>> {
    let mut st = c.prepare(
        "SELECT id, created_at, last_seen_at, expires_at, user_agent FROM sessions
         WHERE expires_at>?1 ORDER BY last_seen_at DESC",
    )?;
    st.query_map([now], |r| {
        Ok(SessionRow {
            id: r.get(0)?,
            created_at: r.get(1)?,
            last_seen_at: r.get(2)?,
            expires_at: r.get(3)?,
            user_agent: r.get(4)?,
        })
    })?
    .collect()
}

pub fn delete_session(c: &Connection, id: i64) -> rusqlite::Result<()> {
    c.execute("DELETE FROM sessions WHERE id=?1", [id])?;
    Ok(())
}

pub fn delete_other_sessions(c: &Connection, keep: i64) -> rusqlite::Result<()> {
    c.execute("DELETE FROM sessions WHERE id<>?1", [keep])?;
    Ok(())
}

// ---- API keys ----

#[derive(Debug, Clone)]
pub struct ApiKeyRow {
    pub id: i64,
    pub public_id: String,
    pub name: String,
    pub prefix: String,
    pub created_at: i64,
    pub last_used_at: Option<i64>,
    pub revoked_at: Option<i64>,
}

const KEY_COLS: &str = "id, public_id, name, prefix, created_at, last_used_at, revoked_at";

fn key_row(r: &rusqlite::Row) -> rusqlite::Result<ApiKeyRow> {
    Ok(ApiKeyRow {
        id: r.get(0)?,
        public_id: r.get(1)?,
        name: r.get(2)?,
        prefix: r.get(3)?,
        created_at: r.get(4)?,
        last_used_at: r.get(5)?,
        revoked_at: r.get(6)?,
    })
}

/// Create a key; returns the row and the secret (shown once).
pub fn create_key(c: &Connection, name: &str, now: i64) -> rusqlite::Result<(ApiKeyRow, String)> {
    let secret = format!("rbk_{}", random_token());
    c.execute(
        "INSERT INTO api_keys(public_id, name, prefix, token_sha256, created_at) VALUES(?1,?2,?3,?4,?5)",
        params![new_id(), name, &secret[..12], sha256(&secret), now],
    )?;
    let row = opt(c.query_row(
        &format!("SELECT {KEY_COLS} FROM api_keys WHERE id=?1"),
        [c.last_insert_rowid()],
        key_row,
    ))?
    .expect("just inserted");
    Ok((row, secret))
}

pub fn key_by_secret(c: &Connection, secret: &str) -> rusqlite::Result<Option<ApiKeyRow>> {
    opt(c.query_row(
        &format!("SELECT {KEY_COLS} FROM api_keys WHERE token_sha256=?1"),
        [sha256(secret)],
        key_row,
    ))
}

pub fn list_keys(c: &Connection) -> rusqlite::Result<Vec<ApiKeyRow>> {
    let mut st = c.prepare(&format!("SELECT {KEY_COLS} FROM api_keys ORDER BY id DESC"))?;
    st.query_map([], key_row)?.collect()
}

pub fn touch_key(c: &Connection, id: i64, now: i64) -> rusqlite::Result<()> {
    c.execute(
        "UPDATE api_keys SET last_used_at=?2 WHERE id=?1",
        params![id, now],
    )?;
    Ok(())
}

pub fn rename_key(c: &Connection, pid: &str, name: &str) -> rusqlite::Result<usize> {
    c.execute(
        "UPDATE api_keys SET name=?2 WHERE public_id=?1",
        params![pid, name],
    )
}

pub fn revoke_key(c: &Connection, pid: &str, now: i64) -> rusqlite::Result<usize> {
    c.execute(
        "UPDATE api_keys SET revoked_at=COALESCE(revoked_at, ?2) WHERE public_id=?1",
        params![pid, now],
    )
}

pub fn delete_key(c: &Connection, pid: &str) -> rusqlite::Result<usize> {
    c.execute("DELETE FROM api_keys WHERE public_id=?1", [pid])
}

// ---- pairing codes ----

pub fn create_pairing_code(c: &Connection, now: i64) -> rusqlite::Result<(String, i64)> {
    let code = crate::util::pairing_code();
    let expires = now + PAIRING_TTL_MS;
    c.execute(
        "INSERT INTO pairing_codes(code_sha256, created_at, expires_at) VALUES(?1,?2,?3)",
        params![sha256(&code), now, expires],
    )?;
    Ok((code, expires))
}

/// Mark a code used if it is valid. Returns whether it was.
pub fn consume_pairing_code(c: &Connection, code: &str, now: i64) -> rusqlite::Result<bool> {
    let n = c.execute(
        "UPDATE pairing_codes SET used_at=?2 WHERE code_sha256=?1 AND used_at IS NULL AND expires_at>?2",
        params![sha256(&code.trim().to_ascii_uppercase()), now],
    )?;
    Ok(n == 1)
}

/// Remove expired sessions and stale pairing codes.
pub fn prune(c: &Connection, now: i64) -> rusqlite::Result<()> {
    c.execute("DELETE FROM sessions WHERE expires_at<=?1", [now])?;
    c.execute(
        "DELETE FROM pairing_codes WHERE expires_at<=?1",
        [now - 24 * 3600 * 1000],
    )?;
    Ok(())
}
