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
        Ok(r) => {
            let status = r.status().as_u16();
            let text = r.text().await.unwrap_or_default();
            (Some(status), text.chars().take(500).collect())
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
            "peer": "+15550000000", "body": "rustybee test", "sim": null, "state": "received",
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
