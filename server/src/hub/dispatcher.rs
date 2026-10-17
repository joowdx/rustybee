//! Per-device dispatcher: replay, window of 1, pacing (spec 006).

use super::Outgoing;
use crate::app::{App, UiEvent};
use crate::domain::lifecycle::State;
use crate::domain::pacing::{Next, Pacer, clamp_delay_ms};
use crate::protocol::{Envelope, Send};
use crate::store::{self, devices, messages};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Notify, mpsc};

pub fn send_frame(m: &messages::MessageRow) -> Outgoing {
    let id = m.frame_id.clone().unwrap_or_else(crate::util::new_id);
    let env = Envelope::with_id(
        id,
        "send",
        Send {
            msg_id: m.public_id.clone(),
            to: m.peer.clone(),
            body: m.body.clone(),
            sim: m.sim,
        },
    );
    Outgoing::Text(env.to_text())
}

const RETRY_MIN: Duration = Duration::from_millis(250);
const RETRY_MAX: Duration = Duration::from_secs(30);

/// Run until the socket goes away. A store error restarts the loop (with the
/// replay) after a growing pause, so one failed query never leaves a
/// connected device without a dispatcher.
pub async fn run(
    app: App,
    device_id: i64,
    notify: Arc<Notify>,
    tx: mpsc::UnboundedSender<Outgoing>,
) {
    let mut pause = RETRY_MIN;
    loop {
        let started = Instant::now();
        match run_inner(&app, device_id, &notify, &tx).await {
            Ok(()) => return,
            Err(e) => {
                if tx.is_closed() {
                    return;
                }
                if started.elapsed() > RETRY_MAX {
                    pause = RETRY_MIN;
                }
                tracing::warn!(device_id, error = %e, retry_in = ?pause, "dispatcher error");
                tokio::time::sleep(pause).await;
                pause = (pause * 2).min(RETRY_MAX);
            }
        }
    }
}

async fn run_inner(
    app: &App,
    device_id: i64,
    notify: &Notify,
    tx: &mpsc::UnboundedSender<Outgoing>,
) -> store::Result<()> {
    let store = &app.store;
    // 1. Replay every unacked dispatched message with its stored frame id.
    let replay = store
        .read(move |c| messages::unacked_dispatched(c, device_id))
        .await?;
    for m in &replay {
        tracing::debug!(msg = %m.public_id, "replaying send");
        if tx.send(send_frame(m)).is_err() {
            return Ok(());
        }
    }
    let mut pacer = Pacer::new(app.cfg.inflight_timeout);
    if let Some(m) = store
        .read(move |c| messages::latest_dispatched(c, device_id))
        .await?
    {
        // The window resumes from the stored times, not from now: a row
        // dispatched long ago must not cost a full timeout (AC-006-04).
        let now_ms = app.clock.now_ms();
        let ago = |at: i64| Duration::from_millis((now_ms - at).max(0) as u64);
        let sent_ago = m.dispatched_at.map(ago).unwrap_or_default();
        pacer.resume(m.id, Instant::now(), sent_ago, m.device_acked_at.map(ago));
    }
    loop {
        if let Some(f) = pacer.in_flight() {
            let row = store.read(move |c| messages::by_id(c, f.msg)).await?;
            let now = Instant::now();
            match row {
                Some(r) if r.state == State::Dispatched => {
                    if r.device_acked_at.is_some() {
                        pacer.acked(now);
                    }
                }
                _ => pacer.release(now),
            }
        }
        let Some(dev) = store.read(move |c| devices::by_id(c, device_id)).await? else {
            return Ok(());
        };
        let delay = Duration::from_millis(clamp_delay_ms(dev.send_delay_ms) as u64);
        let wait = match pacer.poll(Instant::now(), delay) {
            Next::Send if dev.enabled => {
                let now_ms = app.clock.now_ms();
                let next = store
                    .write(move |c| messages::dispatch_next(c, device_id, now_ms))
                    .await?;
                match next {
                    Some(m) => {
                        tracing::debug!(msg = %m.public_id, "dispatching");
                        if tx.send(send_frame(&m)).is_err() {
                            return Ok(());
                        }
                        pacer.start(m.id, Instant::now());
                        app.publish(UiEvent::Message(m.public_id.clone()));
                        continue;
                    }
                    None => None,
                }
            }
            Next::Send => None,
            Next::Wait(t) => Some(t),
        };
        match wait {
            Some(t) => {
                tokio::select! {
                    _ = notify.notified() => {}
                    _ = tokio::time::sleep_until(tokio::time::Instant::from_std(t)) => {}
                }
            }
            None => notify.notified().await,
        }
    }
}
