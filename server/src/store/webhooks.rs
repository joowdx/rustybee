//! Webhook subscriptions and the delivery outbox (spec 008).

use super::messages::MessageRow;
use super::opt;
use crate::util::{new_id, rfc3339};
use rusqlite::{Connection, params};
use serde_json::json;

pub const EVENTS: [&str; 5] = [
    "message.sent",
    "message.delivered",
    "message.failed",
    "message.expired",
    "message.received",
];

#[derive(Debug, Clone)]
pub struct WebhookRow {
    pub id: i64,
    pub public_id: String,
    pub name: String,
    pub url: String,
    pub events: Vec<String>,
    pub secret: String,
    pub active: bool,
    pub created_at: i64,
}

const HOOK_COLS: &str = "id, public_id, name, url, events_json, secret, active, created_at";

fn hook_row(r: &rusqlite::Row) -> rusqlite::Result<WebhookRow> {
    let events: String = r.get(4)?;
    Ok(WebhookRow {
        id: r.get(0)?,
        public_id: r.get(1)?,
        name: r.get(2)?,
        url: r.get(3)?,
        events: serde_json::from_str(&events).unwrap_or_default(),
        secret: r.get(5)?,
        active: r.get(6)?,
        created_at: r.get(7)?,
    })
}

pub fn new_secret() -> String {
    crate::domain::hex(&crate::util::random_bytes::<32>())
}

pub fn list(c: &Connection) -> rusqlite::Result<Vec<WebhookRow>> {
    let mut st = c.prepare(&format!("SELECT {HOOK_COLS} FROM webhooks ORDER BY id"))?;
    st.query_map([], hook_row)?.collect()
}

pub fn by_public(c: &Connection, pid: &str) -> rusqlite::Result<Option<WebhookRow>> {
    opt(c.query_row(
        &format!("SELECT {HOOK_COLS} FROM webhooks WHERE public_id=?1"),
        [pid],
        hook_row,
    ))
}

pub fn create(
    c: &Connection,
    name: &str,
    url: &str,
    events: &[String],
    active: bool,
    now: i64,
) -> rusqlite::Result<WebhookRow> {
    let pid = new_id();
    c.execute(
        "INSERT INTO webhooks(public_id, name, url, events_json, secret, active, created_at)
         VALUES(?1,?2,?3,?4,?5,?6,?7)",
        params![
            pid,
            name,
            url,
            serde_json::to_string(events).unwrap_or_default(),
            new_secret(),
            active,
            now
        ],
    )?;
    Ok(by_public(c, &pid)?.expect("just inserted"))
}

pub fn update(
    c: &Connection,
    pid: &str,
    name: &str,
    url: &str,
    events: &[String],
    active: bool,
) -> rusqlite::Result<usize> {
    c.execute(
        "UPDATE webhooks SET name=?2, url=?3, events_json=?4, active=?5 WHERE public_id=?1",
        params![
            pid,
            name,
            url,
            serde_json::to_string(events).unwrap_or_default(),
            active
        ],
    )
}

pub fn rotate_secret(c: &Connection, pid: &str) -> rusqlite::Result<usize> {
    c.execute(
        "UPDATE webhooks SET secret=?2 WHERE public_id=?1",
        params![pid, new_secret()],
    )
}

pub fn delete(c: &Connection, pid: &str) -> rusqlite::Result<usize> {
    c.execute("DELETE FROM webhooks WHERE public_id=?1", [pid])
}

pub fn payload(delivery_id: &str, event: &str, created_at: i64, data: serde_json::Value) -> String {
    json!({
        "id": delivery_id,
        "event": event,
        "created_at": rfc3339(created_at),
        "data": data,
    })
    .to_string()
}

/// Insert one delivery per active webhook subscribed to `event`. Runs inside
/// the caller's state-change transaction.
pub fn enqueue(c: &Connection, event: &str, msg: &MessageRow, now: i64) -> rusqlite::Result<usize> {
    let hooks: Vec<WebhookRow> = list(c)?
        .into_iter()
        .filter(|h| h.active && h.events.iter().any(|e| e == event))
        .collect();
    let data = msg.to_json();
    for h in &hooks {
        let pid = new_id();
        c.execute(
            "INSERT INTO webhook_deliveries(public_id, webhook_id, event, message_id, payload, next_attempt_at, created_at)
             VALUES(?1,?2,?3,?4,?5,?6,?6)",
            params![pid, h.id, event, msg.id, payload(&pid, event, now, data.clone()), now],
        )?;
    }
    Ok(hooks.len())
}

#[derive(Debug, Clone)]
pub struct DeliveryRow {
    pub id: i64,
    pub public_id: String,
    pub webhook_id: i64,
    pub webhook_public_id: String,
    pub event: String,
    pub message_public_id: Option<String>,
    pub payload: String,
    pub attempts: i64,
    pub next_attempt_at: Option<i64>,
    pub lease_until: Option<i64>,
    pub last_status: Option<i64>,
    pub last_error: Option<String>,
    pub created_at: i64,
    pub delivered_at: Option<i64>,
    pub aborted_at: Option<i64>,
}

impl DeliveryRow {
    pub fn status_label(&self) -> &'static str {
        if self.delivered_at.is_some() {
            "delivered"
        } else if self.aborted_at.is_some() {
            "aborted"
        } else if self.attempts > 0 {
            "retrying"
        } else {
            "pending"
        }
    }
}

const DEL_SELECT: &str = "SELECT w.id, w.public_id, w.webhook_id, h.public_id, w.event, m.public_id, w.payload,
    w.attempts, w.next_attempt_at, w.lease_until, w.last_status, w.last_error, w.created_at, w.delivered_at,
    w.aborted_at FROM webhook_deliveries w JOIN webhooks h ON h.id=w.webhook_id
    LEFT JOIN messages m ON m.id=w.message_id";

fn del_row(r: &rusqlite::Row) -> rusqlite::Result<DeliveryRow> {
    Ok(DeliveryRow {
        id: r.get(0)?,
        public_id: r.get(1)?,
        webhook_id: r.get(2)?,
        webhook_public_id: r.get(3)?,
        event: r.get(4)?,
        message_public_id: r.get(5)?,
        payload: r.get(6)?,
        attempts: r.get(7)?,
        next_attempt_at: r.get(8)?,
        lease_until: r.get(9)?,
        last_status: r.get(10)?,
        last_error: r.get(11)?,
        created_at: r.get(12)?,
        delivered_at: r.get(13)?,
        aborted_at: r.get(14)?,
    })
}

pub fn delivery(c: &Connection, pid: &str) -> rusqlite::Result<Option<DeliveryRow>> {
    opt(c.query_row(
        &format!("{DEL_SELECT} WHERE w.public_id=?1"),
        [pid],
        del_row,
    ))
}

pub fn deliveries_for_webhook(
    c: &Connection,
    webhook_id: i64,
    before: Option<i64>,
    limit: i64,
) -> rusqlite::Result<Vec<DeliveryRow>> {
    let mut st = c.prepare(&format!(
        "{DEL_SELECT} WHERE w.webhook_id=?1 AND w.id<?2 ORDER BY w.id DESC LIMIT ?3"
    ))?;
    st.query_map(
        params![webhook_id, before.unwrap_or(i64::MAX), limit],
        del_row,
    )?
    .collect()
}

pub fn deliveries_for_message(
    c: &Connection,
    message_id: i64,
) -> rusqlite::Result<Vec<DeliveryRow>> {
    let mut st = c.prepare(&format!("{DEL_SELECT} WHERE w.message_id=?1 ORDER BY w.id"))?;
    st.query_map([message_id], del_row)?.collect()
}

/// A claimed delivery with what the worker needs to send it.
#[derive(Debug, Clone)]
pub struct Claimed {
    pub id: i64,
    pub public_id: String,
    pub event: String,
    pub payload: String,
    pub attempts: i64,
    pub url: String,
    pub secret: String,
}

/// Lease up to `limit` due rows until `now + lease_ms`.
pub fn claim(
    c: &Connection,
    now: i64,
    lease_ms: i64,
    limit: i64,
) -> rusqlite::Result<Vec<Claimed>> {
    let mut st = c.prepare(
        "UPDATE webhook_deliveries SET lease_until=?2 WHERE id IN (
           SELECT id FROM webhook_deliveries
           WHERE next_attempt_at<=?1 AND (lease_until IS NULL OR lease_until<?1)
             AND delivered_at IS NULL AND aborted_at IS NULL
           ORDER BY next_attempt_at LIMIT ?3)
         RETURNING id",
    )?;
    let ids: Vec<i64> = st
        .query_map(params![now, now + lease_ms, limit], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        out.push(c.query_row(
            "SELECT w.id, w.public_id, w.event, w.payload, w.attempts, h.url, h.secret
             FROM webhook_deliveries w JOIN webhooks h ON h.id=w.webhook_id WHERE w.id=?1",
            [id],
            |r| {
                Ok(Claimed {
                    id: r.get(0)?,
                    public_id: r.get(1)?,
                    event: r.get(2)?,
                    payload: r.get(3)?,
                    attempts: r.get(4)?,
                    url: r.get(5)?,
                    secret: r.get(6)?,
                })
            },
        )?);
    }
    Ok(out)
}

pub fn mark_delivered(c: &Connection, id: i64, status: u16, now: i64) -> rusqlite::Result<()> {
    c.execute(
        "UPDATE webhook_deliveries SET attempts=attempts+1, delivered_at=?2, lease_until=NULL,
           last_status=?3, last_error=NULL, next_attempt_at=NULL WHERE id=?1",
        params![id, now, status],
    )?;
    Ok(())
}

pub fn mark_failed(
    c: &Connection,
    id: i64,
    status: Option<u16>,
    error: &str,
    next_attempt_at: Option<i64>,
    now: i64,
) -> rusqlite::Result<()> {
    let error: String = error.chars().take(500).collect();
    c.execute(
        "UPDATE webhook_deliveries SET attempts=attempts+1, lease_until=NULL, last_status=?2, last_error=?3,
           next_attempt_at=?4, aborted_at=CASE WHEN ?4 IS NULL THEN ?5 ELSE NULL END WHERE id=?1",
        params![id, status, error, next_attempt_at, now],
    )?;
    Ok(())
}

/// Redeliver (spec 008): reset an aborted or delivered row.
pub fn redeliver(c: &Connection, pid: &str, now: i64) -> rusqlite::Result<usize> {
    c.execute(
        "UPDATE webhook_deliveries SET attempts=0, next_attempt_at=?2, aborted_at=NULL, delivered_at=NULL,
           lease_until=NULL WHERE public_id=?1",
        params![pid, now],
    )
}

/// Earliest pending attempt, for the worker's sleep.
pub fn next_due(c: &Connection) -> rusqlite::Result<Option<i64>> {
    c.query_row(
        "SELECT min(max(next_attempt_at, COALESCE(lease_until, 0))) FROM webhook_deliveries
         WHERE delivered_at IS NULL AND aborted_at IS NULL AND next_attempt_at IS NOT NULL",
        [],
        |r| r.get(0),
    )
}
