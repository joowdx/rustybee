//! Shared application context, startup and background tasks.

use crate::config::Config;
use crate::domain::phone;
use crate::error::ApiError;
use crate::fcm::Fcm;
use crate::hub::Hub;
use crate::store::messages::{MessageRow, NewMessage};
use crate::store::{self, Store, devices};
use crate::util::Clock;
use axum::http::StatusCode;
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{Notify, broadcast, oneshot};
use tokio::task::JoinHandle;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const MAX_RECIPIENTS: usize = 1000;
pub const MAX_BODY_CHARS: usize = 1600;

/// Events for the web UI's SSE stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiEvent {
    Device(String),
    Message(String),
}

/// A parsed CSV kept in memory for the bulk flow.
pub struct BulkUpload {
    pub created: Instant,
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

pub struct Ctx {
    pub cfg: Config,
    pub store: Store,
    pub clock: Clock,
    pub hub: Hub,
    pub bus: broadcast::Sender<UiEvent>,
    pub webhook_notify: Notify,
    pub fcm_notify: Notify,
    pub fcm: Option<Arc<Fcm>>,
    pub http: reqwest::Client,
    pub login_failures: Mutex<HashMap<IpAddr, Vec<Instant>>>,
    pub bulk: Mutex<HashMap<String, BulkUpload>>,
    pub otp: Mutex<crate::otp::OtpState>,
}

pub type App = Arc<Ctx>;

/// Why `queue` wrote nothing after validation.
enum Rejected {
    Choice(devices::Choice),
    /// The device's backlog of `queued` messages, which the batch would push over the cap.
    QueueFull(i64),
}

/// One item of a send request, before validation.
pub struct Draft {
    /// Field name used in error messages, e.g. `to[3]`.
    pub label: String,
    pub to: String,
    pub body: String,
}

impl Ctx {
    pub fn publish(&self, ev: UiEvent) {
        let _ = self.bus.send(ev);
    }

    /// Validate and queue messages as one batch (specs 004, 006, 007).
    /// Validation happens before anything is written.
    pub async fn queue(
        &self,
        drafts: Vec<Draft>,
        device: Option<String>,
        sim: Option<i64>,
    ) -> Result<(String, Vec<MessageRow>), ApiError> {
        if drafts.is_empty() || drafts.len() > MAX_RECIPIENTS {
            return Err(ApiError::bad_request(format!(
                "between 1 and {MAX_RECIPIENTS} recipients are required"
            )));
        }
        let mut items = Vec::with_capacity(drafts.len());
        for d in drafts {
            let n = d.body.chars().count();
            if n == 0 {
                return Err(ApiError::bad_request("body must not be empty"));
            }
            if n > MAX_BODY_CHARS {
                return Err(ApiError::new(
                    StatusCode::BAD_REQUEST,
                    "body_too_long",
                    format!("body is {n} characters; the limit is {MAX_BODY_CHARS}"),
                ));
            }
            let to = phone::normalize_with(&d.to, self.cfg.allow_short_codes).map_err(|e| {
                ApiError::new(
                    StatusCode::BAD_REQUEST,
                    "invalid_recipient",
                    format!("{}: {e}", d.label),
                )
            })?;
            items.push(NewMessage { to, body: d.body });
        }
        let now = self.clock.now_ms();
        let max_queued = i64::from(self.cfg.max_queued);
        let res = self
            .store
            .write(move |c| {
                Ok(match devices::choose(c, device.as_deref())? {
                    devices::Choice::Ok(id) => {
                        let queued: i64 = c.query_row(
                            "SELECT count(*) FROM messages WHERE device_id=?1 AND state='queued'",
                            [id],
                            |r| r.get(0),
                        )?;
                        if queued + items.len() as i64 > max_queued {
                            Err(Rejected::QueueFull(queued))
                        } else {
                            Ok((id, store::messages::create_batch(c, id, &items, sim, now)?))
                        }
                    }
                    other => Err(Rejected::Choice(other)),
                })
            })
            .await?;
        let (device_id, (batch, rows)) = res.map_err(|r| match r {
            Rejected::QueueFull(queued) => ApiError::new(
                StatusCode::TOO_MANY_REQUESTS,
                "queue_full",
                format!(
                    "the device already has {queued} queued messages; the limit is {max_queued}"
                ),
            ),
            Rejected::Choice(devices::Choice::NotFound) => {
                ApiError::new(StatusCode::NOT_FOUND, "not_found", "device not found")
            }
            Rejected::Choice(devices::Choice::Disabled) => ApiError::new(
                StatusCode::CONFLICT,
                "device_disabled",
                "device is disabled",
            ),
            Rejected::Choice(_) => ApiError::new(
                StatusCode::BAD_REQUEST,
                "device_required",
                "several devices are enabled and none is the default; pass device_id",
            ),
        })?;
        self.hub.notify(device_id);
        if !self.hub.is_connected(device_id) {
            self.fcm_notify.notify_one();
        }
        for r in &rows {
            self.publish(UiEvent::Message(r.public_id.clone()));
        }
        Ok((batch, rows))
    }

    /// Push the current config to a connected device.
    pub async fn push_config(&self, device_id: i64) -> store::Result<()> {
        if let Some(d) = self
            .store
            .read(move |c| devices::by_id(c, device_id))
            .await?
        {
            let env = crate::protocol::Envelope::new("config", d.config(self.cfg.ping_s));
            self.hub
                .send(device_id, crate::hub::Outgoing::Text(env.to_text()));
            self.hub.notify(device_id);
            self.publish(UiEvent::Device(d.public_id));
        }
        Ok(())
    }

    /// Remove a device: close its socket with 4003, cancel its queued and
    /// dispatched messages and soft-delete it (spec 003).
    pub async fn remove_device(&self, device_id: i64) -> store::Result<()> {
        // Delete first: once the row is gone the token no longer authenticates, so the
        // phone cannot reconnect between the close and the delete.
        let now = self.clock.now_ms();
        self.store
            .write(move |c| devices::delete(c, device_id, now))
            .await?;
        self.hub
            .close(device_id, crate::hub::CLOSE_REVOKED, "revoked");
        self.webhook_notify.notify_one();
        self.publish(UiEvent::Device(String::new()));
        Ok(())
    }

    pub async fn has_password(&self) -> bool {
        self.store
            .read(store::auth::password_hash)
            .await
            .ok()
            .flatten()
            .is_some()
    }
}

pub struct Running {
    pub addr: SocketAddr,
    pub app: App,
    shutdown: Option<oneshot::Sender<()>>,
    server: Option<JoinHandle<()>>,
    tasks: Vec<JoinHandle<()>>,
}

impl Running {
    /// Stop serving, close device sockets and background tasks.
    pub async fn stop(mut self) {
        self.shutdown_inner().await;
    }

    async fn shutdown_inner(&mut self) {
        self.app.hub.close_all(1001, "server shutting down");
        if let Some(s) = self.shutdown.take() {
            let _ = s.send(());
        }
        for t in self.tasks.drain(..) {
            t.abort();
        }
        if let Some(server) = self.server.take() {
            let abort = server.abort_handle();
            if tokio::time::timeout(Duration::from_secs(5), server)
                .await
                .is_err()
            {
                abort.abort();
            }
        }
    }

    /// Wait until the server task ends (after `stop` from a signal handler).
    pub async fn wait_for_signal(mut self) {
        shutdown_signal().await;
        tracing::info!("shutting down");
        self.shutdown_inner().await;
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        for t in &self.tasks {
            t.abort();
        }
        if let Some(s) = &self.server {
            s.abort();
        }
    }
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! { _ = ctrl_c => {}, _ = term => {} }
}

/// Open the store, apply first-run settings, bind and start everything.
pub async fn start(cfg: Config) -> Result<Running, String> {
    let store = Store::open(&cfg.db_path()).map_err(|e| format!("open database: {e}"))?;
    let clock = Clock::default();
    let pw = cfg.admin_password.clone();
    store
        .write(move |c| {
            c.execute("UPDATE devices SET connected=0", [])?;
            if let Some(pw) = pw
                && store::auth::password_hash(c)?.is_none()
            {
                if pw.chars().count() < store::auth::MIN_PASSWORD {
                    tracing::warn!(
                        "RUSTYBEE_ADMIN_PASSWORD is shorter than 10 characters; ignored"
                    );
                } else {
                    store::auth::set_password_hash(c, &store::auth::hash_password(&pw))?;
                    tracing::info!("admin password set from RUSTYBEE_ADMIN_PASSWORD");
                }
            }
            Ok(())
        })
        .await
        .map_err(|e| e.to_string())?;
    let fcm = Fcm::from_config(&cfg)?.map(Arc::new);
    let http = reqwest::Client::builder()
        .user_agent(format!("rustybee/{VERSION}"))
        .connect_timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())?;
    let listener = tokio::net::TcpListener::bind(&cfg.bind)
        .await
        .map_err(|e| format!("bind {}: {e}", cfg.bind))?;
    let addr = listener.local_addr().map_err(|e| e.to_string())?;
    let (bus, _) = broadcast::channel(256);
    let app: App = Arc::new(Ctx {
        cfg,
        store,
        clock,
        hub: Hub::default(),
        bus,
        webhook_notify: Notify::new(),
        fcm_notify: Notify::new(),
        fcm: fcm.clone(),
        http,
        login_failures: Mutex::new(HashMap::new()),
        bulk: Mutex::new(HashMap::new()),
        otp: Mutex::new(Default::default()),
    });
    let mut tasks = vec![
        tokio::spawn(crate::webhooks::run(app.clone())),
        tokio::spawn(crate::sweeper::run(app.clone())),
    ];
    if let Some(f) = fcm {
        tracing::info!("FCM wake enabled");
        tasks.push(tokio::spawn(crate::fcm::run(app.clone(), f)));
    }
    let router = crate::http::router(app.clone());
    let (tx, rx) = oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        let r = axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(async {
            let _ = rx.await;
        })
        .await;
        if let Err(e) = r {
            tracing::error!(error = %e, "server error");
        }
    });
    tracing::info!(%addr, version = VERSION, "listening");
    Ok(Running {
        addr,
        app,
        shutdown: Some(tx),
        server: Some(server),
        tasks,
    })
}
