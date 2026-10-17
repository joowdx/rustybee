//! Periodic maintenance: TTL sweep, status timeout, pruning, WAL checkpoint.

use crate::app::{App, UiEvent};
use crate::store::messages::{self, MessageRow};
use crate::store::{self};
use std::time::{Duration, Instant};

const DAY_MS: i64 = 24 * 3600 * 1000;

pub async fn run(app: App) {
    let mut last_checkpoint = Instant::now();
    let mut last_prune: Option<i64> = None;
    loop {
        tokio::time::sleep(app.cfg.sweep_interval).await;
        if let Err(e) = sweep_once(&app).await {
            tracing::warn!(error = %e, "sweep");
        }
        if let Err(e) = crate::otp::resolve(&app).await {
            tracing::warn!(error = %e, "login code sweep");
        }
        let now = app.clock.now_ms();
        if last_prune.is_none_or(|t| now - t >= DAY_MS) {
            last_prune = Some(now);
            if let Err(e) = prune_once(&app).await {
                tracing::warn!(error = %e, "prune");
            }
        }
        if last_checkpoint.elapsed() >= Duration::from_secs(3600) {
            last_checkpoint = Instant::now();
            if let Err(e) = app.store.checkpoint().await {
                tracing::warn!(error = %e, "checkpoint");
            }
        }
    }
}

/// The daily prune: seen frames, auth rows and, when `RUSTYBEE_RETENTION_DAYS`
/// is set, finished messages and webhook deliveries (spec 003). Returns the
/// number of messages and deliveries deleted.
pub async fn prune_once(app: &App) -> store::Result<(usize, usize)> {
    let now = app.clock.now_ms();
    let retention = app.cfg.retention_days.map(|d| now - i64::from(d) * DAY_MS);
    let (msgs, hooks) = app
        .store
        .write(move |c| {
            c.execute(
                "DELETE FROM seen_frames WHERE seen_at<?1",
                [now - 7 * DAY_MS],
            )?;
            store::auth::prune(c, now)?;
            store::idempotency::prune(c, now)?;
            let Some(before) = retention else {
                return Ok((0, 0));
            };
            // Deliveries first; any left pointing at a pruned message get NULL.
            let hooks = store::webhooks::prune_finished(c, before)?;
            let msgs = messages::prune_finished(c, before)?;
            Ok((msgs, hooks))
        })
        .await?;
    if msgs + hooks > 0 {
        tracing::info!(messages = msgs, deliveries = hooks, "retention pruned");
    }
    Ok((msgs, hooks))
}

/// One TTL / status-timeout pass (spec 004). Returns the changed messages.
pub async fn sweep_once(app: &App) -> store::Result<Vec<MessageRow>> {
    let now = app.clock.now_ms();
    let ttl = app.cfg.message_ttl.as_millis() as i64;
    let timeout = app.cfg.status_timeout.as_millis() as i64;
    let changed = app
        .store
        .write(move |c| messages::sweep(c, now, ttl, timeout))
        .await?;
    if !changed.is_empty() {
        tracing::info!(count = changed.len(), "sweeper changed messages");
        app.webhook_notify.notify_one();
        for m in &changed {
            app.hub.notify(m.device_id);
            app.publish(UiEvent::Message(m.public_id.clone()));
        }
    }
    Ok(changed)
}
