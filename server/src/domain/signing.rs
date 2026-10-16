//! Webhook signatures (spec 008): `t=<unix seconds>,v1=<hex HMAC-SHA256(secret, "<t>.<body>")>`.

use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

fn mac(secret: &str, t: i64, body: &[u8]) -> HmacSha256 {
    let mut m = HmacSha256::new_from_slice(secret.as_bytes()).expect("hmac accepts any key length");
    m.update(t.to_string().as_bytes());
    m.update(b".");
    m.update(body);
    m
}

pub fn sign(secret: &str, t: i64, body: &[u8]) -> String {
    let tag = mac(secret, t, body).finalize().into_bytes();
    format!("t={t},v1={}", super::hex(&tag))
}

/// The receiver recipe: split the header, recompute over `t.body`, compare in
/// constant time, reject if the timestamp is more than `tolerance_s` away from `now_s`.
pub fn verify(secret: &str, header: &str, body: &[u8], now_s: i64, tolerance_s: i64) -> bool {
    let mut t = None;
    let mut v1 = None;
    for part in header.split(',') {
        match part.split_once('=') {
            Some(("t", v)) => t = v.parse::<i64>().ok(),
            Some(("v1", v)) => v1 = super::unhex(v),
            _ => {}
        }
    }
    let (Some(t), Some(v1)) = (t, v1) else {
        return false;
    };
    if (now_s - t).abs() > tolerance_s {
        return false;
    }
    mac(secret, t, body).verify_slice(&v1).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ac_008_02_signature_verifies() {
        let body = br#"{"event":"message.sent"}"#;
        let h = sign("s3cret", 1_760_000_000, body);
        assert!(h.starts_with("t=1760000000,v1="));
        assert_eq!(h.len(), "t=1760000000,v1=".len() + 64);
        assert!(verify("s3cret", &h, body, 1_760_000_100, 300));
    }

    #[test]
    fn ac_008_02_wrong_secret_or_body_or_time_fails() {
        let body = b"{}";
        let h = sign("s3cret", 1_760_000_000, body);
        assert!(!verify("other", &h, body, 1_760_000_000, 300));
        assert!(!verify("s3cret", &h, b"{ }", 1_760_000_000, 300));
        assert!(!verify("s3cret", &h, body, 1_760_000_301, 300));
        assert!(!verify("s3cret", "garbage", body, 1_760_000_000, 300));
    }

    #[test]
    fn known_vector() {
        // HMAC-SHA256(key="key", "1.x")
        let h = sign("key", 1, b"x");
        let expected = {
            let mut m = HmacSha256::new_from_slice(b"key").unwrap();
            m.update(b"1.x");
            super::super::hex(&m.finalize().into_bytes())
        };
        assert_eq!(h, format!("t=1,v1={expected}"));
    }
}
