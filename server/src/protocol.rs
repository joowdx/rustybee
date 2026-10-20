//! Device protocol frames (spec 002). Golden examples: `protocol/fixtures/`.

use crate::domain::filters::Filters;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const VERSION: u32 = 1;

/// The raw envelope. `data` is decoded per `type`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Envelope {
    pub v: u32,
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub data: Value,
}

impl Envelope {
    pub fn new(kind: &str, data: impl Serialize) -> Self {
        Self::with_id(ulid::Ulid::new().to_string(), kind, data)
    }

    pub fn with_id(id: String, kind: &str, data: impl Serialize) -> Self {
        Envelope {
            v: VERSION,
            id,
            kind: kind.to_string(),
            data: serde_json::to_value(data).unwrap_or(Value::Null),
        }
    }

    pub fn to_text(&self) -> String {
        serde_json::to_string(self).expect("envelope serializes")
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Ack {
    #[serde(rename = "ref")]
    pub reference: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Nack {
    #[serde(rename = "ref")]
    pub reference: String,
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sim {
    pub subscription_id: i64,
    pub slot: Option<i64>,
    pub carrier: Option<String>,
    pub number: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Battery {
    pub level: Option<i64>,
    pub charging: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Permissions {
    pub send_sms: Option<bool>,
    pub receive_sms: Option<bool>,
    pub notifications: Option<bool>,
    pub battery_unrestricted: Option<bool>,
    /// Optional, newer apps only: `READ_PHONE_STATE` granted (spec 002).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_phone_state: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Hello {
    pub app_version: Option<String>,
    pub app_build: Option<i64>,
    pub os_version: Option<String>,
    pub manufacturer: Option<String>,
    pub model: Option<String>,
    #[serde(default)]
    pub sims: Vec<Sim>,
    pub battery: Option<Battery>,
    pub network: Option<String>,
    pub fcm_token: Option<String>,
    pub permissions: Option<Permissions>,
    pub outbox: Option<i64>,
    /// How this process started and why the previous one ended. Missing or
    /// malformed values are dropped so the rest of `hello` still stands (spec 002).
    #[serde(
        default,
        deserialize_with = "de_lifecycle",
        skip_serializing_if = "Option::is_none"
    )]
    pub lifecycle: Option<Value>,
    /// Per-radio network name and signal, each field opt-in on the phone (spec 015).
    /// Malformed values are treated as absent so the rest of `hello` still stands.
    #[serde(
        default,
        deserialize_with = "de_radios",
        skip_serializing_if = "Option::is_none"
    )]
    pub radios: Option<Radios>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Health {
    pub battery: Option<Battery>,
    pub network: Option<String>,
    pub permissions: Option<Permissions>,
    pub outbox: Option<i64>,
    /// Sent only when the token changed; replaces the stored token (spec 002).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fcm_token: Option<String>,
    /// The active SIMs, sent by newer apps when they can read them; replaces the stored list,
    /// `[]` included. Absent keeps it (spec 002).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sims: Option<Vec<Sim>>,
    /// Replaces the stored radios when present, `{}` included; malformed counts as absent (spec 015).
    #[serde(
        default,
        deserialize_with = "de_radios",
        skip_serializing_if = "Option::is_none"
    )]
    pub radios: Option<Radios>,
}

/// Radios the phone reports (spec 015). A radio with nothing to report is absent.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Radios {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cellular: Option<Radio>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wifi: Option<Radio>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Radio {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Signal bars, 0–4.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dbm: Option<i64>,
    /// Cellular generation, "2G" to "5G", sent while mobile data is on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tech: Option<String>,
}

impl Radios {
    /// What the server stores (spec 015): names trimmed and capped, levels clamped, odd dBm and
    /// unknown generations dropped. Only cellular has a generation.
    pub fn cleaned(&self) -> Self {
        Self {
            cellular: self.cellular.as_ref().map(Radio::cleaned),
            wifi: self.wifi.as_ref().map(|r| Radio {
                tech: None,
                ..r.cleaned()
            }),
        }
    }
}

impl Radio {
    const MAX_NAME: usize = 64;

    fn cleaned(&self) -> Self {
        Self {
            name: self
                .name
                .as_deref()
                .map(|n| n.trim().chars().take(Self::MAX_NAME).collect::<String>())
                .filter(|n| !n.is_empty()),
            level: self.level.map(|l| l.clamp(0, 4)),
            dbm: self.dbm.filter(|d| (-150..=-1).contains(d)),
            tech: self
                .tech
                .clone()
                .filter(|t| ["2G", "3G", "4G", "5G"].contains(&t.as_str())),
        }
    }
}

/// Parse `radios` leniently: an opt-in detail must never fail the frame (spec 015).
fn de_radios<'de, D>(deserializer: D) -> Result<Option<Radios>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<Value>::deserialize(deserializer)?;
    Ok(value.and_then(|v| serde_json::from_value(v).ok()))
}

/// Keep a `hello.lifecycle` object only when it has the shape from spec 002.
/// Anything else is ignored rather than failing the frame.
fn de_lifecycle<'de, D>(deserializer: D) -> Result<Option<Value>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<Value>::deserialize(deserializer)?;
    Ok(value.filter(lifecycle_well_formed))
}

fn lifecycle_well_formed(v: &Value) -> bool {
    let Some(obj) = v.as_object() else {
        return false;
    };
    // `start` is omitted when the OS has no record for this process (spec 011).
    match obj.get("start") {
        None | Some(Value::Null) => {}
        Some(start) => {
            let Some(start) = start.as_object() else {
                return false;
            };
            if start.get("reason").and_then(Value::as_str).is_none()
                || start.get("at").and_then(Value::as_i64).is_none()
                || start
                    .get("was_force_stopped")
                    .and_then(Value::as_bool)
                    .is_none()
            {
                return false;
            }
        }
    }
    match obj.get("exit") {
        None | Some(Value::Null) => true,
        Some(exit) => {
            let Some(exit) = exit.as_object() else {
                return false;
            };
            if exit.get("reason").and_then(Value::as_str).is_none()
                || exit.get("at").and_then(Value::as_i64).is_none()
            {
                return false;
            }
            matches!(
                exit.get("description"),
                None | Some(Value::Null) | Some(Value::String(_))
            )
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ErrorInfo {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StatusState {
    Sent,
    Delivered,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Status {
    pub msg_id: String,
    pub state: StatusState,
    pub at: i64,
    pub parts: Option<i64>,
    pub error: Option<ErrorInfo>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Inbound {
    pub uid: String,
    pub from: String,
    pub body: String,
    pub sim: Option<i64>,
    pub received_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeviceConfig {
    pub name: String,
    pub ping_s: u32,
    pub receive_enabled: bool,
    pub sim: Option<i64>,
    pub filters: Filters,
    /// Bumped when `receive_enabled` or the filters change; the device echoes it in `set_forwarding`.
    #[serde(default)]
    pub filters_rev: i64,
}

/// The preferred SIM picked on the device; `None` is the phone's default SMS SIM (spec 002).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SetSim {
    pub sim: Option<i64>,
}

/// Forwarding edited on the device, based on the `filters_rev` it last saw.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SetForwarding {
    pub base_rev: i64,
    pub receive_enabled: bool,
    pub filters: Filters,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Welcome {
    pub device_id: String,
    pub server_time: i64,
    pub config: DeviceConfig,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Send {
    pub msg_id: String,
    pub to: String,
    pub body: String,
    pub sim: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PairRequest {
    pub code: String,
    pub name: Option<String>,
    pub manufacturer: Option<String>,
    pub model: Option<String>,
    pub os_version: Option<String>,
    pub app_version: Option<String>,
    pub app_build: Option<i64>,
    pub fcm_token: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PairResponse {
    pub device_id: String,
    pub device_token: String,
    pub ws_url: String,
}

/// A decoded device → server or server → device frame.
#[derive(Debug, Clone, PartialEq)]
pub enum Frame {
    Hello(Hello),
    Status(Status),
    Inbound(Inbound),
    Health(Health),
    Welcome(Welcome),
    Config(DeviceConfig),
    Send(Send),
    Ack(Ack),
    Nack(Nack),
    SetForwarding(SetForwarding),
    SetSim(SetSim),
}

#[derive(Debug, Clone, PartialEq)]
pub struct DecodeError {
    /// `bad_frame`, `unsupported` or `invalid`.
    pub code: &'static str,
    pub message: String,
    /// The envelope id, when one could be read.
    pub id: Option<String>,
}

impl Frame {
    pub fn kind(&self) -> &'static str {
        match self {
            Frame::Hello(_) => "hello",
            Frame::Status(_) => "status",
            Frame::Inbound(_) => "inbound",
            Frame::Health(_) => "health",
            Frame::Welcome(_) => "welcome",
            Frame::Config(_) => "config",
            Frame::Send(_) => "send",
            Frame::Ack(_) => "ack",
            Frame::Nack(_) => "nack",
            Frame::SetForwarding(_) => "set_forwarding",
            Frame::SetSim(_) => "set_sim",
        }
    }

    pub fn data(&self) -> Value {
        let v = match self {
            Frame::Hello(d) => serde_json::to_value(d),
            Frame::Status(d) => serde_json::to_value(d),
            Frame::Inbound(d) => serde_json::to_value(d),
            Frame::Health(d) => serde_json::to_value(d),
            Frame::Welcome(d) => serde_json::to_value(d),
            Frame::Config(d) => serde_json::to_value(d),
            Frame::Send(d) => serde_json::to_value(d),
            Frame::Ack(d) => serde_json::to_value(d),
            Frame::Nack(d) => serde_json::to_value(d),
            Frame::SetForwarding(d) => serde_json::to_value(d),
            Frame::SetSim(d) => serde_json::to_value(d),
        };
        v.unwrap_or(Value::Null)
    }

    pub fn into_envelope(self, id: String) -> Envelope {
        Envelope::with_id(id, self.kind(), self.data())
    }
}

/// Parse a text frame into its envelope id and typed frame.
pub fn decode(text: &str) -> Result<(String, Frame), DecodeError> {
    let raw: Value = serde_json::from_str(text).map_err(|e| DecodeError {
        code: "bad_frame",
        message: format!("malformed JSON: {e}"),
        id: None,
    })?;
    let id = raw.get("id").and_then(Value::as_str).map(str::to_string);
    let err = |code, message: String| DecodeError {
        code,
        message,
        id: id.clone(),
    };
    let env: Envelope = serde_json::from_value(raw.clone())
        .map_err(|e| err("bad_frame", format!("bad envelope: {e}")))?;
    if env.v != VERSION {
        return Err(err("unsupported", format!("unsupported version {}", env.v)));
    }
    fn typed<T: for<'de> Deserialize<'de>>(d: Value) -> Result<T, String> {
        serde_json::from_value(d).map_err(|e| e.to_string())
    }
    let frame = match env.kind.as_str() {
        "hello" => typed(env.data).map(Frame::Hello),
        "status" => typed(env.data).map(Frame::Status),
        "inbound" => typed(env.data).map(Frame::Inbound),
        "health" => typed(env.data).map(Frame::Health),
        "welcome" => typed(env.data).map(Frame::Welcome),
        "config" => typed(env.data).map(Frame::Config),
        "send" => typed(env.data).map(Frame::Send),
        "ack" => typed(env.data).map(Frame::Ack),
        "nack" => typed(env.data).map(Frame::Nack),
        "set_forwarding" => typed(env.data).map(Frame::SetForwarding),
        "set_sim" => typed(env.data).map(Frame::SetSim),
        other => return Err(err("unsupported", format!("unknown type {other:?}"))),
    }
    .map_err(|m| err("invalid", m))?;
    Ok((env.id, frame))
}
