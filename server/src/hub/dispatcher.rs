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

pub async fn run(
    app: App,
    device_id: i64,
    notify: Arc<Notify>,
    tx: mpsc::UnboundedSender<Outgoing>,
) {
    if let Err(e) = run_inner(&app, device_id, &notify, &tx).await {
        tracing::warn!(device_id, error = %e, "dispatcher stopped");
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
        pacer.start(m.id, Instant::now());
        if m.device_acked_at.is_some() {
            pacer.acked(Instant::now());
        }
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
