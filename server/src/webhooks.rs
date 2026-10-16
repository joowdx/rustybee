//! Webhook outbox worker (spec 008).

use crate::app::{App, VERSION};
use crate::domain::{DeliveryOutcome, signing, webhook_outcome};
use crate::store::webhooks::{self as hooks, Claimed, WebhookRow};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

pub const CONCURRENCY: usize = 4;
pub const CLAIM_LIMIT: i64 = 16;
const MAX_IDLE: Duration = Duration::from_secs(60);
/// Stop reading a receiver's response body after this many bytes.
pub const MAX_RESPONSE_READ: usize = 2048;
/// Characters of the response body kept in `last_error`.
pub const MAX_ERROR_CHARS: usize = 500;

/// The first [`MAX_ERROR_CHARS`] characters of at most [`MAX_RESPONSE_READ`]
/// bytes. A character cut at the byte limit becomes U+FFFD.
fn response_excerpt(buf: &[u8]) -> String {
    let buf = &buf[..buf.len().min(MAX_RESPONSE_READ)];
    String::from_utf8_lossy(buf)
        .chars()
        .take(MAX_ERROR_CHARS)
        .collect()
}

pub async fn run(app: App) {
    let sem = Arc::new(Semaphore::new(CONCURRENCY));
    loop {
        match run_once(&app, &sem).await {
            Ok(n) if n > 0 => continue,
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, "webhook worker"),
        }
        let next = app.store.read(hooks::next_due).await.ok().flatten();
        let wait = next
            .map(|t| Duration::from_millis((t - app.clock.now_ms()).max(1) as u64))
            .unwrap_or(MAX_IDLE)
            .min(MAX_IDLE);
        tokio::select! {
            _ = app.webhook_notify.notified() => {}
            _ = tokio::time::sleep(wait) => {}
        }
    }
}

/// Claim due rows and attempt each one. Returns how many were claimed.
pub async fn run_once(app: &App, sem: &Arc<Semaphore>) -> crate::store::Result<usize> {
    let now = app.clock.now_ms();
    let lease = app.cfg.webhook_lease.as_millis() as i64;
    let claimed = app
        .store
        .write(move |c| hooks::claim(c, now, lease, CLAIM_LIMIT))
        .await?;
    let n = claimed.len();
    let mut set = JoinSet::new();
    for d in claimed {
        let permit = sem.clone().acquire_owned().await.expect("semaphore open");
        let app = app.clone();
        set.spawn(async move {
            attempt(&app, d).await;
            drop(permit);
        });
    }
    while set.join_next().await.is_some() {}
    Ok(n)
}

async fn post(
    app: &App,
    url: &str,
    event: &str,
    delivery: &str,
    secret: &str,
    body: &str,
) -> (Option<u16>, String) {
    let t = app.clock.now_ms() / 1000;
    let res = app
        .http
        .post(url)
        .timeout(app.cfg.webhook_timeout)
        .header("Content-Type", "application/json")
        .header("User-Agent", format!("rustybee/{VERSION}"))
        .header("X-Rustybee-Event", event)
        .header("X-Rustybee-Delivery", delivery)
        .header(
            "X-Rustybee-Signature",
            signing::sign(secret, t, body.as_bytes()),
        )
        .body(body.to_string())
        .send()
        .await;
    match res {
        Ok(mut r) => {
            let status = r.status().as_u16();
            let mut buf = Vec::new();
            while buf.len() < MAX_RESPONSE_READ {
                match r.chunk().await {
                    Ok(Some(c)) => buf.extend_from_slice(&c),
                    _ => break,
                }
            }
            (Some(status), response_excerpt(&buf))
        }
        Err(e) => (None, e.to_string()),
    }
}

async fn attempt(app: &App, d: Claimed) {
    let (status, text) = post(app, &d.url, &d.event, &d.public_id, &d.secret, &d.payload).await;
    let outcome = webhook_outcome(d.attempts + 1, status, &app.cfg.webhook_backoff);
    let now = app.clock.now_ms();
    let id = d.id;
    tracing::debug!(delivery = %d.public_id, ?status, ?outcome, "webhook attempt");
    let r = app
        .store
        .write(move |c| match outcome {
            DeliveryOutcome::Delivered => hooks::mark_delivered(c, id, status.unwrap_or(200), now),
            DeliveryOutcome::Retry(after) => hooks::mark_failed(
                c,
                id,
                status,
                &text,
                Some(now + after.as_millis() as i64),
                now,
            ),
            DeliveryOutcome::Abort => hooks::mark_failed(c, id, status, &text, None, now),
        })
        .await;
    if let Err(e) = r {
        tracing::warn!(error = %e, "record webhook outcome");
    }
}

/// UI "Test": send a synthetic `message.received` payload directly.
pub async fn send_test(app: &App, hook: &WebhookRow) -> Result<u16, String> {
    let now = app.clock.now_ms();
    let id = crate::util::new_id();
    let body = serde_json::json!({
        "id": id,
        "event": "message.received",
        "created_at": crate::util::rfc3339(now),
        "test": true,
        "data": {
            "id": crate::util::new_id(), "batch_id": null, "device_id": null, "direction": "in",
            "peer": "+639170000000", "body": "rustybee test", "sim": null, "state": "received",
            "parts": null, "error": null, "created_at": crate::util::rfc3339(now),
            "dispatched_at": null, "sent_at": null, "delivered_at": null, "failed_at": null,
            "expired_at": null, "received_at": crate::util::rfc3339(now)
        }
    })
    .to_string();
    match post(app, &hook.url, "message.received", &id, &hook.secret, &body).await {
        (Some(s), _) => Ok(s),
        (None, e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ac_008_06_excerpt_is_bounded_and_utf8_safe() {
        assert_eq!(response_excerpt(b"nope"), "nope");
        let long = "x".repeat(10_000);
        assert_eq!(response_excerpt(long.as_bytes()).chars().count(), 500);
        // 3-byte characters: 2048 bytes hold 682 of them, of which 500 are kept.
        let wide = "\u{20AC}".repeat(1000);
        assert_eq!(response_excerpt(wide.as_bytes()), "\u{20AC}".repeat(500));
        // A character split at the byte limit does not panic.
        let mut odd = "a".repeat(2047).into_bytes();
        odd.extend_from_slice("\u{20AC}".as_bytes());
        assert_eq!(response_excerpt(&odd), "a".repeat(500));
    }
}
