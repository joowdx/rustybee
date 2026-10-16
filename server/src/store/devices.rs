//! Devices and their telemetry.

use super::opt;
use crate::domain::filters::Filters;
use crate::protocol::{DeviceConfig, Health, Hello, PairRequest};
use rusqlite::{Connection, Row, params};
use serde_json::{Value, json};

#[derive(Debug, Clone)]
pub struct DeviceRow {
    pub id: i64,
    pub public_id: String,
    pub name: String,
    pub enabled: bool,
    pub is_default: bool,
    pub send_delay_ms: i64,
    pub receive_enabled: bool,
    pub sim: Option<i64>,
    pub filters: Filters,
    pub fcm_token: Option<String>,
    pub manufacturer: Option<String>,
    pub model: Option<String>,
    pub os_version: Option<String>,
    pub app_version: Option<String>,
    pub app_build: Option<i64>,
    pub telemetry: Value,
    pub sims: Value,
    pub connected: bool,
    pub last_seen_at: Option<i64>,
    pub last_wake_at: Option<i64>,
    pub created_at: i64,
}

const COLS: &str = "id, public_id, name, enabled, is_default, send_delay_ms, receive_enabled, sim, \
    filters_json, fcm_token, manufacturer, model, os_version, app_version, app_build, telemetry_json, \
    sims_json, connected, last_seen_at, last_wake_at, created_at";

fn from_row(r: &Row) -> rusqlite::Result<DeviceRow> {
    let filters: String = r.get(8)?;
    let telemetry: Option<String> = r.get(15)?;
    let sims: Option<String> = r.get(16)?;
    Ok(DeviceRow {
        id: r.get(0)?,
        public_id: r.get(1)?,
        name: r.get(2)?,
        enabled: r.get(3)?,
        is_default: r.get(4)?,
        send_delay_ms: r.get(5)?,
        receive_enabled: r.get(6)?,
        sim: r.get(7)?,
        filters: serde_json::from_str(&filters).unwrap_or_default(),
        fcm_token: r.get(9)?,
        manufacturer: r.get(10)?,
        model: r.get(11)?,
        os_version: r.get(12)?,
        app_version: r.get(13)?,
        app_build: r.get(14)?,
        telemetry: telemetry
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or(Value::Null),
        sims: sims
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_else(|| json!([])),
        connected: r.get(17)?,
        last_seen_at: r.get(18)?,
        last_wake_at: r.get(19)?,
        created_at: r.get(20)?,
    })
}

impl DeviceRow {
    pub fn config(&self, ping_s: u32) -> DeviceConfig {
        DeviceConfig {
            name: self.name.clone(),
            ping_s,
            receive_enabled: self.receive_enabled,
            sim: self.sim,
            filters: self.filters.clone(),
        }
    }

    pub fn battery_level(&self) -> Option<i64> {
        self.telemetry
            .pointer("/battery/level")
            .and_then(Value::as_i64)
    }

    pub fn charging(&self) -> Option<bool> {
        self.telemetry
            .pointer("/battery/charging")
            .and_then(Value::as_bool)
    }

    pub fn network(&self) -> Option<String> {
        self.telemetry
            .get("network")
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    pub fn outbox(&self) -> Option<i64> {
        self.telemetry.get("outbox").and_then(Value::as_i64)
    }

    /// Names of permissions the device reports as missing.
    pub fn missing_permissions(&self) -> Vec<String> {
        let Some(p) = self.telemetry.get("permissions").and_then(Value::as_object) else {
            return vec![];
        };
        p.iter()
            .filter(|(_, v)| v.as_bool() == Some(false))
            .map(|(k, _)| k.replace('_', " "))
            .collect()
    }

    /// Public API shape (spec 007).
    pub fn to_json(&self) -> Value {
        json!({
            "id": self.public_id,
            "name": self.name,
            "enabled": self.enabled,
            "is_default": self.is_default,
            "connected": self.connected,
            "last_seen_at": self.last_seen_at.map(crate::util::rfc3339),
            "model": self.model,
            "app_version": self.app_version,
            "battery": self.battery_level(),
            "charging": self.charging(),
            "network": self.network(),
            "sims": self.sims,
        })
    }
}

pub fn by_id(c: &Connection, id: i64) -> rusqlite::Result<Option<DeviceRow>> {
    opt(c.query_row(
        &format!("SELECT {COLS} FROM devices WHERE id=?1 AND removed_at IS NULL"),
        [id],
        from_row,
    ))
}

pub fn by_public(c: &Connection, pid: &str) -> rusqlite::Result<Option<DeviceRow>> {
    opt(c.query_row(
        &format!("SELECT {COLS} FROM devices WHERE public_id=?1 AND removed_at IS NULL"),
        [pid],
        from_row,
    ))
}

pub fn by_token(c: &Connection, sha: &[u8]) -> rusqlite::Result<Option<DeviceRow>> {
    opt(c.query_row(
        &format!("SELECT {COLS} FROM devices WHERE token_sha256=?1 AND removed_at IS NULL"),
        [sha],
        from_row,
    ))
}

pub fn list(c: &Connection) -> rusqlite::Result<Vec<DeviceRow>> {
    let mut st = c.prepare(&format!(
        "SELECT {COLS} FROM devices WHERE removed_at IS NULL ORDER BY id"
    ))?;
    st.query_map([], from_row)?.collect()
}

pub fn create(
    c: &Connection,
    req: &PairRequest,
    token_sha: &[u8],
    now: i64,
) -> rusqlite::Result<DeviceRow> {
    let has_default: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM devices WHERE is_default=1 AND removed_at IS NULL)",
        [],
        |r| r.get(0),
    )?;
    let name = req
        .name
        .clone()
        .filter(|n| !n.trim().is_empty())
        .or_else(|| req.model.clone())
        .unwrap_or_else(|| "Phone".into());
    c.execute(
        "INSERT INTO devices(public_id, name, token_sha256, is_default, fcm_token, manufacturer, model,
           os_version, app_version, app_build, created_at, last_seen_at)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?11)",
        params![
            crate::util::new_id(),
            name,
            token_sha,
            !has_default,
            req.fcm_token,
            req.manufacturer,
            req.model,
            req.os_version,
            req.app_version,
            req.app_build,
            now
        ],
    )?;
    Ok(by_id(c, c.last_insert_rowid())?.expect("just inserted"))
}

pub fn apply_hello(c: &Connection, id: i64, h: &Hello, now: i64) -> rusqlite::Result<()> {
    let telemetry = json!({
        "battery": h.battery,
        "network": h.network,
        "permissions": h.permissions,
        "outbox": h.outbox,
    });
    c.execute(
        "UPDATE devices SET app_version=?2, app_build=?3, os_version=?4, manufacturer=?5, model=?6,
           sims_json=?7, telemetry_json=?8, fcm_token=?9, connected=1, last_seen_at=?10 WHERE id=?1",
        params![
            id,
            h.app_version,
            h.app_build,
            h.os_version,
            h.manufacturer,
            h.model,
            serde_json::to_string(&h.sims).unwrap_or_else(|_| "[]".into()),
            telemetry.to_string(),
            h.fcm_token,
            now
        ],
    )?;
    Ok(())
}

pub fn apply_health(c: &Connection, id: i64, h: &Health, now: i64) -> rusqlite::Result<()> {
    let current: Option<String> = opt(c.query_row(
        "SELECT telemetry_json FROM devices WHERE id=?1 AND removed_at IS NULL",
        [id],
        |r| r.get(0),
    ))?
    .flatten();
    let mut t: Value = current
        .and_then(|s| serde_json::from_str(&s).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}));
    let obj = t.as_object_mut().expect("object");
    if let Some(b) = &h.battery {
        obj.insert("battery".into(), json!(b));
    }
    if let Some(n) = &h.network {
        obj.insert("network".into(), json!(n));
    }
    if let Some(p) = &h.permissions {
        obj.insert("permissions".into(), json!(p));
    }
    if let Some(o) = h.outbox {
        obj.insert("outbox".into(), json!(o));
    }
    c.execute(
        "UPDATE devices SET telemetry_json=?2, last_seen_at=?3 WHERE id=?1",
        params![id, t.to_string(), now],
    )?;
    Ok(())
}

pub fn set_connected(c: &Connection, id: i64, connected: bool, now: i64) -> rusqlite::Result<()> {
    c.execute(
        "UPDATE devices SET connected=?2, last_seen_at=?3 WHERE id=?1",
        params![id, connected, now],
    )?;
    Ok(())
}

pub fn touch(c: &Connection, id: i64, now: i64) -> rusqlite::Result<()> {
    c.execute(
        "UPDATE devices SET last_seen_at=?2 WHERE id=?1",
        params![id, now],
    )?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Choice {
    Ok(i64),
    NotFound,
    Disabled,
    Required,
}

/// Device choice for a new message (spec 006): explicit, default, only enabled, else 400.
pub fn choose(c: &Connection, explicit: Option<&str>) -> rusqlite::Result<Choice> {
    if let Some(pid) = explicit {
        return Ok(match by_public(c, pid)? {
            None => Choice::NotFound,
            Some(d) if !d.enabled => Choice::Disabled,
            Some(d) => Choice::Ok(d.id),
        });
    }
    let all = list(c)?;
    if let Some(d) = all.iter().find(|d| d.is_default && d.enabled) {
        return Ok(Choice::Ok(d.id));
    }
    let enabled: Vec<_> = all.iter().filter(|d| d.enabled).collect();
    Ok(match enabled.as_slice() {
        [only] => Choice::Ok(only.id),
        _ => Choice::Required,
    })
}

pub struct Settings {
    pub name: String,
    pub enabled: bool,
    pub send_delay_ms: i64,
    pub receive_enabled: bool,
    pub sim: Option<i64>,
    pub filters: Filters,
}

pub fn update_settings(c: &Connection, id: i64, s: &Settings) -> rusqlite::Result<()> {
    c.execute(
        "UPDATE devices SET name=?2, enabled=?3, send_delay_ms=?4, receive_enabled=?5, sim=?6, filters_json=?7
         WHERE id=?1",
        params![
            id,
            s.name,
            s.enabled,
            crate::domain::pacing::clamp_delay_ms(s.send_delay_ms),
            s.receive_enabled,
            s.sim,
            serde_json::to_string(&s.filters).unwrap_or_default()
        ],
    )?;
    Ok(())
}

pub fn set_default(c: &Connection, id: i64) -> rusqlite::Result<()> {
    c.execute(
        "UPDATE devices SET is_default=(id=?1) WHERE removed_at IS NULL",
        [id],
    )?;
    Ok(())
}

pub fn queued_count(c: &Connection, id: i64) -> rusqlite::Result<i64> {
    c.query_row(
        "SELECT count(*) FROM messages WHERE device_id=?1 AND state='queued'",
        [id],
        |r| r.get(0),
    )
}

/// Remove a device (spec 003 soft delete). Every outbound `queued` or
/// `dispatched` message is cancelled (spec 004) so its webhooks fire; the
/// row is kept for history but hidden from every lookup, its token can no
/// longer authenticate and it is never chosen as the default again.
pub fn delete(c: &Connection, id: i64, now: i64) -> rusqlite::Result<()> {
    let pending: Vec<i64> = {
        let mut st = c.prepare(
            "SELECT id FROM messages WHERE device_id=?1 AND direction='out'
               AND state IN ('queued','dispatched') ORDER BY id",
        )?;
        st.query_map([id], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?
    };
    for m in pending {
        super::messages::apply_event(
            c,
            m,
            crate::domain::lifecycle::Event::Cancel,
            now,
            None,
            Some(("cancelled", "device removed")),
            now,
        )?;
    }
    c.execute(
        "UPDATE devices SET removed_at=?2, token_sha256=?3, fcm_token=NULL, is_default=0, connected=0
         WHERE id=?1 AND removed_at IS NULL",
        params![id, now, crate::util::random_bytes::<32>().as_slice()],
    )?;
    Ok(())
}
