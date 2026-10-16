//! Optional FCM wake (spec 009). Only a `{"t":"wake"}` data message is ever sent.

use crate::app::App;
use crate::config::Config;
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use rusqlite::params;
use serde::Deserialize;
use serde_json::json;
use std::sync::{Arc, Mutex};

const SCOPE: &str = "https://www.googleapis.com/auth/firebase.messaging";
const QUIET_MS: i64 = 3 * 60 * 1000;
const WAKE_EVERY_MS: i64 = 15 * 60 * 1000;

#[derive(Deserialize)]
struct Credentials {
    project_id: String,
    client_email: String,
    private_key: String,
}

pub struct Fcm {
    project_id: String,
    client_email: String,
    key: EncodingKey,
    oauth_url: String,
    base_url: String,
    token: Mutex<Option<(String, i64)>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WakeResult {
    Sent,
    Unregistered,
    Failed(String),
}

impl Fcm {
    /// `None` unless `RUSTYBEE_FCM_CREDENTIALS` is configured.
    pub fn from_config(cfg: &Config) -> Result<Option<Fcm>, String> {
        let Some(path) = &cfg.fcm_credentials else {
            return Ok(None);
        };
        let raw =
            std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
        let c: Credentials =
            serde_json::from_str(&raw).map_err(|e| format!("FCM credentials: {e}"))?;
        let key = EncodingKey::from_rsa_pem(c.private_key.as_bytes())
            .map_err(|e| format!("FCM private key: {e}"))?;
        Ok(Some(Fcm {
            project_id: c.project_id,
            client_email: c.client_email,
            key,
            oauth_url: cfg.fcm_oauth_url.clone(),
            base_url: cfg.fcm_base_url.trim_end_matches('/').to_string(),
            token: Mutex::new(None),
        }))
    }

    async fn access_token(&self, app: &App) -> Result<String, String> {
        let now = app.clock.now_ms();
        if let Some((t, exp)) = self.token.lock().unwrap().clone()
            && now < exp - 5 * 60 * 1000
        {
            return Ok(t);
        }
        let iat = now / 1000;
        let claims = json!({
            "iss": self.client_email,
            "scope": SCOPE,
            "aud": self.oauth_url,
            "iat": iat,
            "exp": iat + 3600,
        });
        let jwt = jsonwebtoken::encode(&Header::new(Algorithm::RS256), &claims, &self.key)
            .map_err(|e| e.to_string())?;
        let body = format!(
            "grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Ajwt-bearer&assertion={jwt}"
        );
        let res = app
            .http
            .post(&self.oauth_url)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(body)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if !res.status().is_success() {
            return Err(format!("token endpoint returned {}", res.status()));
        }
        #[derive(Deserialize)]
        struct Tok {
            access_token: String,
            expires_in: Option<i64>,
        }
        let tok: Tok = res.json().await.map_err(|e| e.to_string())?;
        let exp = now + tok.expires_in.unwrap_or(3600) * 1000;
        *self.token.lock().unwrap() = Some((tok.access_token.clone(), exp));
        Ok(tok.access_token)
    }

    pub async fn wake(&self, app: &App, token: &str) -> WakeResult {
        let access = match self.access_token(app).await {
            Ok(t) => t,
            Err(e) => return WakeResult::Failed(e),
        };
        let url = format!(
            "{}/v1/projects/{}/messages:send",
            self.base_url, self.project_id
        );
        let body = json!({"message": {"token": token, "data": {"t": "wake"},
            "android": {"priority": "HIGH", "ttl": "600s"}}});
        match app
            .http
            .post(&url)
            .bearer_auth(access)
            .json(&body)
            .send()
            .await
        {
            Ok(r) if r.status().is_success() => WakeResult::Sent,
            Ok(r) => {
                let status = r.status().as_u16();
                let text = r.text().await.unwrap_or_default();
                if status == 404 || text.contains("UNREGISTERED") {
                    WakeResult::Unregistered
                } else {
                    WakeResult::Failed(format!(
                        "{status}: {}",
                        text.chars().take(200).collect::<String>()
                    ))
                }
            }
            Err(e) => WakeResult::Failed(e.to_string()),
        }
    }
}

pub async fn run(app: App, fcm: Arc<Fcm>) {
    loop {
        if let Err(e) = check_once(&app, &fcm).await {
            tracing::warn!(error = %e, "fcm wake check");
        }
        tokio::select! {
            _ = app.fcm_notify.notified() => {}
            _ = tokio::time::sleep(app.cfg.fcm_interval) => {}
        }
    }
}

/// Evaluate the wake rule once. Returns the device ids that were woken.
pub async fn check_once(app: &App, fcm: &Fcm) -> crate::store::Result<Vec<i64>> {
    let now = app.clock.now_ms();
    let due: Vec<(i64, String)> = app
        .store
        .write(move |c| {
            let mut st = c.prepare(
                "SELECT id, fcm_token FROM devices d WHERE connected=0 AND fcm_token IS NOT NULL
                   AND (last_seen_at IS NULL OR last_seen_at<?1)
                   AND (last_wake_at IS NULL OR last_wake_at<?2)
                   AND EXISTS(SELECT 1 FROM messages m WHERE m.device_id=d.id AND (m.state='queued'
                       OR (m.state='dispatched' AND m.device_acked_at IS NULL)))",
            )?;
            let due: Vec<(i64, String)> = st
                .query_map(params![now - QUIET_MS, now - WAKE_EVERY_MS], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })?
                .collect::<rusqlite::Result<_>>()?;
            for (id, _) in &due {
                c.execute(
                    "UPDATE devices SET last_wake_at=?2 WHERE id=?1",
                    params![id, now],
                )?;
            }
            Ok(due)
        })
        .await?;
    let mut woken = vec![];
    for (id, token) in due {
        match fcm.wake(app, &token).await {
            WakeResult::Sent => {
                tracing::info!(device_id = id, "FCM wake sent");
                woken.push(id);
            }
            WakeResult::Unregistered => {
                tracing::info!(device_id = id, "FCM token unregistered; cleared");
                app.store
                    .write(move |c| {
                        c.execute("UPDATE devices SET fcm_token=NULL WHERE id=?1", [id])
                            .map(|_| ())
                    })
                    .await?;
            }
            WakeResult::Failed(e) => tracing::warn!(device_id = id, error = %e, "FCM wake failed"),
        }
    }
    Ok(woken)
}
