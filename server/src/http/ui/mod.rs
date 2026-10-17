//! Web UI (spec 010): askama pages, htmx partials and the SSE stream.
// Handlers return `Result<Response, Response>`; both arms are responses.
#![allow(clippy::result_large_err)]

mod admin;
mod pages;

use crate::app::{App, UiEvent};
use crate::assets;
use crate::store::devices::DeviceRow;
use crate::store::messages::MessageRow;
use crate::util::{ago, human_time, short_time};
use askama::Template;
use axum::Router;
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use serde::Deserialize;
use serde_json::Value;
use std::fmt::Display;

pub fn routes() -> Router<App> {
    Router::new()
        .route("/login", get(pages::login_page).post(pages::login))
        .route("/login/code", post(pages::send_code))
        .route("/login/verify", post(pages::verify_code))
        .route("/logout", post(pages::logout))
        .route("/", get(pages::dashboard))
        .route("/partials/dashboard", get(pages::dashboard_partial))
        .route("/events", get(pages::events))
        .route("/send", get(pages::send_page).post(pages::send))
        .route("/bulk", get(pages::bulk_page))
        .route("/bulk/upload", post(pages::bulk_upload))
        .route("/bulk/preview", post(pages::bulk_preview))
        .route("/bulk/send", post(pages::bulk_send))
        .route("/messages", get(pages::messages))
        .route("/messages/rows", get(pages::message_rows))
        .route("/messages/{id}", get(pages::message))
        .route("/messages/{id}/reply", post(pages::reply))
        .route("/devices", get(admin::devices))
        .route("/devices/pair", post(admin::pair))
        .route("/devices/paired", get(admin::paired))
        .route("/devices/{id}", post(admin::save_device))
        .route("/devices/{id}/default", post(admin::make_default))
        .route("/devices/{id}/delete", post(admin::delete_device))
        .route("/keys", get(admin::keys).post(admin::create_key))
        .route("/keys/{id}/rename", post(admin::rename_key))
        .route("/keys/{id}/revoke", post(admin::revoke_key))
        .route("/keys/{id}/delete", post(admin::delete_key))
        .route(
            "/webhooks",
            get(admin::webhooks).post(admin::create_webhook),
        )
        .route(
            "/webhooks/{id}",
            get(admin::webhook).post(admin::update_webhook),
        )
        .route("/webhooks/{id}/rotate", post(admin::rotate_webhook))
        .route("/webhooks/{id}/test", post(admin::test_webhook))
        .route("/webhooks/{id}/delete", post(admin::delete_webhook))
        .route(
            "/webhooks/deliveries/{id}/redeliver",
            post(admin::redeliver),
        )
        .route("/settings", get(admin::settings))
        .route("/settings/password", post(admin::change_password))
        .route("/settings/phone", post(admin::send_phone_code))
        .route("/settings/phone/verify", post(admin::verify_phone))
        .route("/settings/phone/remove", post(admin::remove_phone))
        .route(
            "/settings/sessions/{id}/revoke",
            post(admin::revoke_session),
        )
        .layer(axum::middleware::from_fn(super::auth::csrf))
}

/// Shared page chrome.
pub struct Layout {
    pub title: String,
    pub nav: &'static str,
    pub css: String,
    pub js: String,
    pub htmx: String,
    pub sse: String,
    pub favicon: String,
    pub touch_icon: String,
    pub logo: String,
    pub flash: Option<String>,
    pub error: Option<String>,
}

impl Layout {
    pub fn new(title: impl Into<String>, nav: &'static str) -> Self {
        Layout {
            title: title.into(),
            nav,
            css: assets::url("app.css"),
            js: assets::url("app.js"),
            htmx: assets::url("htmx.min.js"),
            sse: assets::url("sse.js"),
            favicon: assets::url("bee.svg"),
            touch_icon: assets::url("apple-touch-icon.png"),
            logo: assets::url("bee.svg"),
            flash: None,
            error: None,
        }
    }

    pub fn notice(mut self, f: &Notice) -> Self {
        self.flash = f.ok.as_deref().map(flash_text);
        self.error = f.err.clone().filter(|e| !e.is_empty());
        self
    }

    pub fn err(mut self, e: impl Into<String>) -> Self {
        self.error = Some(e.into());
        self
    }
}

/// `?ok=` / `?err=` set by a redirect after a POST.
#[derive(Deserialize, Default)]
pub struct Notice {
    pub ok: Option<String>,
    pub err: Option<String>,
}

fn flash_text(key: &str) -> String {
    if let Some(code) = key.strip_prefix("test_") {
        return format!("The test delivery was answered with HTTP {code}.");
    }
    match key {
        "saved" => "Saved.",
        "default" => "Default device updated.",
        "removed" => "Device removed.",
        "renamed" => "Key renamed.",
        "revoked" => "Key revoked.",
        "deleted" => "Deleted.",
        "created" => "Created.",
        "rotated" => "Secret rotated. Update your receiver.",
        "redeliver" => "Delivery queued again.",
        "reply" => "Reply queued.",
        "password" => "Password changed. Other sessions were signed out.",
        "session" => "Browser signed out.",
        "phone_code" => "Code sent. Enter it below to confirm the number.",
        "phone" => "Phone confirmed. It can now receive login codes.",
        "phone_removed" => "Phone removed.",
        _ => "Done.",
    }
    .to_string()
}

pub type Res = Result<Response, Response>;

pub fn render<T: Template>(t: &T) -> Response {
    match t.render() {
        Ok(s) => Html(s).into_response(),
        Err(e) => ise(e),
    }
}

pub fn ise(e: impl Display) -> Response {
    tracing::error!(error = %e, "ui error");
    (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
}

pub fn not_found() -> Response {
    (StatusCode::NOT_FOUND, "not found").into_response()
}

pub fn see_other(to: &str) -> Response {
    Redirect::to(to).into_response()
}

pub fn back_err(to: &str, msg: impl Display) -> Response {
    let sep = if to.contains('?') { '&' } else { '?' };
    see_other(&format!("{to}{sep}err={}", pct(&msg.to_string())))
}

/// Percent-encode everything but RFC 3986 unreserved characters.
pub fn pct(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

pub fn on(v: &Option<String>) -> bool {
    v.as_deref().is_some_and(|s| !s.is_empty() && s != "off")
}

pub struct Opt {
    pub value: String,
    pub label: String,
    pub selected: bool,
}

pub struct DevOpt {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub selected: bool,
}

pub fn device_opts(devs: &[DeviceRow], selected: &str) -> Vec<DevOpt> {
    devs.iter()
        .map(|d| DevOpt {
            id: d.public_id.clone(),
            name: d.name.clone(),
            enabled: d.enabled,
            selected: d.public_id == selected,
        })
        .collect()
}

/// SIM choices of one device, labelled for a select.
pub fn sims_of(d: &DeviceRow, selected: Option<i64>, prefix: bool) -> Vec<Opt> {
    let Some(list) = d.sims.as_array() else {
        return vec![];
    };
    list.iter()
        .filter_map(|s| {
            let id = s.get("subscription_id")?.as_i64()?;
            let slot = s.get("slot").and_then(Value::as_i64);
            let carrier = s.get("carrier").and_then(Value::as_str).unwrap_or("");
            let mut label = match slot {
                Some(n) => format!("SIM {}", n + 1),
                None => format!("SIM {id}"),
            };
            if !carrier.is_empty() {
                label = format!("{label} · {carrier}");
            }
            if prefix {
                label = format!("{}: {label}", d.name);
            }
            Some(Opt {
                value: id.to_string(),
                label,
                selected: selected == Some(id),
            })
        })
        .collect()
}

pub fn sim_opts(devs: &[DeviceRow], selected: Option<i64>) -> Vec<Opt> {
    let mut out: Vec<Opt> = Vec::new();
    for d in devs {
        for o in sims_of(d, selected, devs.len() > 1) {
            if !out.iter().any(|x| x.value == o.value) {
                out.push(o);
            }
        }
    }
    out
}

pub struct RowView {
    pub id: String,
    pub out: bool,
    pub peer: String,
    pub body: String,
    pub state: &'static str,
    pub device: String,
    pub created: String,
    pub when: String,
}

impl RowView {
    pub fn from(m: &MessageRow) -> RowView {
        RowView {
            id: m.public_id.clone(),
            out: m.direction != "in",
            peer: m.peer.clone(),
            body: m.body.chars().take(140).collect(),
            state: m.state.as_str(),
            device: m.device_label(),
            created: human_time(m.created_at),
            when: short_time(m.created_at),
        }
    }
}

pub fn last_seen(now: i64, d: &DeviceRow) -> String {
    if d.connected {
        "now".into()
    } else {
        d.last_seen_at
            .map(|t| ago(now, t))
            .unwrap_or_else(|| "never".into())
    }
}

/// What a device card shows for connection, wake failure, and the last process start.
pub struct DeviceFace {
    /// `on`, `unreachable`, `off`, or none for a plain offline dot.
    pub status_class: Option<&'static str>,
    pub status_text: String,
    pub unreachable: Option<String>,
    pub restart: Option<String>,
    pub restart_title: Option<String>,
    pub warnings: Vec<String>,
}

pub fn device_face(now: i64, d: &DeviceRow) -> DeviceFace {
    // A live socket is online. A phone the user switched off is not "offline",
    // and that state wins over a stale unreachable flag.
    let (status_class, status_text, unreachable) = if d.connected {
        (Some("on"), "Online".into(), None)
    } else if let Some(t) = d.gateway_off_at {
        (
            Some("off"),
            format!("Gateway switched off on the phone since {}", ago(now, t)),
            None,
        )
    } else if let Some(t) = d.unreachable_at {
        (
            Some("unreachable"),
            format!("Seen {}", last_seen(now, d)),
            Some(format!(
                "Not answering wake-ups since {}. It was probably force-stopped or blocked by the phone's battery manager. Open the app on the phone.",
                ago(now, t)
            )),
        )
    } else {
        (None, format!("Seen {}", last_seen(now, d)), None)
    };
    let (restart, restart_title) = restart_line(now, d.lifecycle_json.as_deref());
    DeviceFace {
        status_class,
        status_text,
        unreachable,
        restart,
        restart_title,
        warnings: permission_warnings(d),
    }
}

fn notifications_off(d: &DeviceRow) -> bool {
    d.telemetry
        .pointer("/permissions/notifications")
        .and_then(Value::as_bool)
        == Some(false)
}

fn permission_warnings(d: &DeviceRow) -> Vec<String> {
    let mut warnings: Vec<String> = d
        .missing_permissions()
        .into_iter()
        .map(|p| format!("Missing permission: {p}"))
        .collect();
    // gms phones are the ones that carry an FCM token. The specific sentence
    // replaces the generic missing-permission line for notifications.
    if d.fcm_token.is_some() && notifications_off(d) {
        warnings.retain(|w| w != "Missing permission: notifications");
        warnings.push("Notifications are off on the phone, so wake-ups will not work".into());
    }
    warnings
}

fn reason_words(code: &str) -> &str {
    match code {
        "push" => "wake-up push",
        "job" => "watchdog",
        "boot_complete" => "phone boot",
        "low_memory" => "low memory",
        "user_requested" => "stopped from settings",
        other => other,
    }
}

/// "Last restart …" from `lifecycle_json`, or nothing when it is missing.
fn restart_line(now: i64, raw: Option<&str>) -> (Option<String>, Option<String>) {
    let Some(raw) = raw else {
        return (None, None);
    };
    let Ok(v) = serde_json::from_str::<Value>(raw) else {
        return (None, None);
    };
    let Some(reason) = v.pointer("/start/reason").and_then(Value::as_str) else {
        return (None, None);
    };
    let Some(at) = v.pointer("/start/at").and_then(Value::as_i64) else {
        return (None, None);
    };
    // The app sends `at=0` when the OS gave no launch timestamp.
    let mut text = if at > 0 {
        format!("Last restart {}: {}", ago(now, at), reason_words(reason))
    } else {
        format!("Last restart: {}", reason_words(reason))
    };
    let mut title = None;
    if let Some(exit_reason) = v.pointer("/exit/reason").and_then(Value::as_str) {
        text.push_str(", after ");
        text.push_str(reason_words(exit_reason));
        title = v
            .pointer("/exit/description")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
    }
    if v.pointer("/start/was_force_stopped")
        .and_then(Value::as_bool)
        == Some(true)
    {
        text.push_str(", was force-stopped");
    }
    (Some(text), title)
}

pub fn sse_event(ev: UiEvent) -> axum::response::sse::Event {
    let (name, id) = match ev {
        UiEvent::Device(id) => ("device", id),
        UiEvent::Message(id) => ("message", id),
    };
    axum::response::sse::Event::default()
        .event(name)
        .data(if id.is_empty() { "-".to_string() } else { id })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pct_encodes_reserved() {
        assert_eq!(pct("a b&c=d/é"), "a%20b%26c%3Dd%2F%C3%A9");
        assert_eq!(pct("AZaz09-._~"), "AZaz09-._~");
    }

    #[test]
    fn flash_keys() {
        assert_eq!(
            flash_text("test_204"),
            "The test delivery was answered with HTTP 204."
        );
        assert_eq!(flash_text("saved"), "Saved.");
    }
}
