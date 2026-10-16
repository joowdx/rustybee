//! Small helpers: wall clock, ids, tokens, timestamps.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::RngCore;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use time::OffsetDateTime;

/// Wall clock in Unix milliseconds, with an offset tests can advance.
#[derive(Clone, Default)]
pub struct Clock {
    offset_ms: Arc<AtomicI64>,
}

impl Clock {
    pub fn now_ms(&self) -> i64 {
        let real = OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000;
        real as i64 + self.offset_ms.load(Ordering::Relaxed)
    }

    pub fn advance_ms(&self, ms: i64) {
        self.offset_ms.fetch_add(ms, Ordering::Relaxed);
    }
}

pub fn new_id() -> String {
    ulid::Ulid::new().to_string()
}

pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    rand::thread_rng().fill_bytes(&mut b);
    b
}

/// 32 random bytes as 43 base64url characters.
pub fn random_token() -> String {
    URL_SAFE_NO_PAD.encode(random_bytes::<32>())
}

pub fn b64url(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

pub fn b64url_decode(s: &str) -> Option<Vec<u8>> {
    URL_SAFE_NO_PAD.decode(s).ok()
}

pub fn sha256(s: &str) -> Vec<u8> {
    Sha256::digest(s.as_bytes()).to_vec()
}

const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// Pairing code: 20 random Crockford base32 characters.
pub fn pairing_code() -> String {
    random_bytes::<20>()
        .iter()
        .map(|b| CROCKFORD[(b & 31) as usize] as char)
        .collect()
}

/// RFC 3339 UTC with milliseconds, e.g. `2026-10-16T08:00:00.000Z`.
pub fn rfc3339(ms: i64) -> String {
    let fmt = time::macros::format_description!(
        "[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:3]Z"
    );
    OffsetDateTime::from_unix_timestamp_nanos(ms as i128 * 1_000_000)
        .ok()
        .and_then(|t| t.format(&fmt).ok())
        .unwrap_or_default()
}

pub fn parse_rfc3339(s: &str) -> Option<i64> {
    let t = OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok()?;
    Some((t.unix_timestamp_nanos() / 1_000_000) as i64)
}

/// Short human form for the UI: `2026-10-16 08:00`.
pub fn human_time(ms: i64) -> String {
    let fmt = time::macros::format_description!("[year]-[month]-[day] [hour]:[minute]:[second]");
    OffsetDateTime::from_unix_timestamp_nanos(ms as i128 * 1_000_000)
        .ok()
        .and_then(|t| t.format(&fmt).ok())
        .unwrap_or_default()
}

/// Compact timestamp for lists, like "Oct 16, 01:37".
pub fn short_time(ms: i64) -> String {
    let fmt = time::macros::format_description!("[month repr:short] [day padding:none], [hour]:[minute]");
    OffsetDateTime::from_unix_timestamp_nanos(ms as i128 * 1_000_000)
        .ok()
        .and_then(|t| t.format(&fmt).ok())
        .unwrap_or_default()
}

pub fn ago(now: i64, then: i64) -> String {
    let s = ((now - then) / 1000).max(0);
    match s {
        0..=59 => format!("{s}s ago"),
        60..=3599 => format!("{}m ago", s / 60),
        3600..=86399 => format!("{}h ago", s / 3600),
        _ => format!("{}d ago", s / 86400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3339_round_trip() {
        assert_eq!(rfc3339(1_760_601_600_123), "2025-10-16T08:00:00.123Z");
        assert_eq!(
            parse_rfc3339("2025-10-16T08:00:00.123Z"),
            Some(1_760_601_600_123)
        );
        assert_eq!(
            parse_rfc3339("2025-10-16T10:00:00+02:00"),
            Some(1_760_601_600_000)
        );
        assert_eq!(parse_rfc3339("yesterday"), None);
    }

    #[test]
    fn tokens_have_spec_shape() {
        assert_eq!(random_token().len(), 43);
        let c = pairing_code();
        assert_eq!(c.len(), 20);
        assert!(c.bytes().all(|b| CROCKFORD.contains(&b)));
    }
}
