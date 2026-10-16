//! Pure domain logic. No IO, no clocks, no database.

pub mod filters;
pub mod lifecycle;
pub mod pacing;
pub mod phone;
pub mod signing;
pub mod sms;

use sha2::{Digest, Sha256};
use std::time::Duration;

pub fn hex(bytes: &[u8]) -> String {
    const H: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(H[(b >> 4) as usize] as char);
        s.push(H[(b & 15) as usize] as char);
    }
    s
}

pub fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

/// `inbound.uid` (spec 002): lowercase hex of the first 16 bytes of
/// `SHA-256(from \n received_at \n body \n sim)`.
pub fn inbound_uid(from: &str, received_at: i64, body: &str, sim: Option<i64>) -> String {
    let sim = sim.map(|s| s.to_string()).unwrap_or_default();
    let digest = Sha256::digest(format!("{from}\n{received_at}\n{body}\n{sim}").as_bytes());
    hex(&digest[..16])
}

/// Counts of a batch's messages by state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct BatchCounts {
    pub queued: i64,
    pub dispatched: i64,
    pub sent: i64,
    pub delivered: i64,
    pub failed: i64,
    pub expired: i64,
}

/// Derived batch status (spec 004).
pub fn batch_status(c: &BatchCounts) -> &'static str {
    if c.queued + c.dispatched > 0 {
        "pending"
    } else if c.failed + c.expired == 0 {
        "completed"
    } else if c.sent + c.delivered == 0 {
        "failed"
    } else {
        "partial"
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryOutcome {
    Delivered,
    /// Try again after this long.
    Retry(Duration),
    Abort,
}

pub const WEBHOOK_MAX_ATTEMPTS: i64 = 7;
pub const WEBHOOK_MAX_4XX_ATTEMPTS: i64 = 3;

/// Decide what happens after attempt number `attempt` (1-based) of a webhook
/// delivery. `status` is `None` for a network error or timeout.
pub fn webhook_outcome(attempt: i64, status: Option<u16>, backoff: &[Duration]) -> DeliveryOutcome {
    let retryable = match status {
        Some(s) if (200..300).contains(&s) => return DeliveryOutcome::Delivered,
        None => true,
        Some(s) => s == 408 || s == 429 || !(400..500).contains(&s),
    };
    let limit = if retryable {
        WEBHOOK_MAX_ATTEMPTS
    } else {
        WEBHOOK_MAX_4XX_ATTEMPTS
    };
    if attempt >= limit || backoff.is_empty() {
        return DeliveryOutcome::Abort;
    }
    let i = (attempt as usize - 1).min(backoff.len() - 1);
    DeliveryOutcome::Retry(backoff[i])
}

pub fn default_backoff() -> Vec<Duration> {
    [30, 120, 600, 3600, 6 * 3600, 24 * 3600]
        .into_iter()
        .map(Duration::from_secs)
        .collect()
}

/// Substitute `{{column}}` placeholders. Unknown names are left as-is.
pub fn render_template(template: &str, lookup: impl Fn(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        match after.find("}}") {
            Some(end) => {
                let name = after[..end].trim();
                match lookup(name) {
                    Some(v) => out.push_str(&v),
                    None => out.push_str(&rest[start..start + 2 + end + 2]),
                }
                rest = &after[end + 2..];
            }
            None => {
                out.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trip() {
        assert_eq!(hex(&[0, 15, 255]), "000fff");
        assert_eq!(unhex("000fff"), Some(vec![0, 15, 255]));
        assert_eq!(unhex("0g"), None);
        assert_eq!(unhex("abc"), None);
    }

    #[test]
    fn ac_002_01_inbound_uid_vector() {
        assert_eq!(
            inbound_uid(
                "+15551234567",
                1760000000000,
                "Your code is 123456",
                Some(1)
            ),
            "3b0368d22af9d295abcbe52c2ce7fdaa"
        );
    }

    #[test]
    fn ac_007_06_batch_status() {
        let c = |q, d, s, dl, f, e| BatchCounts {
            queued: q,
            dispatched: d,
            sent: s,
            delivered: dl,
            failed: f,
            expired: e,
        };
        assert_eq!(batch_status(&c(1, 0, 2, 0, 0, 0)), "pending");
        assert_eq!(batch_status(&c(0, 1, 0, 0, 0, 0)), "pending");
        assert_eq!(batch_status(&c(0, 0, 1, 2, 0, 0)), "completed");
        assert_eq!(batch_status(&c(0, 0, 0, 0, 2, 1)), "failed");
        assert_eq!(batch_status(&c(0, 0, 1, 0, 1, 0)), "partial");
    }

    #[test]
    fn ac_008_03_outcome_policy() {
        let b = default_backoff();
        assert_eq!(
            webhook_outcome(1, Some(204), &b),
            DeliveryOutcome::Delivered
        );
        let mut waits = vec![];
        for n in 1..=7 {
            match webhook_outcome(n, Some(500), &b) {
                DeliveryOutcome::Retry(d) => waits.push(d.as_secs()),
                DeliveryOutcome::Abort => assert_eq!(n, 7),
                DeliveryOutcome::Delivered => unreachable!(),
            }
        }
        assert_eq!(waits, vec![30, 120, 600, 3600, 21600, 86400]);
        assert_eq!(webhook_outcome(1, None, &b), DeliveryOutcome::Retry(b[0]));
        assert_eq!(
            webhook_outcome(1, Some(429), &b),
            DeliveryOutcome::Retry(b[0])
        );
        assert_eq!(
            webhook_outcome(1, Some(408), &b),
            DeliveryOutcome::Retry(b[0])
        );
        assert_eq!(
            webhook_outcome(2, Some(400), &b),
            DeliveryOutcome::Retry(b[1])
        );
        assert_eq!(webhook_outcome(3, Some(404), &b), DeliveryOutcome::Abort);
    }

    #[test]
    fn template_substitution() {
        let r = render_template("Hi {{ name }}, {{x}} {{name}}{{", |k| {
            (k == "name").then(|| "Ann".to_string())
        });
        assert_eq!(r, "Hi Ann, {{x}} Ann{{");
    }
}
