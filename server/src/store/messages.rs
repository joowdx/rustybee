//! Messages, batches and the transactional webhook outbox hook (spec 004).

use super::opt;
use crate::domain::BatchCounts;
use crate::domain::lifecycle::{Event, State, transition};
use crate::protocol::Inbound;
use crate::util::{new_id, rfc3339};
use rusqlite::types::Value as SqlValue;
use rusqlite::{Connection, Row, params, params_from_iter};
use serde_json::{Value, json};

#[derive(Debug, Clone)]
pub struct MessageRow {
    pub id: i64,
    pub public_id: String,
    pub batch_public_id: Option<String>,
    pub device_id: i64,
    pub device_public_id: String,
    pub device_name: String,
    pub device_removed: bool,
    pub direction: String,
    pub peer: String,
    pub body: String,
    pub sim: Option<i64>,
    pub state: State,
    pub parts: Option<i64>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub frame_id: Option<String>,
    pub device_acked_at: Option<i64>,
    pub created_at: i64,
    pub dispatched_at: Option<i64>,
    pub sent_at: Option<i64>,
    pub delivered_at: Option<i64>,
    pub failed_at: Option<i64>,
    pub expired_at: Option<i64>,
    pub received_at: Option<i64>,
}

const SELECT: &str = "SELECT m.id, m.public_id, b.public_id, m.device_id, d.public_id, d.name, m.direction, m.peer,
    m.body, m.sim, m.state, m.parts, m.error_code, m.error_message, m.frame_id, m.device_acked_at,
    m.created_at, m.dispatched_at, m.sent_at, m.delivered_at, m.failed_at, m.expired_at, m.received_at,
    d.removed_at IS NOT NULL
    FROM messages m JOIN devices d ON d.id=m.device_id LEFT JOIN batches b ON b.id=m.batch_id";

fn from_row(r: &Row) -> rusqlite::Result<MessageRow> {
    let state: String = r.get(10)?;
    Ok(MessageRow {
        id: r.get(0)?,
        public_id: r.get(1)?,
        batch_public_id: r.get(2)?,
        device_id: r.get(3)?,
        device_public_id: r.get(4)?,
        device_name: r.get(5)?,
        direction: r.get(6)?,
        peer: r.get(7)?,
        body: r.get(8)?,
        sim: r.get(9)?,
        state: state.parse().unwrap_or(State::Failed),
        parts: r.get(11)?,
        error_code: r.get(12)?,
        error_message: r.get(13)?,
        frame_id: r.get(14)?,
        device_acked_at: r.get(15)?,
        created_at: r.get(16)?,
        dispatched_at: r.get(17)?,
        sent_at: r.get(18)?,
        delivered_at: r.get(19)?,
        failed_at: r.get(20)?,
        expired_at: r.get(21)?,
        received_at: r.get(22)?,
        device_removed: r.get(23)?,
    })
}

impl MessageRow {
    /// Public API message object (spec 007).
    pub fn to_json(&self) -> Value {
        let ts = |v: Option<i64>| v.map(rfc3339);
        let error = self.error_code.as_ref().map(
            |code| json!({"code": code, "message": self.error_message.clone().unwrap_or_default()}),
        );
        json!({
            "id": self.public_id,
            "batch_id": self.batch_public_id,
            "device_id": self.device_public_id,
            "direction": self.direction,
            "peer": self.peer,
            "body": self.body,
            "sim": self.sim,
            "state": self.state.as_str(),
            "parts": self.parts,
            "error": error,
            "created_at": rfc3339(self.created_at),
            "dispatched_at": ts(self.dispatched_at),
            "sent_at": ts(self.sent_at),
            "delivered_at": ts(self.delivered_at),
            "failed_at": ts(self.failed_at),
            "expired_at": ts(self.expired_at),
            "received_at": ts(self.received_at),
        })
    }

    /// Device name for the UI; removed devices are marked (spec 003).
    pub fn device_label(&self) -> String {
        if self.device_removed {
            format!("{} (removed)", self.device_name)
        } else {
            self.device_name.clone()
        }
    }

    /// `(label, timestamp)` pairs for the UI timeline.
    pub fn timeline(&self) -> Vec<(&'static str, i64)> {
        [
            ("created", Some(self.created_at)),
            ("received", self.received_at),
            ("dispatched", self.dispatched_at),
            ("acked by device", self.device_acked_at),
            ("sent", self.sent_at),
            ("delivered", self.delivered_at),
            ("failed", self.failed_at),
            ("expired", self.expired_at),
        ]
        .into_iter()
        .filter_map(|(l, t)| t.map(|t| (l, t)))
        .collect()
    }
}

pub fn by_id(c: &Connection, id: i64) -> rusqlite::Result<Option<MessageRow>> {
    opt(c.query_row(&format!("{SELECT} WHERE m.id=?1"), [id], from_row))
}

pub fn by_public(c: &Connection, pid: &str) -> rusqlite::Result<Option<MessageRow>> {
    opt(c.query_row(&format!("{SELECT} WHERE m.public_id=?1"), [pid], from_row))
}

/// Apply a lifecycle event. Returns the updated row when a transition happened.
/// Webhook rows for the new state are inserted in the same transaction.
pub fn apply_event(
    c: &Connection,
    id: i64,
    event: Event,
    at: i64,
    parts: Option<i64>,
    error: Option<(&str, &str)>,
    now: i64,
) -> rusqlite::Result<Option<MessageRow>> {
    let Some(msg) = by_id(c, id)? else {
        return Ok(None);
    };
    if msg.direction != "out" {
        return Ok(None);
    }
    let Some(to) = transition(msg.state, event) else {
        tracing::debug!(msg = %msg.public_id, from = %msg.state, ?event, "ignored transition");
        return Ok(None);
    };
    let col = match to {
        State::Dispatched => "dispatched_at",
        State::Sent => "sent_at",
        State::Delivered => "delivered_at",
        State::Failed => "failed_at",
        State::Expired => "expired_at",
        State::Queued | State::Received => unreachable!("not a transition target"),
    };
    let (code, message) = match (to, error) {
        (State::Failed, Some((c, m))) => (Some(c), Some(m)),
        (State::Failed, None) => (Some("generic_failure"), Some("")),
        _ => (None, None),
    };
    c.execute(
        &format!(
            "UPDATE messages SET state=?2, {col}=?3, parts=COALESCE(?4, parts),
               error_code=COALESCE(?5, error_code), error_message=COALESCE(?6, error_message)
             WHERE id=?1"
        ),
        params![id, to.as_str(), at, parts, code, message],
    )?;
    let row = by_id(c, id)?.expect("row exists");
    if let Some(ev) = to.webhook_event() {
        super::webhooks::enqueue(c, ev, &row, now)?;
    }
    Ok(Some(row))
}

/// Validated items to create as one batch.
pub struct NewMessage {
    pub to: String,
    pub body: String,
}

pub fn create_batch(
    c: &Connection,
    device_id: i64,
    items: &[NewMessage],
    sim: Option<i64>,
    now: i64,
) -> rusqlite::Result<(String, Vec<MessageRow>)> {
    let batch_pid = new_id();
    c.execute(
        "INSERT INTO batches(public_id, device_id, total, created_at) VALUES(?1,?2,?3,?4)",
        params![batch_pid, device_id, items.len() as i64, now],
    )?;
    let batch_id = c.last_insert_rowid();
    let mut ids = Vec::with_capacity(items.len());
    {
        let mut st = c.prepare(
            "INSERT INTO messages(public_id, batch_id, device_id, direction, peer, body, sim, state, created_at)
             VALUES(?1,?2,?3,'out',?4,?5,?6,'queued',?7)",
        )?;
        for it in items {
            st.execute(params![
                new_id(),
                batch_id,
                device_id,
                it.to,
                it.body,
                sim,
                now
            ])?;
            ids.push(c.last_insert_rowid());
        }
    }
    let mut rows = Vec::with_capacity(ids.len());
    for id in ids {
        rows.extend(by_id(c, id)?);
    }
    Ok((batch_pid, rows))
}

/// Insert an inbound SMS. `None` when `(device_id, uid)` was already stored.
pub fn insert_inbound(
    c: &Connection,
    device_id: i64,
    m: &Inbound,
    now: i64,
) -> rusqlite::Result<Option<MessageRow>> {
    let n = c.execute(
        "INSERT INTO messages(public_id, device_id, direction, peer, body, sim, state, inbound_uid,
           created_at, received_at)
         VALUES(?1,?2,'in',?3,?4,?5,'received',?6,?7,?8)
         ON CONFLICT(device_id, inbound_uid) DO NOTHING",
        params![
            new_id(),
            device_id,
            m.from,
            m.body,
            m.sim,
            m.uid,
            now,
            m.received_at
        ],
    )?;
    if n == 0 {
        return Ok(None);
    }
    let row = by_id(c, c.last_insert_rowid())?.expect("just inserted");
    super::webhooks::enqueue(c, "message.received", &row, now)?;
    Ok(Some(row))
}

/// Dispatcher step 4: move the oldest queued message to `dispatched`.
pub fn dispatch_next(
    c: &Connection,
    device_id: i64,
    now: i64,
) -> rusqlite::Result<Option<MessageRow>> {
    let next: Option<i64> = opt(c.query_row(
        "SELECT id FROM messages WHERE device_id=?1 AND state='queued' ORDER BY id LIMIT 1",
        [device_id],
        |r| r.get(0),
    ))?;
    let Some(id) = next else { return Ok(None) };
    if apply_event(c, id, Event::Dispatch, now, None, None, now)?.is_none() {
        return Ok(None);
    }
    c.execute(
        "UPDATE messages SET frame_id=?2 WHERE id=?1",
        params![id, new_id()],
    )?;
    by_id(c, id)
}

/// Dispatcher step 1: dispatched rows the device never acked, oldest first.
pub fn unacked_dispatched(c: &Connection, device_id: i64) -> rusqlite::Result<Vec<MessageRow>> {
    let mut st = c.prepare(&format!(
        "{SELECT} WHERE m.device_id=?1 AND m.state='dispatched' AND m.device_acked_at IS NULL ORDER BY m.id"
    ))?;
    st.query_map([device_id], from_row)?.collect()
}

/// Newest dispatched row (acked or not), used to seed the in-flight window.
pub fn latest_dispatched(c: &Connection, device_id: i64) -> rusqlite::Result<Option<MessageRow>> {
    opt(c.query_row(
        &format!(
            "{SELECT} WHERE m.device_id=?1 AND m.state='dispatched' ORDER BY m.id DESC LIMIT 1"
        ),
        [device_id],
        from_row,
    ))
}

/// Record the device's ack of a `send` frame. Returns the message row id.
pub fn ack_frame(
    c: &Connection,
    device_id: i64,
    frame_id: &str,
    now: i64,
) -> rusqlite::Result<Option<i64>> {
    let id: Option<i64> = opt(c.query_row(
        "SELECT id FROM messages WHERE device_id=?1 AND frame_id=?2",
        params![device_id, frame_id],
        |r| r.get(0),
    ))?;
    if let Some(id) = id {
        c.execute(
            "UPDATE messages SET device_acked_at=COALESCE(device_acked_at, ?2) WHERE id=?1",
            params![id, now],
        )?;
    }
    Ok(id)
}

pub fn batch_counts(
    c: &Connection,
    batch_pid: &str,
) -> rusqlite::Result<Option<(i64, String, i64, BatchCounts)>> {
    let batch: Option<(i64, String, i64)> = opt(c.query_row(
        "SELECT b.id, d.public_id, b.total FROM batches b JOIN devices d ON d.id=b.device_id WHERE b.public_id=?1",
        [batch_pid],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    ))?;
    let Some((id, device, total)) = batch else {
        return Ok(None);
    };
    let mut counts = BatchCounts::default();
    let mut st =
        c.prepare("SELECT state, count(*) FROM messages WHERE batch_id=?1 GROUP BY state")?;
    let rows = st.query_map([id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
    for row in rows {
        let (state, n) = row?;
        match state.as_str() {
            "queued" => counts.queued = n,
            "dispatched" => counts.dispatched = n,
            "sent" => counts.sent = n,
            "delivered" => counts.delivered = n,
            "failed" => counts.failed = n,
            "expired" => counts.expired = n,
            _ => {}
        }
    }
    Ok(Some((id, device, total, counts)))
}

#[derive(Debug, Clone, Default)]
pub struct Query {
    pub direction: Option<String>,
    pub state: Option<String>,
    pub device_id: Option<String>,
    pub peer: Option<String>,
    pub batch_id: Option<String>,
    pub q: Option<String>,
    pub since: Option<i64>,
    pub until: Option<i64>,
    pub before_id: Option<i64>,
    pub limit: i64,
}

/// Keyset page over `id DESC`. Returns rows and the id to continue before.
pub fn list(c: &Connection, q: &Query) -> rusqlite::Result<(Vec<MessageRow>, Option<i64>)> {
    let mut sql = format!("{SELECT} WHERE 1=1");
    let mut args: Vec<SqlValue> = vec![];
    let mut add = |cond: &str, vals: Vec<SqlValue>| {
        sql.push_str(" AND ");
        sql.push_str(cond);
        args.extend(vals);
    };
    let text = |v: &String| SqlValue::Text(v.clone());
    if let Some(v) = &q.direction {
        add("m.direction=?", vec![text(v)]);
    }
    if let Some(v) = &q.state {
        add("m.state=?", vec![text(v)]);
    }
    if let Some(v) = &q.device_id {
        add("d.public_id=?", vec![text(v)]);
    }
    if let Some(v) = &q.peer {
        add("m.peer=?", vec![text(v)]);
    }
    if let Some(v) = &q.batch_id {
        add("b.public_id=?", vec![text(v)]);
    }
    if let Some(v) = &q.q {
        add(
            "(instr(lower(m.body), lower(?)) > 0 OR instr(lower(m.peer), lower(?)) > 0)",
            vec![text(v), text(v)],
        );
    }
    if let Some(v) = q.since {
        add("m.created_at>=?", vec![v.into()]);
    }
    if let Some(v) = q.until {
        add("m.created_at<?", vec![v.into()]);
    }
    if let Some(v) = q.before_id {
        add("m.id<?", vec![v.into()]);
    }
    let limit = q.limit.clamp(1, 1000);
    sql.push_str(" ORDER BY m.id DESC LIMIT ?");
    args.push((limit + 1).into());
    let mut st = c.prepare(&sql)?;
    let mut rows: Vec<MessageRow> = st
        .query_map(params_from_iter(args), from_row)?
        .collect::<rusqlite::Result<_>>()?;
    let next = if rows.len() as i64 > limit {
        rows.truncate(limit as usize);
        rows.last().map(|r| r.id)
    } else {
        None
    };
    Ok((rows, next))
}

#[derive(Debug, Clone, Copy, Default, serde::Serialize)]
pub struct DayCounts {
    pub sent: i64,
    pub delivered: i64,
    pub failed: i64,
    pub received: i64,
    pub queued: i64,
}

pub fn day_counts(c: &Connection, since: i64) -> rusqlite::Result<DayCounts> {
    c.query_row(
        "SELECT
           (SELECT count(*) FROM messages WHERE sent_at>=?1),
           (SELECT count(*) FROM messages WHERE delivered_at>=?1),
           (SELECT count(*) FROM messages WHERE failed_at>=?1 OR expired_at>=?1),
           (SELECT count(*) FROM messages WHERE direction='in' AND created_at>=?1),
           (SELECT count(*) FROM messages WHERE state='queued')",
        [since],
        |r| {
            Ok(DayCounts {
                sent: r.get(0)?,
                delivered: r.get(1)?,
                failed: r.get(2)?,
                received: r.get(3)?,
                queued: r.get(4)?,
            })
        },
    )
}

/// TTL sweeper (spec 004). Returns the rows that changed.
pub fn sweep(
    c: &Connection,
    now: i64,
    ttl_ms: i64,
    status_timeout_ms: i64,
) -> rusqlite::Result<Vec<MessageRow>> {
    let ids = |sql: &str, arg: i64| -> rusqlite::Result<Vec<i64>> {
        let mut st = c.prepare(sql)?;
        st.query_map([arg], |r| r.get(0))?.collect()
    };
    let mut changed = vec![];
    let expire = ids(
        "SELECT id FROM messages WHERE created_at<?1 AND (state='queued'
           OR (state='dispatched' AND device_acked_at IS NULL))",
        now - ttl_ms,
    )?;
    for id in expire {
        changed.extend(apply_event(c, id, Event::Ttl, now, None, None, now)?);
    }
    let timeout = ids(
        "SELECT id FROM messages WHERE state='dispatched' AND device_acked_at<?1",
        now - status_timeout_ms,
    )?;
    for id in timeout {
        changed.extend(apply_event(
            c,
            id,
            Event::StatusTimeout,
            now,
            None,
            Some(("status_timeout", "no status from the device")),
            now,
        )?);
    }
    Ok(changed)
}
