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
        let now = app.clock.now_ms();
        if last_prune.is_none_or(|t| now - t >= DAY_MS) {
            last_prune = Some(now);
            let r = app
                .store
                .write(move |c| {
                    c.execute(
                        "DELETE FROM seen_frames WHERE seen_at<?1",
                        [now - 7 * DAY_MS],
                    )?;
                    store::auth::prune(c, now)
                })
                .await;
            if let Err(e) = r {
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
