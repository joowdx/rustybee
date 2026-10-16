//! Login codes texted to the admin phone, and which login methods are open
//! (spec 005).
//!
//! Password login is a fallback: it is open only while no admin phone is
//! set, no enabled device is connected, or the last
//! `cfg.otp_max_failures` code sends in a row failed.

use crate::app::{App, UiEvent};
use crate::domain::lifecycle::State;
use crate::domain::phone;
use crate::store::messages::{self, NewMessage};
use crate::store::{self, auth, devices, get_setting, set_setting};
use crate::util::sha256;
use rand::Rng;
use rusqlite::Connection;
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::MutexGuard;

pub const CODE_TTL_MS: i64 = 5 * 60 * 1000;
pub const MAX_ATTEMPTS: u32 = 5;
const SEND_GAP_MS: i64 = 30 * 1000;
const SEND_WINDOW_MS: i64 = 15 * 60 * 1000;
const SEND_WINDOW_MAX: usize = 5;
const GLOBAL_WINDOW_MS: i64 = 3600 * 1000;
const GLOBAL_WINDOW_MAX: usize = 20;
const REDACTED: &str = "Login code (hidden)";
const PHONE_KEY: &str = "admin_phone";
const FAILURES_KEY: &str = "otp_send_failures";

/// What a code is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    /// Log in to the dashboard.
    Login,
    /// Confirm a new admin phone number from the settings page.
    Verify,
}

struct Pending {
    hash: Vec<u8>,
    phone: String,
    expires_at: i64,
    attempts: u32,
}

struct Outstanding {
    msg_id: i64,
    created_at: i64,
    /// Already counted as a failure after `otp_send_timeout`.
    counted: bool,
}

/// In-memory code state; lost on restart, which only means sending a new code.
#[derive(Default)]
pub struct OtpState {
    login: Option<Pending>,
    verify: Option<Pending>,
    outstanding: Vec<Outstanding>,
    sends: HashMap<IpAddr, Vec<i64>>,
    all_sends: Vec<i64>,
}

impl OtpState {
    fn slot(&mut self, p: Purpose) -> &mut Option<Pending> {
        match p {
            Purpose::Login => &mut self.login,
            Purpose::Verify => &mut self.verify,
        }
    }
}

fn state(app: &App) -> MutexGuard<'_, OtpState> {
    app.otp.lock().unwrap_or_else(|e| e.into_inner())
}

/// Which login methods the login page offers and accepts.
#[derive(Debug, Clone, Default)]
pub struct Methods {
    pub otp: bool,
    pub password: bool,
    pub has_password: bool,
    /// At least one admin phone is set.
    pub has_phone: bool,
    pub online: bool,
    pub failures: u32,
}

/// Admin phones, stored newline-separated; an older single value is a list of one.
pub fn admin_phones(c: &Connection) -> rusqlite::Result<Vec<String>> {
    Ok(get_setting(c, PHONE_KEY)?
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .collect())
}

fn save_phones(c: &Connection, list: &[String]) -> rusqlite::Result<()> {
    if list.is_empty() {
        c.execute("DELETE FROM settings WHERE key=?1", [PHONE_KEY])
            .map(|_| ())
    } else {
        set_setting(c, PHONE_KEY, &list.join("\n"))
    }
}

/// Add an admin phone; false when it is already in the list.
pub fn add_admin_phone(c: &Connection, phone: &str) -> rusqlite::Result<bool> {
    let mut list = admin_phones(c)?;
    if list.iter().any(|p| p == phone) {
        return Ok(false);
    }
    list.push(phone.to_string());
    save_phones(c, &list).map(|_| true)
}

pub fn remove_admin_phone(c: &Connection, phone: &str) -> rusqlite::Result<()> {
    let mut list = admin_phones(c)?;
    list.retain(|p| p != phone);
    save_phones(c, &list)
}

/// The last four digits of a phone number.
pub fn last4(phone: &str) -> String {
    let digits: Vec<char> = phone.chars().filter(char::is_ascii_digit).collect();
    digits[digits.len().saturating_sub(4)..].iter().collect()
}

pub fn send_failures(c: &Connection) -> rusqlite::Result<u32> {
    Ok(get_setting(c, FAILURES_KEY)?
        .and_then(|v| v.parse().ok())
        .unwrap_or(0))
}

pub fn reset_failures(c: &Connection) -> rusqlite::Result<()> {
    set_setting(c, FAILURES_KEY, "0")
}

pub fn mask(phone: &str) -> String {
    format!("•••• {}", last4(phone))
}

/// Enabled devices with a live socket, the default one first.
async fn online_devices(app: &App) -> store::Result<Vec<i64>> {
    let mut list = app.store.read(devices::list).await?;
    list.retain(|d| d.enabled && app.hub.is_connected(d.id));
    list.sort_by_key(|d| !d.is_default);
    Ok(list.into_iter().map(|d| d.id).collect())
}

pub async fn methods(app: &App) -> store::Result<Methods> {
    resolve(app).await?;
    let (hash, phone, failures) = app
        .store
        .read(|c| Ok((auth::password_hash(c)?, admin_phones(c)?, send_failures(c)?)))
        .await?;
    let online = !online_devices(app).await?.is_empty();
    let has_password = hash.is_some();
    let has_phone = !phone.is_empty();
    let otp = has_phone && online;
    let fallback = !has_phone || !online || failures >= app.cfg.otp_max_failures;
    Ok(Methods {
        otp,
        password: has_password && fallback,
        has_password,
        has_phone,
        online,
        failures,
    })
}

#[derive(Debug)]
pub enum SendError {
    NoPhone,
    /// No admin phone ends in the given last four digits.
    NotRegistered,
    /// The last four digits were not exactly four digits.
    BadDigits,
    /// The number being confirmed is already an admin phone.
    AlreadyAdded,
    InvalidPhone(String),
    Offline,
    TooSoon,
    Store(store::StoreError),
}

impl std::fmt::Display for SendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SendError::NoPhone => f.write_str("No admin phone is set."),
            SendError::NotRegistered => f.write_str("Sorry, that number isn't registered."),
            SendError::BadDigits => f.write_str("Enter the last 4 digits of your phone number."),
            SendError::AlreadyAdded => f.write_str("That number is already a login phone."),
            SendError::InvalidPhone(e) => write!(f, "That phone number is not valid: {e}."),
            SendError::Offline => f.write_str("No device is online to send the code."),
            SendError::TooSoon => {
                f.write_str("A code was sent moments ago. Wait a little before asking again.")
            }
            SendError::Store(e) => write!(f, "{e}"),
        }
    }
}

impl From<store::StoreError> for SendError {
    fn from(e: store::StoreError) -> Self {
        SendError::Store(e)
    }
}

/// Rate limits: per IP one send per 30 s and five per 15 min; twenty per
/// hour overall. Records the send when allowed.
fn allow_send(st: &mut OtpState, ip: IpAddr, now: i64) -> bool {
    st.all_sends.retain(|t| now - t < GLOBAL_WINDOW_MS);
    st.sends.retain(|_, v| {
        v.retain(|t| now - t < SEND_WINDOW_MS);
        !v.is_empty()
    });
    let mine = st.sends.get(&ip).map(Vec::as_slice).unwrap_or_default();
    if mine.len() >= SEND_WINDOW_MAX
        || mine.last().is_some_and(|t| now - t < SEND_GAP_MS)
        || st.all_sends.len() >= GLOBAL_WINDOW_MAX
    {
        return false;
    }
    st.sends.entry(ip).or_default().push(now);
    st.all_sends.push(now);
    true
}

/// Text a fresh code. For `Login`, `to` is the last four digits and the code
/// goes to every admin phone ending in them; for `Verify` it is the number
/// being confirmed. A new code replaces any pending one for the same purpose.
pub async fn send(app: &App, purpose: Purpose, to: &str, ip: IpAddr) -> Result<(), SendError> {
    let phones = app.store.read(admin_phones).await?;
    if purpose == Purpose::Login && phones.is_empty() {
        return Err(SendError::NoPhone);
    }
    let Some(&device_id) = online_devices(app).await?.first() else {
        return Err(SendError::Offline);
    };
    let to: Vec<String> = match purpose {
        Purpose::Verify => {
            let p = phone::normalize(to).map_err(SendError::InvalidPhone)?;
            if phones.contains(&p) {
                return Err(SendError::AlreadyAdded);
            }
            vec![p]
        }
        Purpose::Login => {
            let digits = to.trim();
            if digits.len() != 4 || !digits.bytes().all(|b| b.is_ascii_digit()) {
                return Err(SendError::BadDigits);
            }
            let hits: Vec<String> = phones.into_iter().filter(|p| last4(p) == digits).collect();
            if hits.is_empty() {
                return Err(SendError::NotRegistered);
            }
            hits
        }
    };
    let now = app.clock.now_ms();
    if !allow_send(&mut state(app), ip, now) {
        return Err(SendError::TooSoon);
    }
    let code = format!("{:06}", rand::thread_rng().gen_range(0..1_000_000));
    let body = format!("rustybee login code: {code}. Expires in 5 minutes.");
    let items: Vec<NewMessage> = to
        .iter()
        .map(|p| NewMessage {
            to: p.clone(),
            body: body.clone(),
        })
        .collect();
    let rows = app
        .store
        .write(move |c| {
            let (_, rows) = messages::create_batch(c, device_id, &items, None, now)?;
            for row in &rows {
                messages::mark_internal(c, row.id)?;
            }
            Ok(rows)
        })
        .await?;
    {
        let mut st = state(app);
        *st.slot(purpose) = Some(Pending {
            hash: sha256(&code),
            phone: to[0].clone(),
            expires_at: now + CODE_TTL_MS,
            attempts: 0,
        });
        st.outstanding.extend(rows.iter().map(|row| Outstanding {
            msg_id: row.id,
            created_at: now,
            counted: false,
        }));
    }
    app.hub.notify(device_id);
    for row in rows {
        app.publish(UiEvent::Message(row.public_id));
    }
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
pub enum Verify {
    /// The code matched; carries the phone it was sent to.
    Ok(String),
    Wrong,
    /// No code pending, it expired, or it ran out of attempts.
    Expired,
}

fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Check a code. Each code allows `MAX_ATTEMPTS` tries and is single-use.
pub fn verify(app: &App, purpose: Purpose, code: &str) -> Verify {
    let now = app.clock.now_ms();
    let code: String = code.chars().filter(char::is_ascii_digit).collect();
    let mut st = state(app);
    let slot = st.slot(purpose);
    let Some(p) = slot.as_mut().filter(|p| p.expires_at > now) else {
        *slot = None;
        return Verify::Expired;
    };
    if ct_eq(&p.hash, &sha256(&code)) {
        let phone = p.phone.clone();
        *slot = None;
        return Verify::Ok(phone);
    }
    p.attempts += 1;
    if p.attempts >= MAX_ATTEMPTS {
        *slot = None;
    }
    Verify::Wrong
}

/// Whether a code for `purpose` is waiting to be entered.
pub fn pending(app: &App, purpose: Purpose) -> bool {
    let now = app.clock.now_ms();
    state(app)
        .slot(purpose)
        .as_ref()
        .is_some_and(|p| p.expires_at > now)
}

/// Last four digits of the number a pending login code went to.
pub fn pending_last4(app: &App) -> Option<String> {
    let now = app.clock.now_ms();
    state(app)
        .login
        .as_ref()
        .filter(|p| p.expires_at > now)
        .map(|p| last4(&p.phone))
}

/// Settle code messages that finished: a send resets the failure counter,
/// a failure, expiry or no send within `otp_send_timeout` adds one. Bodies
/// of finished code messages are redacted. Runs from the sweeper and
/// before the login page decides which methods to offer.
pub async fn resolve(app: &App) -> store::Result<()> {
    let list: Vec<(i64, i64, bool)> = state(app)
        .outstanding
        .iter()
        .map(|o| (o.msg_id, o.created_at, o.counted))
        .collect();
    if list.is_empty() {
        return Ok(());
    }
    let now = app.clock.now_ms();
    let timeout = app.cfg.otp_send_timeout.as_millis() as i64;
    let (done, counted) = app
        .store
        .write(move |c| {
            let (mut done, mut counted) = (vec![], vec![]);
            for (id, at, was_counted) in list {
                let st = messages::by_id(c, id)?.map(|m| m.state);
                let finished = match st {
                    Some(State::Sent | State::Delivered) => {
                        reset_failures(c)?;
                        true
                    }
                    None | Some(State::Failed | State::Expired) => {
                        if !was_counted {
                            set_setting(c, FAILURES_KEY, &(send_failures(c)? + 1).to_string())?;
                        }
                        true
                    }
                    Some(_) => {
                        if !was_counted && now - at >= timeout {
                            set_setting(c, FAILURES_KEY, &(send_failures(c)? + 1).to_string())?;
                            counted.push(id);
                        }
                        false
                    }
                };
                if finished {
                    messages::redact(c, id, REDACTED)?;
                    done.push(id);
                }
            }
            Ok((done, counted))
        })
        .await?;
    let mut st = state(app);
    st.outstanding.retain(|o| !done.contains(&o.msg_id));
    for o in &mut st.outstanding {
        if counted.contains(&o.msg_id) {
            o.counted = true;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_all_but_last_four() {
        assert_eq!(mask("+639171234567"), "•••• 4567");
        assert_eq!(last4("+12"), "12");
    }

    #[test]
    fn constant_time_eq() {
        assert!(ct_eq(b"abc", b"abc"));
        assert!(!ct_eq(b"abc", b"abd"));
        assert!(!ct_eq(b"abc", b"ab"));
    }

    #[test]
    fn send_rate_limits() {
        let mut st = OtpState::default();
        let ip: IpAddr = "10.0.0.1".parse().unwrap();
        assert!(allow_send(&mut st, ip, 0));
        assert!(!allow_send(&mut st, ip, 10_000));
        let mut t = 0;
        for _ in 0..4 {
            t += SEND_GAP_MS;
            assert!(allow_send(&mut st, ip, t));
        }
        assert!(!allow_send(&mut st, ip, t + SEND_GAP_MS));
        assert!(allow_send(&mut st, "10.0.0.2".parse().unwrap(), t));
        assert!(allow_send(&mut st, ip, SEND_WINDOW_MS + 1));
    }
}
