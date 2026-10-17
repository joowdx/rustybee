//! `Idempotency-Key` records for the send endpoints (spec 007).

use super::opt;
use rusqlite::{Connection, params};

/// A key is remembered for 24 hours, whatever the retention setting.
pub const TTL_MS: i64 = 24 * 3600 * 1000;
pub const MAX_KEY_LEN: usize = 255;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lookup {
    /// Unknown or expired key: run the request.
    Miss,
    /// Same key and same request: the stored response body.
    Replay(String),
    /// Same key, different request.
    Mismatch,
}

pub fn lookup(c: &Connection, key: &str, request: &[u8], now: i64) -> rusqlite::Result<Lookup> {
    let row: Option<(Vec<u8>, String, i64)> = opt(c.query_row(
        "SELECT request_sha256, response, created_at FROM idempotency_keys WHERE key=?1",
        [key],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    ))?;
    Ok(match row {
        None => Lookup::Miss,
        Some((_, _, at)) if at <= now - TTL_MS => {
            c.execute("DELETE FROM idempotency_keys WHERE key=?1", [key])?;
            Lookup::Miss
        }
        Some((sha, response, _)) if sha == request => Lookup::Replay(response),
        Some(_) => Lookup::Mismatch,
    })
}

pub fn insert(
    c: &Connection,
    key: &str,
    request: &[u8],
    response: &str,
    now: i64,
) -> rusqlite::Result<()> {
    c.execute(
        "INSERT INTO idempotency_keys(key, request_sha256, response, created_at) VALUES(?1,?2,?3,?4)",
        params![key, request, response, now],
    )?;
    Ok(())
}

/// Daily prune: forget keys older than [`TTL_MS`].
pub fn prune(c: &Connection, now: i64) -> rusqlite::Result<usize> {
    c.execute(
        "DELETE FROM idempotency_keys WHERE created_at<=?1",
        [now - TTL_MS],
    )
}
