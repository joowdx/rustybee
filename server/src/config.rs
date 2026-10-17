//! Settings from environment variables (spec 014), plus test-only knobs.

use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct Config {
    pub bind: String,
    pub data_dir: PathBuf,
    /// Base URL without a trailing slash.
    pub public_url: String,
    pub admin_password: Option<String>,
    pub ping_s: u32,
    /// Close a device socket after this much silence (`2 × ping_s + 30 s`).
    pub idle_timeout: Duration,
    pub message_ttl: Duration,
    pub trust_proxy: bool,
    pub fcm_credentials: Option<PathBuf>,
    /// Turnstile on Cloudflare-proxied logins (spec 005).
    pub turnstile: Option<Turnstile>,
    /// Most `queued` messages one device may hold (spec 007).
    pub max_queued: u32,
    /// Accept recipients shorter than 7 digits (spec 004).
    pub allow_short_codes: bool,
    /// Prune finished messages and webhook deliveries older than this (spec 003).
    pub retention_days: Option<u32>,
    /// Daily backups go here when set (spec 003).
    pub backup_dir: Option<PathBuf>,
    /// Daily backups kept in `backup_dir`; older ones are deleted.
    pub backup_keep: u32,
    // ---- injectable timings and endpoints (tests) ----
    pub inflight_timeout: Duration,
    pub status_timeout: Duration,
    pub sweep_interval: Duration,
    pub webhook_backoff: Vec<Duration>,
    pub webhook_lease: Duration,
    pub webhook_timeout: Duration,
    /// Overall timeout of the shared outbound HTTP client (FCM, Turnstile).
    pub http_timeout: Duration,
    pub fcm_interval: Duration,
    pub fcm_oauth_url: String,
    pub fcm_base_url: String,
    /// Login throttle: failures allowed per window.
    pub login_window: Duration,
    /// Failed login-code sends in a row before password login unlocks.
    pub otp_max_failures: u32,
    /// A login code not sent within this long counts as a failed send.
    pub otp_send_timeout: Duration,
    pub turnstile_verify_url: String,
}

#[derive(Debug, Clone)]
pub struct Turnstile {
    pub site_key: String,
    pub secret: String,
}

fn env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.trim().is_empty())
}

fn env_num<T: std::str::FromStr>(key: &str) -> Result<Option<T>, String> {
    match env(key) {
        None => Ok(None),
        Some(v) => v
            .trim()
            .parse()
            .map(Some)
            .map_err(|_| format!("{key}: not a number: {v:?}")),
    }
}

impl Config {
    pub fn from_env() -> Result<Config, String> {
        let bind = env("RUSTYBEE_BIND").unwrap_or_else(|| "127.0.0.1:8080".into());
        let data_dir = PathBuf::from(env("RUSTYBEE_DATA_DIR").unwrap_or_else(|| "./data".into()));
        let public_url = env("RUSTYBEE_PUBLIC_URL").unwrap_or_else(|| format!("http://{bind}"));
        let ping_s: u32 = env_num("RUSTYBEE_PING_SECONDS")?.unwrap_or(270).max(10);
        let ttl_h: u64 = env_num("RUSTYBEE_MESSAGE_TTL_HOURS")?.unwrap_or(24).max(1);
        let trust_proxy = matches!(
            env("RUSTYBEE_TRUST_PROXY").as_deref(),
            Some("1" | "true" | "yes")
        );
        let mut c = Config::base(bind, data_dir, &public_url);
        c.admin_password = env("RUSTYBEE_ADMIN_PASSWORD");
        c.ping_s = ping_s;
        c.idle_timeout = Duration::from_secs(2 * ping_s as u64 + 30);
        c.message_ttl = Duration::from_secs(ttl_h * 3600);
        c.trust_proxy = trust_proxy;
        c.fcm_credentials = env("RUSTYBEE_FCM_CREDENTIALS").map(PathBuf::from);
        c.otp_max_failures = env_num("RUSTYBEE_OTP_MAX_FAILURES")?.unwrap_or(3).max(1);
        c.max_queued = env_num("RUSTYBEE_MAX_QUEUED")?.unwrap_or(1000).max(1);
        c.allow_short_codes = matches!(
            env("RUSTYBEE_ALLOW_SHORT_CODES").as_deref(),
            Some("1" | "true" | "yes")
        );
        c.retention_days = env_num::<u32>("RUSTYBEE_RETENTION_DAYS")?.map(|d| d.max(1));
        c.backup_dir = env("RUSTYBEE_BACKUP_DIR").map(PathBuf::from);
        c.backup_keep = env_num("RUSTYBEE_BACKUP_KEEP")?.unwrap_or(7).max(1);
        c.turnstile = match (
            env("RUSTYBEE_TURNSTILE_SITE_KEY"),
            env("RUSTYBEE_TURNSTILE_SECRET"),
        ) {
            (Some(site_key), Some(secret)) => Some(Turnstile { site_key, secret }),
            (None, None) => None,
            _ => {
                return Err(
                    "RUSTYBEE_TURNSTILE_SITE_KEY and RUSTYBEE_TURNSTILE_SECRET must be set together"
                        .into(),
                );
            }
        };
        Ok(c)
    }

    fn base(bind: String, data_dir: PathBuf, public_url: &str) -> Config {
        Config {
            bind,
            data_dir,
            public_url: public_url.trim_end_matches('/').to_string(),
            admin_password: None,
            ping_s: 270,
            idle_timeout: Duration::from_secs(570),
            message_ttl: Duration::from_secs(24 * 3600),
            trust_proxy: false,
            fcm_credentials: None,
            turnstile: None,
            max_queued: 1000,
            allow_short_codes: false,
            retention_days: None,
            backup_dir: None,
            backup_keep: 7,
            inflight_timeout: Duration::from_secs(120),
            status_timeout: Duration::from_secs(24 * 3600),
            sweep_interval: Duration::from_secs(60),
            webhook_backoff: crate::domain::default_backoff(),
            webhook_lease: Duration::from_secs(60),
            webhook_timeout: Duration::from_secs(10),
            http_timeout: Duration::from_secs(15),
            fcm_interval: Duration::from_secs(60),
            fcm_oauth_url: "https://oauth2.googleapis.com/token".into(),
            fcm_base_url: "https://fcm.googleapis.com".into(),
            login_window: Duration::from_secs(60),
            otp_max_failures: 3,
            otp_send_timeout: Duration::from_secs(90),
            turnstile_verify_url: "https://challenges.cloudflare.com/turnstile/v0/siteverify"
                .into(),
        }
    }

    /// Defaults for tests: ephemeral port, the given data dir, production timings.
    pub fn for_tests(data_dir: PathBuf) -> Config {
        Config::base("127.0.0.1:0".into(), data_dir, "http://127.0.0.1")
    }

    pub fn db_path(&self) -> PathBuf {
        self.data_dir.join("rustybee.db")
    }

    pub fn secure_cookies(&self) -> bool {
        self.public_url.starts_with("https://")
    }

    /// `ws_url` for pairing: http→ws, https→wss, plus `/api/device/ws`.
    pub fn ws_url(&self) -> String {
        let base = if let Some(rest) = self.public_url.strip_prefix("https://") {
            format!("wss://{rest}")
        } else if let Some(rest) = self.public_url.strip_prefix("http://") {
            format!("ws://{rest}")
        } else {
            self.public_url.clone()
        };
        format!("{base}/api/device/ws")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ws_url_substitution() {
        let mut c = Config::for_tests("/tmp".into());
        c.public_url = "https://sms.example.org".into();
        assert_eq!(c.ws_url(), "wss://sms.example.org/api/device/ws");
        c.public_url = "http://10.0.0.2:8080".into();
        assert_eq!(c.ws_url(), "ws://10.0.0.2:8080/api/device/ws");
    }
}
