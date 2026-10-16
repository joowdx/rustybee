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
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Health {
    pub battery: Option<Battery>,
    pub network: Option<String>,
    pub permissions: Option<Permissions>,
    pub outbox: Option<i64>,
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
    /// Bumped on every filter change; the device echoes it in `set_filters`.
    #[serde(default)]
    pub filters_rev: i64,
}

/// Filters edited on the device, based on the `filters_rev` it last saw.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SetFilters {
    pub base_rev: i64,
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
    SetFilters(SetFilters),
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
            Frame::SetFilters(_) => "set_filters",
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
            Frame::SetFilters(d) => serde_json::to_value(d),
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
        "set_filters" => typed(env.data).map(Frame::SetFilters),
        other => return Err(err("unsupported", format!("unknown type {other:?}"))),
    }
    .map_err(|m| err("invalid", m))?;
    Ok((env.id, frame))
}
