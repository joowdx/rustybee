//! Devices, API keys, webhooks and settings pages.

use super::pages::delivery_last;
use super::{
    Layout, Notice, Opt, Res, back_err, device_face, ise, not_found, on, pct, render, see_other,
    sims_of,
};
use crate::app::{App, VERSION};
use crate::domain::filters::{Filters, Mode, Rule};
use crate::http::auth::Session;
use crate::otp;
use crate::store::{auth, devices, devices::DeviceRow, webhooks};
use crate::util::{ago, human_time, short_time};
use askama::Template;
use axum::Form;
use axum::extract::{ConnectInfo, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use qrcode::QrCode;
use qrcode::render::svg;
use serde::Deserialize;
use std::collections::HashMap;
use std::net::SocketAddr;

// ---------- devices ----------

struct DeviceView {
    id: String,
    name: String,
    model: String,
    status_class: Option<&'static str>,
    status_text: String,
    unreachable: Option<String>,
    restart: Option<String>,
    restart_title: Option<String>,
    warnings: Vec<String>,
    battery_pct: Option<i64>,
    charging: bool,
    net: Vec<devices::NetChip>,
    is_default: bool,
    enabled: bool,
    send_delay_ms: i64,
    receive_enabled: bool,
    filters_rev: i64,
    sims: Vec<Opt>,
    filter_mode: String,
    rules: Vec<RuleView>,
}

struct RuleView {
    field: &'static str,
    op: &'static str,
    value: String,
    ignore_case: bool,
}

struct PairView {
    qr: String,
    link: String,
    code: String,
    expires: i64,
    count: usize,
}

#[derive(Template)]
#[template(path = "devices.html")]
struct DevicesPage {
    l: Layout,
    pair: Option<PairView>,
    devices: Vec<DeviceView>,
}

fn device_view(now: i64, d: &DeviceRow) -> DeviceView {
    let face = device_face(now, d);
    DeviceView {
        id: d.public_id.clone(),
        name: d.name.clone(),
        model: [d.manufacturer.clone(), d.model.clone()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" "),
        status_class: face.status_class,
        status_text: face.status_text,
        unreachable: face.unreachable,
        restart: face.restart,
        restart_title: face.restart_title,
        warnings: face.warnings,
        battery_pct: d.battery_level().map(|b| b.clamp(0, 100)),
        charging: d.charging().unwrap_or(false),
        net: d.net_chips(),
        is_default: d.is_default,
        enabled: d.enabled,
        send_delay_ms: d.send_delay_ms,
        receive_enabled: d.receive_enabled,
        filters_rev: d.filters_rev,
        sims: sims_of(d, d.sim, false),
        filter_mode: match d.filters.mode {
            Mode::Off => "off",
            Mode::Allow => "allow",
            Mode::Block => "block",
        }
        .into(),
        rules: d
            .filters
            .rules
            .iter()
            .map(|r| RuleView {
                field: r.field.as_str(),
                op: r.op.as_str(),
                value: r.value.clone(),
                ignore_case: r.ignore_case,
            })
            .collect(),
    }
}

async fn devices_page(app: &App, l: Layout, pair: Option<PairView>) -> Res {
    let devs = app.store.read(devices::list).await.map_err(ise)?;
    let now = app.clock.now_ms();
    Ok(render(&DevicesPage {
        l,
        pair,
        devices: devs.iter().map(|d| device_view(now, d)).collect(),
    }))
}

pub async fn devices(_: Session, State(app): State<App>, Query(n): Query<Notice>) -> Res {
    devices_page(&app, Layout::new("Devices", "devices").notice(&n), None).await
}

/// Deep link the app accepts (spec 002): `rustybee://pair?u=<server>&c=<code>`.
pub fn pair_link(public_url: &str, code: &str) -> String {
    format!(
        "rustybee://pair?u={}&c={code}",
        pct(public_url.trim_end_matches('/'))
    )
}

pub async fn pair(_: Session, State(app): State<App>) -> Res {
    let now = app.clock.now_ms();
    let (code, expires, count) = app
        .store
        .write(move |c| {
            let (code, exp) = auth::create_pairing_code(c, now)?;
            Ok((code, exp, devices::list(c)?.len()))
        })
        .await
        .map_err(ise)?;
    let link = pair_link(&app.cfg.public_url, &code);
    let qr = QrCode::new(link.as_bytes())
        .map_err(ise)?
        .render::<svg::Color>()
        .min_dimensions(200, 200)
        .quiet_zone(true)
        .build();
    // The page countdown uses the browser clock; send the remaining time relative to it.
    let pair = PairView {
        qr,
        link,
        code,
        expires,
        count,
    };
    devices_page(&app, Layout::new("Devices", "devices"), Some(pair)).await
}

#[derive(Deserialize)]
pub struct PairedQuery {
    #[serde(default)]
    n: usize,
}

/// Polled while the pairing panel is open; refreshes the page once a device appears.
pub async fn paired(_: Session, State(app): State<App>, Query(q): Query<PairedQuery>) -> Res {
    let count = app
        .store
        .read(|c| Ok(devices::list(c)?.len()))
        .await
        .map_err(ise)?;
    if count > q.n {
        Ok(([("HX-Refresh", "true")], StatusCode::OK).into_response())
    } else {
        Ok(StatusCode::NO_CONTENT.into_response())
    }
}

async fn device_by_public(app: &App, id: String) -> Result<DeviceRow, Response> {
    app.store
        .read(move |c| devices::by_public(c, &id))
        .await
        .map_err(ise)?
        .ok_or_else(not_found)
}

/// The device settings form. Filter rules come as parallel lists, one entry per row:
/// `rule_field`, `rule_op`, `rule_value` and `rule_case` (`ignore` or `match`).
struct DeviceForm(Vec<(String, String)>);

impl DeviceForm {
    fn get(&self, key: &str) -> &str {
        self.0.iter().find(|(k, _)| k == key).map_or("", |(_, v)| v)
    }

    fn on(&self, key: &str) -> bool {
        !matches!(self.get(key), "" | "off")
    }

    fn all(&self, key: &str) -> Vec<&str> {
        self.0
            .iter()
            .filter(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
            .collect()
    }

    fn rules(&self) -> Result<Vec<Rule>, String> {
        let (fields, ops, values, cases) = (
            self.all("rule_field"),
            self.all("rule_op"),
            self.all("rule_value"),
            self.all("rule_case"),
        );
        if ops.len() != fields.len() || values.len() != fields.len() || cases.len() != fields.len()
        {
            return Err("the rule rows are incomplete; reload the page".into());
        }
        let mut rules = Vec::new();
        for (i, field) in fields.iter().enumerate() {
            let ignore = cases[i] != "match";
            if let Some(r) = Rule::from_row(field, ops[i], values[i], ignore)
                .map_err(|e| format!("rule {}: {e}", i + 1))?
            {
                rules.push(r);
            }
        }
        Ok(rules)
    }
}

pub async fn save_device(
    _: Session,
    State(app): State<App>,
    Path(id): Path<String>,
    Form(pairs): Form<Vec<(String, String)>>,
) -> Res {
    let f = DeviceForm(pairs);
    let d = device_by_public(&app, id).await?;
    let name = f.get("name").trim().to_string();
    if name.is_empty() || name.chars().count() > 64 {
        return Ok(back_err("/devices", "the name needs 1 to 64 characters"));
    }
    let delay = match f.get("send_delay_ms").trim() {
        "" => 0,
        s => match s.parse::<i64>() {
            Ok(v) if (0..=crate::domain::pacing::MAX_DELAY_MS).contains(&v) => v,
            _ => return Ok(back_err("/devices", "send delay must be 0 to 3600000 ms")),
        },
    };
    let mode = match f.get("filter_mode") {
        "" | "off" => Mode::Off,
        "allow" => Mode::Allow,
        "block" => Mode::Block,
        _ => return Ok(back_err("/devices", "unknown filter mode")),
    };
    let rules = match f.rules() {
        Ok(r) => r,
        Err(e) => return Ok(back_err("/devices", format!("filter rules: {e}"))),
    };
    let s = devices::Settings {
        name,
        enabled: f.on("enabled"),
        send_delay_ms: delay,
        // Only ever turns forwarding off; see `update_settings`.
        receive_enabled: f.on("receive_enabled"),
        sim: f.get("sim").trim().parse().ok(),
        filters: Filters { mode, rules },
        // A form without a rev cannot show it saw the current filters.
        base_rev: f.get("filters_rev").trim().parse().unwrap_or(-1),
    };
    if let Err(e) = s.filters.validate() {
        return Ok(back_err("/devices", format!("filter rules: {e}")));
    }
    let did = d.id;
    let conflict = app
        .store
        .write(move |c| devices::update_settings(c, did, &s))
        .await
        .map_err(ise)?;
    app.push_config(did).await.map_err(ise)?;
    if conflict {
        return Ok(back_err(
            "/devices",
            "The filters were changed on the phone. Reload to see them.",
        ));
    }
    Ok(see_other("/devices?ok=saved"))
}

pub async fn make_default(_: Session, State(app): State<App>, Path(id): Path<String>) -> Res {
    let d = device_by_public(&app, id).await?;
    let did = d.id;
    app.store
        .write(move |c| devices::set_default(c, did))
        .await
        .map_err(ise)?;
    app.publish(crate::app::UiEvent::Device(d.public_id));
    Ok(see_other("/devices?ok=default"))
}

#[derive(Deserialize)]
pub struct ConfirmForm {
    confirm: Option<String>,
}

pub async fn delete_device(
    _: Session,
    State(app): State<App>,
    Path(id): Path<String>,
    Form(f): Form<ConfirmForm>,
) -> Res {
    if !on(&f.confirm) {
        return Ok(back_err("/devices", "tick Confirm to remove a device"));
    }
    let d = device_by_public(&app, id).await?;
    app.remove_device(d.id).await.map_err(ise)?;
    Ok(see_other("/devices?ok=removed"))
}

// ---------- API keys ----------

struct KeyView {
    id: String,
    name: String,
    prefix: String,
    last_used: String,
    revoked: bool,
}

#[derive(Template)]
#[template(path = "keys.html")]
struct KeysPage {
    l: Layout,
    created: Option<(String, String)>,
    keys: Vec<KeyView>,
    base: String,
}

async fn keys_page(app: &App, l: Layout, created: Option<(String, String)>) -> Res {
    let keys = app.store.read(auth::list_keys).await.map_err(ise)?;
    Ok(render(&KeysPage {
        l,
        created,
        keys: keys
            .into_iter()
            .map(|k| KeyView {
                id: k.public_id,
                name: k.name,
                prefix: k.prefix,
                last_used: k
                    .last_used_at
                    .map(short_time)
                    .unwrap_or_else(|| "never".into()),
                revoked: k.revoked_at.is_some(),
            })
            .collect(),
        base: app.cfg.public_url.trim_end_matches('/').to_string(),
    }))
}

pub async fn keys(_: Session, State(app): State<App>, Query(n): Query<Notice>) -> Res {
    keys_page(&app, Layout::new("API keys", "keys").notice(&n), None).await
}

#[derive(Deserialize)]
pub struct NameForm {
    #[serde(default)]
    name: String,
}

fn valid_name(name: &str) -> Option<String> {
    let n = name.trim();
    (!n.is_empty() && n.chars().count() <= 64).then(|| n.to_string())
}

pub async fn create_key(_: Session, State(app): State<App>, Form(f): Form<NameForm>) -> Res {
    let Some(name) = valid_name(&f.name) else {
        return Ok(back_err("/keys", "the name needs 1 to 64 characters"));
    };
    let now = app.clock.now_ms();
    let (row, secret) = app
        .store
        .write(move |c| auth::create_key(c, &name, now))
        .await
        .map_err(ise)?;
    keys_page(
        &app,
        Layout::new("API keys", "keys"),
        Some((row.name, secret)),
    )
    .await
}

fn found(n: usize) -> Result<(), Response> {
    if n == 0 { Err(not_found()) } else { Ok(()) }
}

pub async fn rename_key(
    _: Session,
    State(app): State<App>,
    Path(id): Path<String>,
    Form(f): Form<NameForm>,
) -> Res {
    let Some(name) = valid_name(&f.name) else {
        return Ok(back_err("/keys", "the name needs 1 to 64 characters"));
    };
    let n = app
        .store
        .write(move |c| auth::rename_key(c, &id, &name))
        .await
        .map_err(ise)?;
    found(n)?;
    Ok(see_other("/keys?ok=renamed"))
}

pub async fn revoke_key(_: Session, State(app): State<App>, Path(id): Path<String>) -> Res {
    let now = app.clock.now_ms();
    let n = app
        .store
        .write(move |c| auth::revoke_key(c, &id, now))
        .await
        .map_err(ise)?;
    found(n)?;
    Ok(see_other("/keys?ok=revoked"))
}

pub async fn delete_key(_: Session, State(app): State<App>, Path(id): Path<String>) -> Res {
    let n = app
        .store
        .write(move |c| auth::delete_key(c, &id))
        .await
        .map_err(ise)?;
    found(n)?;
    Ok(see_other("/keys?ok=deleted"))
}

// ---------- webhooks ----------

struct EventOpt {
    key: &'static str,
    name: &'static str,
    on: bool,
}

struct WebhookForm {
    name: String,
    url: String,
    events: Vec<EventOpt>,
    active: bool,
}

impl WebhookForm {
    fn new(name: &str, url: &str, events: &[String], active: bool) -> Self {
        WebhookForm {
            name: name.into(),
            url: url.into(),
            events: webhooks::EVENTS
                .iter()
                .map(|e| EventOpt {
                    key: e.strip_prefix("message.").unwrap_or(e),
                    name: e,
                    on: events.iter().any(|x| x == e),
                })
                .collect(),
            active,
        }
    }
}

struct HookView {
    id: String,
    name: String,
    url: String,
    events: String,
    active: bool,
}

#[derive(Template)]
#[template(path = "webhooks.html")]
struct WebhooksPage {
    l: Layout,
    hooks: Vec<HookView>,
    form: WebhookForm,
}

pub async fn webhooks(_: Session, State(app): State<App>, Query(n): Query<Notice>) -> Res {
    let hooks = app.store.read(webhooks::list).await.map_err(ise)?;
    let all: Vec<String> = webhooks::EVENTS.iter().map(|e| e.to_string()).collect();
    Ok(render(&WebhooksPage {
        l: Layout::new("Webhooks", "webhooks").notice(&n),
        hooks: hooks
            .into_iter()
            .map(|h| HookView {
                id: h.public_id,
                name: h.name,
                url: h.url,
                events: h.events.join(", "),
                active: h.active,
            })
            .collect(),
        form: WebhookForm::new("", "", &all, true),
    }))
}

/// Parse the webhook form; `ev_<suffix>` checkboxes select events.
fn parse_hook(f: &HashMap<String, String>) -> Result<(String, String, Vec<String>, bool), String> {
    let name = valid_name(f.get("name").map(String::as_str).unwrap_or(""))
        .ok_or("the name needs 1 to 64 characters")?;
    let url = f.get("url").map(|s| s.trim()).unwrap_or("").to_string();
    if !(url.starts_with("https://") || url.starts_with("http://")) || url.len() > 2048 {
        return Err("the URL must start with http:// or https://".into());
    }
    let events: Vec<String> = webhooks::EVENTS
        .iter()
        .filter(|e| {
            let key = format!("ev_{}", e.strip_prefix("message.").unwrap_or(e));
            on(&f.get(&key).cloned())
        })
        .map(|e| e.to_string())
        .collect();
    if events.is_empty() {
        return Err("pick at least one event".into());
    }
    Ok((name, url, events, on(&f.get("active").cloned())))
}

pub async fn create_webhook(
    _: Session,
    State(app): State<App>,
    Form(f): Form<HashMap<String, String>>,
) -> Res {
    let (name, url, events, active) = match parse_hook(&f) {
        Ok(v) => v,
        Err(e) => return Ok(back_err("/webhooks", e)),
    };
    let now = app.clock.now_ms();
    let row = app
        .store
        .write(move |c| webhooks::create(c, &name, &url, &events, active, now))
        .await
        .map_err(ise)?;
    Ok(see_other(&format!(
        "/webhooks/{}?ok=created",
        row.public_id
    )))
}

struct DeliveryView {
    id: String,
    event: String,
    message: Option<String>,
    created: String,
    status: &'static str,
    attempts: i64,
    last: String,
    payload: String,
}

#[derive(Template)]
#[template(path = "webhook.html")]
struct WebhookPage {
    l: Layout,
    id: String,
    secret: String,
    form: WebhookForm,
    deliveries: Vec<DeliveryView>,
    older: Option<i64>,
}

#[derive(Deserialize)]
pub struct HookQuery {
    ok: Option<String>,
    err: Option<String>,
    before: Option<i64>,
}

const DELIVERY_PAGE: i64 = 20;

pub async fn webhook(
    _: Session,
    State(app): State<App>,
    Path(id): Path<String>,
    Query(q): Query<HookQuery>,
) -> Res {
    let before = q.before;
    let found = app
        .store
        .read(move |c| {
            let Some(h) = webhooks::by_public(c, &id)? else {
                return Ok(None);
            };
            let d = webhooks::deliveries_for_webhook(c, h.id, before, DELIVERY_PAGE)?;
            Ok(Some((h, d)))
        })
        .await
        .map_err(ise)?;
    let Some((h, dels)) = found else {
        return Err(not_found());
    };
    let older = (dels.len() as i64 == DELIVERY_PAGE)
        .then(|| dels.last().map(|d| d.id))
        .flatten();
    let notice = Notice {
        ok: q.ok,
        err: q.err,
    };
    Ok(render(&WebhookPage {
        l: Layout::new(format!("Webhook {}", h.name), "webhooks").notice(&notice),
        id: h.public_id.clone(),
        secret: h.secret.clone(),
        form: WebhookForm::new(&h.name, &h.url, &h.events, h.active),
        deliveries: dels
            .iter()
            .map(|d| DeliveryView {
                id: d.public_id.clone(),
                event: d.event.clone(),
                message: d.message_public_id.clone(),
                created: human_time(d.created_at),
                status: d.status_label(),
                attempts: d.attempts,
                last: delivery_last(d),
                payload: d.payload.clone(),
            })
            .collect(),
        older,
    }))
}

pub async fn update_webhook(
    _: Session,
    State(app): State<App>,
    Path(id): Path<String>,
    Form(f): Form<HashMap<String, String>>,
) -> Res {
    let back = format!("/webhooks/{id}");
    let (name, url, events, active) = match parse_hook(&f) {
        Ok(v) => v,
        Err(e) => return Ok(back_err(&back, e)),
    };
    let n = app
        .store
        .write(move |c| webhooks::update(c, &id, &name, &url, &events, active))
        .await
        .map_err(ise)?;
    found(n)?;
    // Re-enabling resumes the hook's pending deliveries (spec 008).
    app.webhook_notify.notify_one();
    Ok(see_other(&format!("{back}?ok=saved")))
}

pub async fn rotate_webhook(_: Session, State(app): State<App>, Path(id): Path<String>) -> Res {
    let back = format!("/webhooks/{id}");
    let n = app
        .store
        .write(move |c| webhooks::rotate_secret(c, &id))
        .await
        .map_err(ise)?;
    found(n)?;
    Ok(see_other(&format!("{back}?ok=rotated")))
}

pub async fn test_webhook(_: Session, State(app): State<App>, Path(id): Path<String>) -> Res {
    let back = format!("/webhooks/{id}");
    let h = app
        .store
        .read(move |c| webhooks::by_public(c, &id))
        .await
        .map_err(ise)?
        .ok_or_else(not_found)?;
    Ok(match crate::webhooks::send_test(&app, &h).await {
        Ok(code) => see_other(&format!("{back}?ok=test_{code}")),
        Err(e) => back_err(&back, format!("test delivery failed: {e}")),
    })
}

pub async fn delete_webhook(_: Session, State(app): State<App>, Path(id): Path<String>) -> Res {
    let n = app
        .store
        .write(move |c| webhooks::delete(c, &id))
        .await
        .map_err(ise)?;
    found(n)?;
    Ok(see_other("/webhooks?ok=deleted"))
}

pub async fn redeliver(_: Session, State(app): State<App>, Path(id): Path<String>) -> Res {
    let now = app.clock.now_ms();
    let hook = app
        .store
        .write(move |c| {
            let Some(d) = webhooks::delivery(c, &id)? else {
                return Ok(None);
            };
            webhooks::redeliver(c, &id, now)?;
            Ok(Some(d.webhook_public_id))
        })
        .await
        .map_err(ise)?
        .ok_or_else(not_found)?;
    app.webhook_notify.notify_one();
    Ok(see_other(&format!("/webhooks/{hook}?ok=redeliver")))
}

// ---------- settings ----------

struct SessionView {
    id: i64,
    agent: String,
    browser: String,
    mobile: bool,
    created: String,
    last_seen: String,
    current: bool,
}

/// A readable name for a session's user agent, like "Chrome 155 on macOS",
/// and whether it is a phone or tablet.
fn browser_label(ua: &str) -> (String, bool) {
    let ver = |tok: &str| -> Option<String> {
        let rest = &ua[ua.find(tok)? + tok.len()..];
        Some(rest.chars().take_while(|c| c.is_ascii_digit()).collect())
    };
    let browser = [
        ("Edg/", "Edge"),
        ("OPR/", "Opera"),
        ("SamsungBrowser/", "Samsung Internet"),
        ("Firefox/", "Firefox"),
        ("FxiOS/", "Firefox"),
        ("CriOS/", "Chrome"),
        ("Chrome/", "Chrome"),
    ]
    .iter()
    .find_map(|(t, n)| ver(t).map(|v| format!("{n} {v}")))
    .or_else(|| {
        ua.contains("Safari/")
            .then(|| ver("Version/").map_or("Safari".into(), |v| format!("Safari {v}")))
    });
    let os = [
        ("Android", "Android"),
        ("iPhone", "iOS"),
        ("iPad", "iPadOS"),
        ("CrOS", "ChromeOS"),
        ("Mac OS X", "macOS"),
        ("Windows", "Windows"),
        ("Linux", "Linux"),
    ]
    .iter()
    .find(|(t, _)| ua.contains(t))
    .map(|(_, n)| *n);
    let mobile = matches!(os, Some("Android" | "iOS" | "iPadOS"));
    let label = match (browser, os) {
        (Some(b), Some(o)) => format!("{b} on {o}"),
        (Some(b), None) => b,
        (None, Some(o)) => format!("Browser on {o}"),
        (None, None) => ua
            .split([' ', '/'])
            .find(|s| !s.is_empty())
            .unwrap_or("Unknown browser")
            .to_string(),
    };
    (label, mobile)
}

#[derive(Template)]
#[template(path = "settings.html")]
struct SettingsPage {
    l: Layout,
    phones: Vec<String>,
    phone_pending: bool,
    online: bool,
    /// Whether the password works right now, and why (spec 005).
    password_open: bool,
    password_note: String,
    sessions: Vec<SessionView>,
    version: &'static str,
}

pub async fn settings(s: Session, State(app): State<App>, Query(n): Query<Notice>) -> Res {
    let now = app.clock.now_ms();
    let list = app
        .store
        .read(move |c| Ok((auth::list_sessions(c, now)?, otp::admin_phones(c)?)))
        .await
        .map_err(ise)?;
    let (list, phones) = list;
    let m = otp::methods(&app).await.map_err(ise)?;
    let max = app.cfg.otp_max_failures;
    let password_note = if !m.has_password {
        "No password is set. Run rustybee set-password on the server.".to_string()
    } else if phones.is_empty() {
        "Works until a login phone is confirmed.".to_string()
    } else if !m.online {
        "Works now: no device is online to text a code.".to_string()
    } else if m.failures >= max {
        format!("Works now: the last {} codes failed to send.", m.failures)
    } else {
        format!(
            "Off while a device can text a code. Opens if every device goes offline or {} codes in a row fail to send{}.",
            max,
            if m.failures > 0 {
                format!(" ({} so far)", m.failures)
            } else {
                String::new()
            }
        )
    };
    let online = m.online;
    Ok(render(&SettingsPage {
        password_open: m.password,
        password_note,
        l: Layout::new("Settings", "settings").notice(&n),
        phones,
        phone_pending: otp::pending(&app, otp::Purpose::Verify),
        online,
        sessions: list
            .into_iter()
            .map(|r| {
                let agent = r.user_agent.unwrap_or_default();
                let (browser, mobile) = browser_label(&agent);
                SessionView {
                    id: r.id,
                    agent,
                    browser,
                    mobile,
                    created: short_time(r.created_at),
                    last_seen: if r.id == s.id {
                        "now".into()
                    } else {
                        ago(now, r.last_seen_at)
                    },
                    current: r.id == s.id,
                }
            })
            .collect(),
        version: VERSION,
    }))
}

#[derive(Deserialize)]
pub struct PasswordForm {
    #[serde(default)]
    current: String,
    #[serde(default)]
    new: String,
    #[serde(default)]
    repeat: String,
}

pub async fn change_password(
    s: Session,
    State(app): State<App>,
    Form(f): Form<PasswordForm>,
) -> Res {
    if f.new != f.repeat {
        return Ok(back_err("/settings", "the new passwords do not match"));
    }
    if f.new.chars().count() < auth::MIN_PASSWORD {
        return Ok(back_err(
            "/settings",
            format!(
                "the password needs at least {} characters",
                auth::MIN_PASSWORD
            ),
        ));
    }
    let hash = app
        .store
        .read(auth::password_hash)
        .await
        .map_err(ise)?
        .unwrap_or_default();
    let current = f.current;
    let new = f.new;
    let res = tokio::task::spawn_blocking(move || {
        auth::verify_password(&hash, &current).then(|| auth::hash_password(&new))
    })
    .await
    .map_err(ise)?;
    let Some(new_hash) = res else {
        return Ok(back_err("/settings", "the current password is wrong"));
    };
    let keep = s.id;
    app.store
        .write(move |c| {
            auth::set_password_hash(c, &new_hash)?;
            auth::delete_other_sessions(c, keep)
        })
        .await
        .map_err(ise)?;
    Ok(see_other("/settings?ok=password"))
}

#[derive(Deserialize)]
pub struct PhoneForm {
    #[serde(default)]
    phone: String,
}

/// Text a code to a new admin phone; it is saved once the code is entered.
pub async fn send_phone_code(
    _: Session,
    State(app): State<App>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Form(f): Form<PhoneForm>,
) -> Res {
    let ip = crate::http::client_ip(&app, &headers, Some(peer));
    match otp::send(&app, otp::Purpose::Verify, &f.phone, ip).await {
        Ok(()) => Ok(see_other("/settings?ok=phone_code")),
        Err(otp::SendError::Store(e)) => Err(ise(e)),
        Err(e) => Ok(back_err("/settings", e)),
    }
}

#[derive(Deserialize)]
pub struct CodeForm {
    #[serde(default)]
    code: String,
}

pub async fn verify_phone(
    _: Session,
    State(app): State<App>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Form(f): Form<CodeForm>,
) -> Res {
    let ip = crate::http::client_ip(&app, &headers, Some(peer));
    match otp::verify(&app, otp::Purpose::Verify, &f.code, ip) {
        otp::Verify::Ok(phone) => {
            app.store
                .write(move |c| {
                    let added = otp::add_admin_phone(c, &phone)?;
                    otp::reset_failures(c)?;
                    Ok(added)
                })
                .await
                .map_err(ise)?;
            Ok(see_other("/settings?ok=phone"))
        }
        otp::Verify::Wrong => Ok(back_err("/settings", "wrong code")),
        otp::Verify::Expired => Ok(back_err("/settings", "that code expired; send a new one")),
    }
}

pub async fn remove_phone(_: Session, State(app): State<App>, Form(f): Form<PhoneForm>) -> Res {
    app.store
        .write(move |c| otp::remove_admin_phone(c, &f.phone))
        .await
        .map_err(ise)?;
    Ok(see_other("/settings?ok=phone_removed"))
}

pub async fn revoke_session(
    s: Session,
    State(app): State<App>,
    Path(id): Path<i64>,
    _headers: HeaderMap,
) -> Res {
    app.store
        .write(move |c| auth::delete_session(c, id))
        .await
        .map_err(ise)?;
    if id == s.id {
        return Ok(see_other("/login"));
    }
    Ok(see_other("/settings?ok=session"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pair_link_shape() {
        assert_eq!(
            pair_link("https://sms.example.org/", "ABCD"),
            "rustybee://pair?u=https%3A%2F%2Fsms.example.org&c=ABCD"
        );
    }

    #[test]
    fn webhook_form_parsing() {
        let mut f = HashMap::new();
        f.insert("name".to_string(), "n".to_string());
        f.insert("url".to_string(), "https://x.test/h".to_string());
        assert!(parse_hook(&f).is_err());
        f.insert("ev_received".to_string(), "on".to_string());
        f.insert("active".to_string(), "on".to_string());
        let (_, _, ev, active) = parse_hook(&f).unwrap();
        assert_eq!(ev, vec!["message.received"]);
        assert!(active);
        f.insert("url".to_string(), "ftp://x".to_string());
        assert!(parse_hook(&f).is_err());
    }

    #[test]
    fn names_common_browsers() {
        let mac = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/155.0.0.0 Safari/537.36";
        assert_eq!(browser_label(mac), ("Chrome 155 on macOS".into(), false));
        let iphone = "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1";
        assert_eq!(browser_label(iphone), ("Safari 18 on iOS".into(), true));
        let android = "Mozilla/5.0 (Linux; Android 16) AppleWebKit/537.36 (KHTML, like Gecko) SamsungBrowser/28.0 Chrome/130.0 Mobile Safari/537.36";
        assert_eq!(
            browser_label(android),
            ("Samsung Internet 28 on Android".into(), true)
        );
        assert_eq!(browser_label("curl/8.7.1").0, "curl");
        assert_eq!(browser_label("").0, "Unknown browser");
    }
}
