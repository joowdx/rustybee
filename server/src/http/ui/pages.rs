//! Login, dashboard, live events, sending and message history.

use super::{
    DevOpt, Layout, Notice, Opt, Res, RowView, back_err, device_opts, ise, last_seen, not_found,
    pct, render, see_other, sim_opts, sse_event,
};
use crate::app::{App, BulkUpload, Draft};
use crate::domain::lifecycle::State as MsgState;
use crate::domain::{render_template, sms};
use crate::http::api::{ListParams, encode_cursor, parse_list};
use crate::http::auth::{COOKIE, Session, clear_cookie, cookie, session_cookie};
use crate::store::{auth, devices, messages, webhooks};
use crate::util::{human_time, random_token};
use askama::Template;
use axum::Form;
use axum::extract::{ConnectInfo, Multipart, Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use futures_util::Stream;
use serde::Deserialize;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::time::{Duration, Instant};
use tokio::sync::broadcast::error::RecvError;

const LOGIN_MAX_FAILURES: usize = 5;
const BULK_MAX_BYTES: usize = 1024 * 1024;
const BULK_MAX_ROWS: usize = 1000;
const BULK_TTL: Duration = Duration::from_secs(30 * 60);
const PREVIEW_ROWS: usize = 5;

// ---------- login ----------

#[derive(Template)]
#[template(path = "login.html")]
struct LoginPage {
    l: Layout,
    has_password: bool,
}

pub async fn login_page(State(app): State<App>) -> Response {
    render(&LoginPage {
        l: Layout::new("Log in", ""),
        has_password: app.has_password().await,
    })
}

#[derive(Deserialize)]
pub struct LoginForm {
    #[serde(default)]
    password: String,
}

pub async fn login(
    State(app): State<App>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Form(form): Form<LoginForm>,
) -> Response {
    let ip = crate::http::client_ip(&app, &headers, Some(peer));
    let window = app.cfg.login_window;
    let page = |msg: &str| LoginPage {
        l: Layout::new("Log in", "").err(msg),
        has_password: true,
    };
    {
        let mut map = app.login_failures.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        map.retain(|_, v| {
            v.retain(|t| now.duration_since(*t) < window);
            !v.is_empty()
        });
        if map.get(&ip).is_some_and(|v| v.len() >= LOGIN_MAX_FAILURES) {
            let mut r = render(&page("Too many attempts. Wait a minute and try again."));
            *r.status_mut() = StatusCode::TOO_MANY_REQUESTS;
            return r;
        }
    }
    let hash = match app.store.read(auth::password_hash).await {
        Ok(h) => h,
        Err(e) => return ise(e),
    };
    let Some(hash) = hash else {
        return render(&LoginPage {
            l: Layout::new("Log in", ""),
            has_password: false,
        });
    };
    let pw = form.password;
    let ok = tokio::task::spawn_blocking(move || auth::verify_password(&hash, &pw))
        .await
        .unwrap_or(false);
    if !ok {
        app.login_failures
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(ip)
            .or_default()
            .push(Instant::now());
        let mut r = render(&page("Wrong password."));
        *r.status_mut() = StatusCode::UNAUTHORIZED;
        return r;
    }
    let ua = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.chars().take(200).collect::<String>());
    let now = app.clock.now_ms();
    let token = match app
        .store
        .write(move |c| auth::create_session(c, ua.as_deref(), now))
        .await
    {
        Ok((_, t)) => t,
        Err(e) => return ise(e),
    };
    (
        [(header::SET_COOKIE, session_cookie(&app, &token))],
        see_other("/"),
    )
        .into_response()
}

pub async fn logout(State(app): State<App>, headers: HeaderMap) -> Response {
    if let Some(token) = cookie(&headers, COOKIE).map(str::to_string) {
        let now = app.clock.now_ms();
        let _ = app
            .store
            .write(move |c| {
                if let Some(s) = auth::session_by_token(c, &token, now)? {
                    auth::delete_session(c, s.id)?;
                }
                Ok(())
            })
            .await;
    }
    ([(header::SET_COOKIE, clear_cookie())], see_other("/login")).into_response()
}

// ---------- dashboard ----------

struct DevCard {
    name: String,
    model: String,
    online: bool,
    battery: String,
    charging: bool,
    network: String,
    last_seen: String,
    outbox: String,
    queued: i64,
    warnings: Vec<String>,
}

#[derive(Template)]
#[template(path = "dashboard.html")]
struct DashboardPage {
    l: Layout,
    devices: Vec<DevCard>,
    counts: messages::DayCounts,
    rows: Vec<RowView>,
}

#[derive(Template)]
#[template(path = "dashboard_partial.html")]
struct DashboardPartial {
    devices: Vec<DevCard>,
    counts: messages::DayCounts,
    rows: Vec<RowView>,
}

async fn dashboard_data(
    app: &App,
) -> Result<(Vec<DevCard>, messages::DayCounts, Vec<RowView>), Response> {
    let now = app.clock.now_ms();
    let since = now - now.rem_euclid(86_400_000);
    let (devs, counts, rows) = app
        .store
        .read(move |c| {
            let devs = devices::list(c)?;
            let mut out = Vec::with_capacity(devs.len());
            for d in devs {
                let q = devices::queued_count(c, d.id)?;
                out.push((d, q));
            }
            let counts = messages::day_counts(c, since)?;
            let (rows, _) = messages::list(
                c,
                &messages::Query {
                    limit: 10,
                    ..Default::default()
                },
            )?;
            Ok((out, counts, rows))
        })
        .await
        .map_err(ise)?;
    let cards = devs
        .into_iter()
        .map(|(d, queued)| {
            let mut warnings: Vec<String> = d
                .missing_permissions()
                .into_iter()
                .map(|p| format!("Missing permission: {p}"))
                .collect();
            if !d.enabled {
                warnings.push("Disabled for sending".into());
            }
            DevCard {
                name: d.name.clone(),
                model: [d.manufacturer.clone(), d.model.clone()]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(" "),
                online: d.connected,
                battery: d
                    .battery_level()
                    .map(|b| format!("{b}%"))
                    .unwrap_or_else(|| "–".into()),
                charging: d.charging().unwrap_or(false),
                network: d.network().unwrap_or_else(|| "–".into()),
                last_seen: last_seen(now, &d),
                outbox: d
                    .outbox()
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "–".into()),
                queued,
                warnings,
            }
        })
        .collect();
    Ok((cards, counts, rows.iter().map(RowView::from).collect()))
}

pub async fn dashboard(_: Session, State(app): State<App>) -> Res {
    let (devices, counts, rows) = dashboard_data(&app).await?;
    Ok(render(&DashboardPage {
        l: Layout::new("Home", "home"),
        devices,
        counts,
        rows,
    }))
}

pub async fn dashboard_partial(_: Session, State(app): State<App>) -> Res {
    let (devices, counts, rows) = dashboard_data(&app).await?;
    Ok(render(&DashboardPartial {
        devices,
        counts,
        rows,
    }))
}

pub async fn events(
    _: Session,
    State(app): State<App>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = app.bus.subscribe();
    let stream = futures_util::stream::unfold(rx, |mut rx| async move {
        loop {
            match rx.recv().await {
                Ok(ev) => return Some((Ok(sse_event(ev)), rx)),
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => return None,
            }
        }
    });
    Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(25)))
}

// ---------- send ----------

struct SendResult {
    count: usize,
    batch_id: String,
}

#[derive(Template)]
#[template(path = "send.html")]
struct SendPage {
    l: Layout,
    to: String,
    body: String,
    counter: String,
    device_opts: Vec<DevOpt>,
    sim_opts: Vec<Opt>,
    result: Option<SendResult>,
}

#[derive(Deserialize, Default)]
pub struct SendForm {
    #[serde(default)]
    to: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    device_id: String,
    #[serde(default)]
    sim: String,
}

fn counter(body: &str) -> String {
    let s = sms::segments(body);
    let enc = match s.encoding {
        sms::Encoding::Gsm7 => "GSM-7",
        sms::Encoding::Ucs2 => "UCS-2",
    };
    format!(
        "{enc} · {} chars · {} part{} · {} left",
        body.chars().count(),
        s.parts,
        if s.parts == 1 { "" } else { "s" },
        s.remaining
    )
}

async fn send_page_with(app: &App, f: SendForm, l: Layout, result: Option<SendResult>) -> Res {
    let devs = app.store.read(devices::list).await.map_err(ise)?;
    let sim = f.sim.parse().ok();
    Ok(render(&SendPage {
        l,
        counter: counter(&f.body),
        device_opts: device_opts(&devs, &f.device_id),
        sim_opts: sim_opts(&devs, sim),
        to: f.to,
        body: f.body,
        result,
    }))
}

pub async fn send_page(_: Session, State(app): State<App>) -> Res {
    send_page_with(&app, SendForm::default(), Layout::new("Send", "send"), None).await
}

fn opt_str(s: &str) -> Option<String> {
    Some(s.trim().to_string()).filter(|s| !s.is_empty())
}

pub async fn send(_: Session, State(app): State<App>, Form(f): Form<SendForm>) -> Res {
    let drafts: Vec<Draft> =
        f.to.split([',', ';', '\n'])
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .enumerate()
            .map(|(i, to)| Draft {
                label: format!("recipient {}", i + 1),
                to: to.to_string(),
                body: f.body.clone(),
            })
            .collect();
    let r = app
        .queue(drafts, opt_str(&f.device_id), f.sim.trim().parse().ok())
        .await;
    match r {
        Ok((batch, rows)) => {
            let keep = SendForm {
                device_id: f.device_id,
                sim: f.sim,
                ..Default::default()
            };
            let res = SendResult {
                count: rows.len(),
                batch_id: batch,
            };
            send_page_with(&app, keep, Layout::new("Send", "send"), Some(res)).await
        }
        Err(e) => {
            let mut r =
                send_page_with(&app, f, Layout::new("Send", "send").err(e.message), None).await?;
            *r.status_mut() = StatusCode::UNPROCESSABLE_ENTITY;
            Ok(r)
        }
    }
}

// ---------- bulk ----------

struct PreviewRow {
    to: String,
    body: String,
    parts: usize,
}

#[derive(Template)]
#[template(path = "bulk.html")]
struct BulkPage {
    l: Layout,
    token: String,
    total: usize,
    headers: Vec<Opt>,
    template: String,
    device_opts: Vec<DevOpt>,
    sim_opts: Vec<Opt>,
    preview: Vec<PreviewRow>,
    result: Option<SendResult>,
}

impl BulkPage {
    fn empty(l: Layout) -> Self {
        BulkPage {
            l,
            token: String::new(),
            total: 0,
            headers: vec![],
            template: String::new(),
            device_opts: vec![],
            sim_opts: vec![],
            preview: vec![],
            result: None,
        }
    }
}

fn prune_bulk(app: &App) {
    app.bulk
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .retain(|_, u| u.created.elapsed() < BULK_TTL);
}

pub async fn bulk_page(_: Session) -> Response {
    render(&BulkPage::empty(Layout::new("Bulk send", "bulk")))
}

fn bulk_error(msg: impl Into<String>) -> Response {
    let mut r = render(&BulkPage::empty(Layout::new("Bulk send", "bulk").err(msg)));
    *r.status_mut() = StatusCode::UNPROCESSABLE_ENTITY;
    r
}

fn parse_csv(bytes: &[u8]) -> Result<(Vec<String>, Vec<Vec<String>>), String> {
    let mut rdr = csv::ReaderBuilder::new()
        .flexible(true)
        .trim(csv::Trim::All)
        .from_reader(bytes);
    let headers: Vec<String> = rdr
        .headers()
        .map_err(|e| format!("CSV: {e}"))?
        .iter()
        .map(str::to_string)
        .collect();
    if headers.is_empty() || headers.iter().all(String::is_empty) {
        return Err("the CSV needs a header row".into());
    }
    let mut rows = Vec::new();
    for rec in rdr.records() {
        let rec = rec.map_err(|e| format!("CSV: {e}"))?;
        if rec.iter().all(str::is_empty) {
            continue;
        }
        if rows.len() == BULK_MAX_ROWS {
            return Err(format!("the CSV has more than {BULK_MAX_ROWS} rows"));
        }
        rows.push(rec.iter().map(str::to_string).collect());
    }
    if rows.is_empty() {
        return Err("the CSV has no data rows".into());
    }
    Ok((headers, rows))
}

pub async fn bulk_upload(_: Session, State(app): State<App>, mut mp: Multipart) -> Res {
    let mut data: Option<Vec<u8>> = None;
    while let Some(mut field) = mp
        .next_field()
        .await
        .map_err(|e| bulk_error(e.to_string()))?
    {
        if field.name() != Some("file") {
            continue;
        }
        let mut buf = Vec::new();
        while let Some(chunk) = field.chunk().await.map_err(|e| bulk_error(e.to_string()))? {
            buf.extend_from_slice(&chunk);
            if buf.len() > BULK_MAX_BYTES {
                return Err(bulk_error("the CSV is larger than 1 MB"));
            }
        }
        data = Some(buf);
    }
    let Some(data) = data else {
        return Err(bulk_error("choose a CSV file"));
    };
    let (headers, rows) = parse_csv(&data).map_err(bulk_error)?;
    prune_bulk(&app);
    let token = random_token();
    app.bulk.lock().unwrap_or_else(|e| e.into_inner()).insert(
        token.clone(),
        BulkUpload {
            created: Instant::now(),
            headers,
            rows,
        },
    );
    bulk_step(
        &app,
        &token,
        BulkForm::default(),
        Layout::new("Bulk send", "bulk"),
        false,
    )
    .await
}

#[derive(Deserialize, Default)]
pub struct BulkForm {
    #[serde(default)]
    token: String,
    #[serde(default)]
    phone: String,
    #[serde(default)]
    template: String,
    #[serde(default)]
    device_id: String,
    #[serde(default)]
    sim: String,
}

fn bulk_snapshot(app: &App, token: &str) -> Option<(Vec<String>, Vec<Vec<String>>)> {
    let map = app.bulk.lock().unwrap_or_else(|e| e.into_inner());
    map.get(token)
        .filter(|u| u.created.elapsed() < BULK_TTL)
        .map(|u| (u.headers.clone(), u.rows.clone()))
}

fn phone_index(headers: &[String], chosen: &str) -> usize {
    headers
        .iter()
        .position(|h| h == chosen)
        .or_else(|| {
            headers.iter().position(|h| {
                let h = h.to_lowercase();
                h.contains("phone") || h.contains("number") || h == "to" || h.contains("mobile")
            })
        })
        .unwrap_or(0)
}

fn bulk_drafts(headers: &[String], rows: &[Vec<String>], pi: usize, template: &str) -> Vec<Draft> {
    rows.iter()
        .enumerate()
        .map(|(i, row)| Draft {
            label: format!("row {}", i + 2),
            to: row.get(pi).cloned().unwrap_or_default(),
            body: render_template(template, |k| {
                headers
                    .iter()
                    .position(|h| h == k)
                    .map(|j| row.get(j).cloned().unwrap_or_default())
            }),
        })
        .collect()
}

async fn bulk_step(app: &App, token: &str, f: BulkForm, l: Layout, preview: bool) -> Res {
    let Some((headers, rows)) = bulk_snapshot(app, token) else {
        return Err(bulk_error("the upload expired; upload the CSV again"));
    };
    let pi = phone_index(&headers, &f.phone);
    let devs = app.store.read(devices::list).await.map_err(ise)?;
    let preview = if preview && !f.template.is_empty() {
        bulk_drafts(
            &headers,
            &rows[..rows.len().min(PREVIEW_ROWS)],
            pi,
            &f.template,
        )
        .into_iter()
        .map(|d| PreviewRow {
            parts: sms::segments(&d.body).parts,
            to: d.to,
            body: d.body,
        })
        .collect()
    } else {
        vec![]
    };
    let template = if f.template.is_empty() {
        headers
            .iter()
            .enumerate()
            .find(|(i, _)| *i != pi)
            .map(|(_, h)| format!("Hi {{{{{h}}}}}"))
            .unwrap_or_default()
    } else {
        f.template.clone()
    };
    Ok(render(&BulkPage {
        l,
        token: token.to_string(),
        total: rows.len(),
        headers: headers
            .iter()
            .enumerate()
            .map(|(i, h)| Opt {
                value: h.clone(),
                label: h.clone(),
                selected: i == pi,
            })
            .collect(),
        template,
        device_opts: device_opts(&devs, &f.device_id),
        sim_opts: sim_opts(&devs, f.sim.parse().ok()),
        preview,
        result: None,
    }))
}

pub async fn bulk_preview(_: Session, State(app): State<App>, Form(f): Form<BulkForm>) -> Res {
    let token = f.token.clone();
    bulk_step(&app, &token, f, Layout::new("Bulk send", "bulk"), true).await
}

pub async fn bulk_send(_: Session, State(app): State<App>, Form(f): Form<BulkForm>) -> Res {
    let Some((headers, rows)) = bulk_snapshot(&app, &f.token) else {
        return Err(bulk_error("the upload expired; upload the CSV again"));
    };
    if f.template.trim().is_empty() {
        let token = f.token.clone();
        let l = Layout::new("Bulk send", "bulk").err("write a message template");
        return bulk_step(&app, &token, f, l, false).await;
    }
    let pi = phone_index(&headers, &f.phone);
    let drafts = bulk_drafts(&headers, &rows, pi, &f.template);
    match app
        .queue(drafts, opt_str(&f.device_id), f.sim.trim().parse().ok())
        .await
    {
        Ok((batch, rows)) => {
            app.bulk
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&f.token);
            let mut page = BulkPage::empty(Layout::new("Bulk send", "bulk"));
            page.result = Some(SendResult {
                count: rows.len(),
                batch_id: batch,
            });
            Ok(render(&page))
        }
        Err(e) => {
            let token = f.token.clone();
            let l = Layout::new("Bulk send", "bulk").err(e.message);
            let mut r = bulk_step(&app, &token, f, l, true).await?;
            *r.status_mut() = StatusCode::UNPROCESSABLE_ENTITY;
            Ok(r)
        }
    }
}

// ---------- messages ----------

#[derive(Deserialize, Default, Clone)]
#[serde(default)]
pub struct MsgFilter {
    direction: String,
    state: String,
    device_id: String,
    peer: String,
    q: String,
    since: String,
    until: String,
    batch_id: String,
    cursor: String,
}

impl MsgFilter {
    /// Query string of the active filters (without the cursor).
    fn query(&self) -> Vec<(&'static str, &str)> {
        [
            ("direction", self.direction.as_str()),
            ("state", &self.state),
            ("device_id", &self.device_id),
            ("peer", &self.peer),
            ("q", &self.q),
            ("since", &self.since),
            ("until", &self.until),
            ("batch_id", &self.batch_id),
        ]
        .into_iter()
        .filter(|(_, v)| !v.trim().is_empty())
        .collect()
    }

    fn more(&self, next: Option<i64>) -> Option<String> {
        let next = next?;
        let mut parts: Vec<String> = self
            .query()
            .into_iter()
            .map(|(k, v)| format!("{k}={}", pct(v)))
            .collect();
        parts.push(format!("cursor={}", encode_cursor(next)));
        Some(parts.join("&"))
    }
}

/// `YYYY-MM-DD` from a date input becomes an RFC 3339 bound (UTC).
fn date_bound(v: &str, end: bool) -> Option<String> {
    let v = v.trim();
    if v.is_empty() {
        return None;
    }
    if v.len() == 10 && !v.contains('T') {
        return Some(if end {
            format!("{v}T23:59:59.999Z")
        } else {
            format!("{v}T00:00:00Z")
        });
    }
    Some(v.to_string())
}

async fn query_rows(app: &App, f: &MsgFilter) -> Result<(Vec<RowView>, Option<String>), String> {
    let p = ListParams {
        direction: Some(f.direction.clone()),
        state: Some(f.state.clone()),
        device_id: Some(f.device_id.clone()),
        peer: Some(f.peer.clone()),
        batch_id: Some(f.batch_id.clone()),
        q: Some(f.q.clone()),
        since: date_bound(&f.since, false),
        until: date_bound(&f.until, true),
        limit: None,
        cursor: Some(f.cursor.clone()),
    };
    let q = parse_list(p, 50).map_err(|e| e.message)?;
    let (rows, next) = app
        .store
        .read(move |c| messages::list(c, &q))
        .await
        .map_err(|e| e.to_string())?;
    Ok((rows.iter().map(RowView::from).collect(), f.more(next)))
}

#[derive(Template)]
#[template(path = "messages.html")]
struct MessagesPage {
    l: Layout,
    f: MsgFilter,
    states: Vec<Opt>,
    device_opts: Vec<DevOpt>,
    rows: Vec<RowView>,
    more: Option<String>,
}

pub async fn messages(_: Session, State(app): State<App>, Query(f): Query<MsgFilter>) -> Res {
    let mut l = Layout::new("Messages", "messages");
    let (rows, more) = match query_rows(&app, &f).await {
        Ok(v) => v,
        Err(e) => {
            l = l.err(e);
            (vec![], None)
        }
    };
    let devs = app.store.read(devices::list).await.map_err(ise)?;
    let states = [
        MsgState::Queued,
        MsgState::Dispatched,
        MsgState::Sent,
        MsgState::Delivered,
        MsgState::Failed,
        MsgState::Expired,
        MsgState::Received,
    ]
    .into_iter()
    .map(|s| Opt {
        value: s.as_str().into(),
        label: s.as_str().into(),
        selected: f.state == s.as_str(),
    })
    .collect();
    Ok(render(&MessagesPage {
        l,
        states,
        device_opts: device_opts(&devs, &f.device_id),
        f,
        rows,
        more,
    }))
}

#[derive(Template)]
#[template(path = "rows_partial.html")]
struct RowsPartial {
    rows: Vec<RowView>,
    more: Option<String>,
}

pub async fn message_rows(_: Session, State(app): State<App>, Query(f): Query<MsgFilter>) -> Res {
    let (rows, more) = query_rows(&app, &f)
        .await
        .map_err(|e| (StatusCode::BAD_REQUEST, e).into_response())?;
    Ok(render(&RowsPartial { rows, more }))
}

struct MessageView {
    id: String,
    arrow: &'static str,
    peer: String,
    body: String,
    state: &'static str,
    device: String,
    batch: Option<String>,
    sim: Option<i64>,
    parts: Option<i64>,
    error: Option<String>,
    timeline: Vec<(&'static str, String)>,
}

struct DeliveryLine {
    webhook: String,
    event: String,
    status: &'static str,
    attempts: i64,
    last: String,
}

#[derive(Template)]
#[template(path = "message.html")]
struct MessagePage {
    l: Layout,
    m: MessageView,
    deliveries: Vec<DeliveryLine>,
}

pub fn delivery_last(d: &webhooks::DeliveryRow) -> String {
    match (d.last_status, &d.last_error) {
        (Some(s), Some(e)) => format!("HTTP {s}: {e}"),
        (Some(s), None) => format!("HTTP {s}"),
        (None, Some(e)) => e.clone(),
        (None, None) => "–".into(),
    }
}

pub async fn message(
    _: Session,
    State(app): State<App>,
    Path(id): Path<String>,
    Query(n): Query<Notice>,
) -> Res {
    let found = app
        .store
        .read(move |c| {
            let Some(m) = messages::by_public(c, &id)? else {
                return Ok(None);
            };
            let d = webhooks::deliveries_for_message(c, m.id)?;
            Ok(Some((m, d)))
        })
        .await
        .map_err(ise)?;
    let Some((m, dels)) = found else {
        return Err(not_found());
    };
    let error = match (&m.error_code, &m.error_message) {
        (Some(c), Some(msg)) => Some(format!("{c}: {msg}")),
        (Some(c), None) => Some(c.clone()),
        (None, Some(msg)) => Some(msg.clone()),
        (None, None) => None,
    };
    let view = MessageView {
        id: m.public_id.clone(),
        arrow: if m.direction == "in" { "←" } else { "→" },
        peer: m.peer.clone(),
        body: m.body.clone(),
        state: m.state.as_str(),
        device: m.device_name.clone(),
        batch: m.batch_public_id.clone(),
        sim: m.sim,
        parts: m.parts,
        error,
        timeline: m
            .timeline()
            .into_iter()
            .map(|(l, t)| (l, human_time(t)))
            .collect(),
    };
    Ok(render(&MessagePage {
        l: Layout::new(format!("Message {}", m.peer), "messages").notice(&n),
        m: view,
        deliveries: dels
            .iter()
            .map(|d| DeliveryLine {
                webhook: d.webhook_public_id.clone(),
                event: d.event.clone(),
                status: d.status_label(),
                attempts: d.attempts,
                last: delivery_last(d),
            })
            .collect(),
    }))
}

#[derive(Deserialize)]
pub struct ReplyForm {
    #[serde(default)]
    body: String,
}

pub async fn reply(
    _: Session,
    State(app): State<App>,
    Path(id): Path<String>,
    Form(f): Form<ReplyForm>,
) -> Res {
    let pid = id.clone();
    let m = app
        .store
        .read(move |c| messages::by_public(c, &pid))
        .await
        .map_err(ise)?
        .ok_or_else(not_found)?;
    let back = format!("/messages/{id}");
    let draft = Draft {
        label: "peer".into(),
        to: m.peer.clone(),
        body: f.body,
    };
    match app
        .queue(vec![draft], Some(m.device_public_id.clone()), m.sim)
        .await
    {
        Ok(_) => Ok(see_other(&format!("{back}?ok=reply"))),
        Err(e) => Ok(back_err(&back, e.message)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_parsing_limits() {
        let (h, r) = parse_csv(b"phone,name\n+15550001,Ann\n\n+15550002,Bob\n").unwrap();
        assert_eq!(h, vec!["phone", "name"]);
        assert_eq!(r.len(), 2);
        assert!(parse_csv(b"phone,name\n").is_err());
        let mut big = String::from("phone\n");
        for i in 0..=BULK_MAX_ROWS {
            big.push_str(&format!("+1555{i:07}\n"));
        }
        assert!(parse_csv(big.as_bytes()).is_err());
    }

    #[test]
    fn bulk_template_substitutes() {
        let h = vec!["phone".to_string(), "name".to_string()];
        let rows = vec![vec!["+15550001".to_string(), "Ann".to_string()]];
        let d = bulk_drafts(&h, &rows, phone_index(&h, ""), "Hi {{name}}");
        assert_eq!(d[0].to, "+15550001");
        assert_eq!(d[0].body, "Hi Ann");
    }

    #[test]
    fn date_bounds() {
        assert_eq!(
            date_bound("2026-10-16", false).unwrap(),
            "2026-10-16T00:00:00Z"
        );
        assert_eq!(
            date_bound("2026-10-16", true).unwrap(),
            "2026-10-16T23:59:59.999Z"
        );
        assert_eq!(date_bound(" ", true), None);
    }
}
