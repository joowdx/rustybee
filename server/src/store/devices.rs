//! Devices and their telemetry.

use super::opt;
use crate::domain::filters::Filters;
use crate::protocol::{DeviceConfig, Health, Hello, PairRequest, Radio, Radios};
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
    pub filters_rev: i64,
    pub wake_attempts: i64,
    pub unreachable_at: Option<i64>,
    pub gateway_off_at: Option<i64>,
    pub lifecycle_json: Option<String>,
}

/// One network indicator in a device card's status bar (spec 015).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetChip {
    /// `wifi`, `cell`, or `net` for a connection that is not a radio.
    pub icon: &'static str,
    /// The network or carrier name when reported; the type for a connection other than Wi-Fi or cellular.
    pub label: Option<String>,
    /// Signal bars, 0–4; `None` when the signal is not reported.
    pub level: Option<i64>,
    /// Cellular generation, "2G" to "5G".
    pub tech: Option<String>,
    pub title: String,
    /// Shown as a fault, like "No SIM".
    pub bad: bool,
}

impl NetChip {
    /// Which of the four parts of the signal icon are lit, weakest first.
    pub fn bars(&self) -> [bool; 4] {
        let l = self.level.unwrap_or(0);
        [l >= 1, l >= 2, l >= 3, l >= 4]
    }
}

const COLS: &str = "id, public_id, name, enabled, is_default, send_delay_ms, receive_enabled, sim, \
    filters_json, fcm_token, manufacturer, model, os_version, app_version, app_build, telemetry_json, \
    sims_json, connected, last_seen_at, last_wake_at, created_at, filters_rev, \
    wake_attempts, unreachable_at, gateway_off_at, lifecycle_json";

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
        filters_rev: r.get(21)?,
        wake_attempts: r.get(22)?,
        unreachable_at: r.get(23)?,
        gateway_off_at: r.get(24)?,
        lifecycle_json: r.get(25)?,
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
            filters_rev: self.filters_rev,
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

    /// Radios as stored, for the API (spec 015).
    pub fn radios_json(&self) -> Value {
        self.telemetry.get("radios").cloned().unwrap_or(Value::Null)
    }

    /// The status-bar network indicators, Wi-Fi then cellular, each as its reported network or
    /// carrier name before its signal (spec 015).
    pub fn net_chips(&self) -> Vec<NetChip> {
        let r = self
            .telemetry
            .get("radios")
            .and_then(|v| serde_json::from_value::<Radios>(v.clone()).ok())
            .unwrap_or_default();
        let net = self.network();
        let on_cell = net.as_deref() == Some("cellular");
        let chip = |icon: &'static str, kind: &str, radio: &Radio, label: Option<String>| {
            let mut title = radio.name.clone().unwrap_or_else(|| kind.to_string());
            if let Some(d) = radio.dbm {
                title.push_str(&format!(", {d} dBm"));
            }
            NetChip {
                icon,
                label,
                level: radio.level.map(|l| l.clamp(0, 4)),
                // The generation shows only while mobile data carries the traffic, like the status bar.
                tech: radio.tech.clone().filter(|_| on_cell),
                title,
                bad: false,
            }
        };
        let none = Radio::default();
        let mut out = vec![];
        if let Some(other) = net
            .as_deref()
            .filter(|n| !["none", "wifi", "cellular"].contains(n))
        {
            // Not a radio (ethernet, other): a plain network icon, labelled with its type.
            out.push(chip("net", other, &none, Some(other.to_string())));
        }
        // Wi-Fi first, then cellular, each as `name (signal)`; the one in use is always shown.
        // Only a reported name labels a radio.
        for (icon, kind, radio) in [
            ("wifi", "wifi", r.wifi.as_ref()),
            ("cell", "cellular", r.cellular.as_ref()),
        ] {
            match radio {
                Some(x) => out.push(chip(icon, kind, x, x.name.clone())),
                None if net.as_deref() == Some(kind) => out.push(chip(icon, kind, &none, None)),
                None => {}
            }
        }
        if self.no_sim() {
            // Without a SIM the phone is no gateway: always say so in place of the cellular chip.
            out.retain(|c| c.icon != "cell");
            out.push(NetChip {
                icon: "cell",
                label: Some("No SIM".into()),
                level: None,
                tech: None,
                title: "No SIM in the phone".into(),
                bad: true,
            });
        }
        out
    }

    /// The SIM list was read with `READ_PHONE_STATE` and is empty, so the phone cannot send or
    /// receive. A list from a `hello` sent before the permission was granted says nothing.
    pub fn no_sim(&self) -> bool {
        self.telemetry.get("sims_read").and_then(Value::as_bool) == Some(true)
            && self.sims.as_array().is_some_and(Vec::is_empty)
    }

    /// Whether the device reports the SIM `id` as active.
    pub fn has_sim(&self, id: i64) -> bool {
        self.sims.as_array().is_some_and(|l| {
            l.iter()
                .any(|s| s.get("subscription_id").and_then(Value::as_i64) == Some(id))
        })
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
            "radios": self.radios_json(),
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
    let mut telemetry = json!({
        "battery": h.battery,
        "network": h.network,
        "permissions": h.permissions,
        "outbox": h.outbox,
    });
    // The SIM list below means "no SIM" only when it was read with the permission it needs.
    telemetry["sims_read"] =
        json!(h.permissions.as_ref().and_then(|p| p.read_phone_state) == Some(true));
    // A hello without `radios` clears them (spec 015).
    if let Some(r) = &h.radios {
        telemetry["radios"] = json!(r.cleaned());
    }
    // A missing or malformed lifecycle is already `None` and must not wipe a previous one.
    let lifecycle = h.lifecycle.as_ref().map(ToString::to_string);
    c.execute(
        "UPDATE devices SET app_version=?2, app_build=?3, os_version=?4, manufacturer=?5, model=?6,
           sims_json=?7, telemetry_json=?8, fcm_token=?9, connected=1, last_seen_at=?10,
           wake_attempts=0, unreachable_at=NULL, gateway_off_at=NULL,
           lifecycle_json=COALESCE(?11, lifecycle_json)
         WHERE id=?1",
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
            now,
            lifecycle
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
    // Replaced, `{}` included; absent (an older app) keeps it (spec 015).
    if let Some(r) = &h.radios {
        obj.insert("radios".into(), json!(r.cleaned()));
    }
    // Only an app that could read its SIMs sends them (spec 002).
    if h.sims.is_some() {
        obj.insert("sims_read".into(), json!(true));
    }
    let telemetry = t.to_string();
    if let Some(sims) = &h.sims {
        c.execute(
            "UPDATE devices SET sims_json=?2 WHERE id=?1",
            params![
                id,
                serde_json::to_string(sims).unwrap_or_else(|_| "[]".into())
            ],
        )?;
    }
    if let Some(token) = &h.fcm_token {
        c.execute(
            "UPDATE devices SET telemetry_json=?2, last_seen_at=?3, fcm_token=?4 WHERE id=?1",
            params![id, telemetry, now, token],
        )?;
    } else {
        c.execute(
            "UPDATE devices SET telemetry_json=?2, last_seen_at=?3 WHERE id=?1",
            params![id, telemetry, now],
        )?;
    }
    Ok(())
}

pub fn set_connected(c: &Connection, id: i64, connected: bool, now: i64) -> rusqlite::Result<()> {
    c.execute(
        "UPDATE devices SET connected=?2, last_seen_at=?3 WHERE id=?1",
        params![id, connected, now],
    )?;
    Ok(())
}

/// The app closed with 4010 because the user switched the gateway off (spec 009).
pub fn set_gateway_off(c: &Connection, id: i64, now: i64) -> rusqlite::Result<()> {
    c.execute(
        "UPDATE devices SET gateway_off_at=?2 WHERE id=?1",
        params![id, now],
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
    /// `None` keeps the stored SIM; `Some(None)` is the phone's default.
    pub sim: Option<Option<i64>>,
    pub filters: Filters,
    /// The `filters_rev` the web form was loaded with.
    pub base_rev: i64,
}

/// Save the web device form. Returns `true` when the submitted filters were not
/// stored because the phone changed forwarding after the form was loaded.
pub fn update_settings(c: &Connection, id: i64, s: &Settings) -> rusqlite::Result<bool> {
    let filters = serde_json::to_string(&s.filters).unwrap_or_default();
    // The web can turn forwarding off but not on, and edits the filters only while it
    // stays on and its `base_rev` is still the row's `filters_rev` (spec 002). Both are
    // checked against the row in the UPDATE itself, so a phone edit made since the form
    // was read is never undone. The writer is serialised, so the conflict check sees the
    // same row the UPDATE does.
    let conflict: bool = opt(c.query_row(
        "SELECT receive_enabled AND ?2 AND filters_rev IS NOT ?3 AND filters_json IS NOT ?4
         FROM devices WHERE id=?1",
        params![id, s.receive_enabled, s.base_rev, filters],
        |r| r.get(0),
    ))?
    .unwrap_or(false);
    c.execute(
        "UPDATE devices SET name=?2, enabled=?3, send_delay_ms=?4,
           sim=CASE WHEN ?9 THEN ?6 ELSE sim END,
           receive_enabled=(receive_enabled AND ?5),
           filters_rev=filters_rev + ((receive_enabled AND ?5) IS NOT receive_enabled
             OR (receive_enabled AND ?5 AND filters_rev=?8 AND filters_json IS NOT ?7)),
           filters_json=CASE WHEN receive_enabled AND ?5 AND filters_rev=?8
             THEN ?7 ELSE filters_json END
         WHERE id=?1",
        params![
            id,
            s.name,
            s.enabled,
            crate::domain::pacing::clamp_delay_ms(s.send_delay_ms),
            s.receive_enabled,
            s.sim.flatten(),
            filters,
            s.base_rev,
            s.sim.is_some()
        ],
    )?;
    Ok(conflict)
}

/// Save forwarding edited on the device when `base_rev` is still current.
/// `Ok(new_rev)` on success, `Err(current_rev)` when the web changed it first.
pub fn set_forwarding(
    c: &Connection,
    id: i64,
    base_rev: i64,
    receive_enabled: bool,
    f: &Filters,
) -> rusqlite::Result<Result<i64, i64>> {
    let n = c.execute(
        "UPDATE devices SET receive_enabled=?3, filters_json=?4, filters_rev=filters_rev+1
         WHERE id=?1 AND filters_rev=?2",
        params![
            id,
            base_rev,
            receive_enabled,
            serde_json::to_string(f).unwrap_or_default()
        ],
    )?;
    let rev: i64 = c.query_row("SELECT filters_rev FROM devices WHERE id=?1", [id], |r| {
        r.get(0)
    })?;
    Ok(if n == 1 { Ok(rev) } else { Err(rev) })
}

/// Save the preferred SIM picked on the device; the last edit, phone or web, wins (spec 002).
pub fn set_sim(c: &Connection, id: i64, sim: Option<i64>) -> rusqlite::Result<()> {
    c.execute("UPDATE devices SET sim=?2 WHERE id=?1", params![id, sim])?;
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
        "UPDATE devices SET removed_at=?2, token_sha256=?3, fcm_token=NULL, is_default=0, connected=0,
           unreachable_at=NULL, wake_attempts=0
         WHERE id=?1 AND removed_at IS NULL",
        params![id, now, crate::util::random_bytes::<32>().as_slice()],
    )?;
    Ok(())
}
