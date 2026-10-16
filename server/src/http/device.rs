//! Device pairing and the device WebSocket (spec 002).

use crate::app::{App, UiEvent};
use crate::domain::lifecycle::Event;
use crate::error::ApiError;
use crate::hub::{Outgoing, dispatcher};
use crate::protocol::{
    self, Ack, Envelope, Frame, Nack, PairRequest, PairResponse, StatusState, Welcome,
};
use crate::store::{self, auth, devices, devices::DeviceRow, messages};
use crate::util::{random_token, sha256};
use axum::extract::State;
use axum::extract::rejection::JsonRejection;
use axum::extract::ws::rejection::WebSocketUpgradeRejection;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use rusqlite::{Connection, params};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::{Notify, mpsc};
use tokio::time::Instant;

pub fn routes() -> Router<App> {
    Router::new()
        .route("/api/device/pair", post(pair))
        .route("/api/device/ws", get(ws))
}

async fn pair(
    State(app): State<App>,
    req: Result<Json<PairRequest>, JsonRejection>,
) -> Result<Json<PairResponse>, ApiError> {
    let Json(req) = req?;
    let token = format!("rbd_{}", random_token());
    let sha = sha256(&token);
    let now = app.clock.now_ms();
    let dev = app
        .store
        .write(move |c| {
            if !auth::consume_pairing_code(c, &req.code, now)? {
                return Ok(None);
            }
            devices::create(c, &req, &sha, now).map(Some)
        })
        .await?
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::UNAUTHORIZED,
                "pairing_invalid",
                "pairing code is unknown, expired or already used",
            )
        })?;
    tracing::info!(device = %dev.public_id, name = %dev.name, "device paired");
    app.publish(UiEvent::Device(dev.public_id.clone()));
    Ok(Json(PairResponse {
        device_id: dev.public_id,
        device_token: token,
        ws_url: app.cfg.ws_url(),
    }))
}

async fn ws(
    State(app): State<App>,
    headers: HeaderMap,
    upgrade: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
) -> Response {
    let Some(token) = super::bearer(&headers).filter(|t| t.starts_with("rbd_")) else {
        return ApiError::unauthorized("missing device token").into_response();
    };
    let sha = sha256(token);
    let dev = match app.store.read(move |c| devices::by_token(c, &sha)).await {
        Ok(Some(d)) => d,
        Ok(None) => return ApiError::unauthorized("unknown device token").into_response(),
        Err(e) => return ApiError::from(e).into_response(),
    };
    match upgrade {
        Ok(u) => u.on_upgrade(move |socket| connection(app, socket, dev)),
        Err(e) => e.into_response(),
    }
}

struct Conn {
    app: App,
    device_id: i64,
    device_public_id: String,
    generation: u64,
    notify: Arc<Notify>,
    tx: mpsc::UnboundedSender<Outgoing>,
    welcomed: AtomicBool,
}

async fn connection(app: App, mut socket: WebSocket, dev: DeviceRow) {
    let reg = app.hub.register(dev.id);
    let mut rx = reg.rx;
    let conn = Conn {
        app: app.clone(),
        device_id: dev.id,
        device_public_id: dev.public_id.clone(),
        generation: reg.generation,
        notify: reg.notify,
        tx: reg.tx,
        welcomed: AtomicBool::new(false),
    };
    tracing::info!(device = %dev.public_id, "device connected");
    let idle = app.cfg.idle_timeout;
    let mut deadline = Instant::now() + idle;
    loop {
        tokio::select! {
            biased;
            out = rx.recv() => match out {
                Some(Outgoing::Text(t)) => {
                    if socket.send(Message::Text(t.into())).await.is_err() {
                        break;
                    }
                }
                Some(Outgoing::Close(code, reason)) => {
                    let _ = socket
                        .send(Message::Close(Some(CloseFrame { code, reason: reason.into() })))
                        .await;
                    break;
                }
                None => break,
            },
            msg = socket.recv() => {
                deadline = Instant::now() + idle;
                match msg {
                    None | Some(Err(_)) | Some(Ok(Message::Close(_))) => break,
                    Some(Ok(Message::Text(t))) => {
                        let replies = match conn.handle(t.as_str()).await {
                            Ok(r) => r,
                            Err(e) => {
                                tracing::warn!(error = %e, "device frame");
                                vec![]
                            }
                        };
                        let mut failed = false;
                        for r in replies {
                            if socket.send(Message::Text(r.into())).await.is_err() {
                                failed = true;
                                break;
                            }
                        }
                        if failed {
                            break;
                        }
                        conn.after_replies();
                    }
                    Some(Ok(_)) => {}
                }
            }
            _ = tokio::time::sleep_until(deadline) => {
                tracing::info!(device = %dev.public_id, "device idle; closing");
                let _ = socket
                    .send(Message::Close(Some(CloseFrame { code: 1001, reason: "idle timeout".into() })))
                    .await;
                break;
            }
        }
    }
    if app.hub.unregister(dev.id, conn.generation) {
        let now = app.clock.now_ms();
        let id = dev.id;
        let _ = app
            .store
            .write(move |c| devices::set_connected(c, id, false, now))
            .await;
        app.publish(UiEvent::Device(dev.public_id.clone()));
    }
    tracing::info!(device = %dev.public_id, "device disconnected");
}

fn ack(reference: &str) -> String {
    Envelope::new(
        "ack",
        Ack {
            reference: reference.to_string(),
        },
    )
    .to_text()
}

fn nack(reference: &str, code: &str, message: &str) -> String {
    Envelope::new(
        "nack",
        Nack {
            reference: reference.to_string(),
            code: code.to_string(),
            message: message.to_string(),
        },
    )
    .to_text()
}

/// Record a device frame id; `false` if it was already seen.
fn first_sight(c: &Connection, device_id: i64, frame_id: &str, now: i64) -> rusqlite::Result<bool> {
    let n = c.execute(
        "INSERT INTO seen_frames(device_id, frame_id, seen_at) VALUES(?1,?2,?3)
         ON CONFLICT DO NOTHING",
        params![device_id, frame_id, now],
    )?;
    Ok(n == 1)
}

impl Conn {
    /// Handle one text frame; returns frames to send back, in order.
    async fn handle(&self, text: &str) -> store::Result<Vec<String>> {
        let (id, frame) = match protocol::decode(text) {
            Ok(v) => v,
            Err(e) => {
                tracing::debug!(code = e.code, message = %e.message, "bad device frame");
                return Ok(e
                    .id
                    .map(|id| vec![nack(&id, e.code, &e.message)])
                    .unwrap_or_default());
            }
        };
        let app = &self.app;
        let device_id = self.device_id;
        let now = app.clock.now_ms();
        match frame {
            Frame::Hello(h) => {
                app.store
                    .write(move |c| devices::apply_hello(c, device_id, &h, now))
                    .await?;
                let dev = app
                    .store
                    .read(move |c| devices::by_id(c, device_id))
                    .await?;
                let Some(dev) = dev else { return Ok(vec![]) };
                let welcome = Envelope::new(
                    "welcome",
                    Welcome {
                        device_id: dev.public_id.clone(),
                        server_time: now,
                        config: dev.config(app.cfg.ping_s),
                    },
                );
                app.publish(UiEvent::Device(dev.public_id));
                self.welcomed.store(true, Ordering::Relaxed);
                Ok(vec![ack(&id), welcome.to_text()])
            }
            Frame::Status(s) => {
                let fid = id.clone();
                let changed = app
                    .store
                    .write(move |c| {
                        if !first_sight(c, device_id, &fid, now)? {
                            return Ok(None);
                        }
                        let Some(m) = messages::by_public(c, &s.msg_id)?
                            .filter(|m| m.device_id == device_id)
                        else {
                            return Ok(None);
                        };
                        c.execute(
                            "UPDATE messages SET device_acked_at=COALESCE(device_acked_at, ?2) WHERE id=?1",
                            params![m.id, now],
                        )?;
                        let ev = match s.state {
                            StatusState::Sent => Event::StatusSent,
                            StatusState::Delivered => Event::StatusDelivered,
                            StatusState::Failed => Event::StatusFailed,
                        };
                        let err = s.error.as_ref().map(|e| (e.code.as_str(), e.message.as_str()));
                        messages::apply_event(c, m.id, ev, s.at, s.parts, err, now)
                    })
                    .await?;
                if let Some(m) = changed {
                    app.webhook_notify.notify_one();
                    app.publish(UiEvent::Message(m.public_id));
                }
                self.notify.notify_one();
                Ok(vec![ack(&id)])
            }
            Frame::Inbound(m) => {
                let fid = id.clone();
                let row = app
                    .store
                    .write(move |c| {
                        if !first_sight(c, device_id, &fid, now)? {
                            return Ok(None);
                        }
                        messages::insert_inbound(c, device_id, &m, now)
                    })
                    .await?;
                if let Some(r) = row {
                    app.webhook_notify.notify_one();
                    app.publish(UiEvent::Message(r.public_id));
                }
                Ok(vec![ack(&id)])
            }
            Frame::Health(h) => {
                let fid = id.clone();
                app.store
                    .write(move |c| {
                        if first_sight(c, device_id, &fid, now)? {
                            devices::apply_health(c, device_id, &h, now)?;
                        }
                        Ok(())
                    })
                    .await?;
                app.publish(UiEvent::Device(self.device_public_id.clone()));
                Ok(vec![ack(&id)])
            }
            Frame::Ack(a) => {
                let hit = app
                    .store
                    .write(move |c| messages::ack_frame(c, device_id, &a.reference, now))
                    .await?;
                if hit.is_some() {
                    self.notify.notify_one();
                }
                Ok(vec![])
            }
            Frame::Nack(n) => {
                // The device refused a `send`: never retransmit, fail the message.
                let changed = app
                    .store
                    .write(move |c| {
                        let Some(mid) = messages::ack_frame(c, device_id, &n.reference, now)?
                        else {
                            return Ok(None);
                        };
                        let code = format!("nack_{}", n.code);
                        messages::apply_event(
                            c,
                            mid,
                            Event::StatusFailed,
                            now,
                            None,
                            Some((&code, &n.message)),
                            now,
                        )
                    })
                    .await?;
                if let Some(m) = changed {
                    app.webhook_notify.notify_one();
                    app.publish(UiEvent::Message(m.public_id));
                }
                self.notify.notify_one();
                Ok(vec![])
            }
            Frame::Welcome(_) | Frame::Config(_) | Frame::Send(_) => Ok(vec![nack(
                &id,
                "unsupported",
                "server-to-device frame type",
            )]),
        }
    }

    /// After a `hello` has been answered, start the dispatcher (once per socket).
    fn after_replies(&self) {
        let app = &self.app;
        if !self.welcomed.load(Ordering::Relaxed)
            || app.hub.has_dispatcher(self.device_id, self.generation)
        {
            return;
        }
        let task = tokio::spawn(dispatcher::run(
            app.clone(),
            self.device_id,
            self.notify.clone(),
            self.tx.clone(),
        ));
        app.hub
            .set_dispatcher(self.device_id, self.generation, task);
    }
}
