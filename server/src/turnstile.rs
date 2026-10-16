//! Cloudflare Turnstile on logins that came through Cloudflare (spec 005).

use crate::app::Ctx;
use axum::http::HeaderMap;
use serde::Deserialize;
use std::net::IpAddr;
use std::time::Duration;

/// Site key to render on the login page, when Turnstile applies to this request.
pub fn site_key<'a>(app: &'a Ctx, headers: &HeaderMap) -> Option<&'a str> {
    let t = app.cfg.turnstile.as_ref()?;
    crate::http::via_cloudflare(app, headers).then_some(t.site_key.as_str())
}

#[derive(Deserialize)]
struct Siteverify {
    success: bool,
}

/// True when Turnstile does not apply, or Siteverify accepts the token.
/// Fails closed on a missing token and on any Siteverify error.
pub async fn passes(app: &Ctx, headers: &HeaderMap, token: &str, ip: IpAddr) -> bool {
    let Some(t) = app.cfg.turnstile.as_ref() else {
        return true;
    };
    if !crate::http::via_cloudflare(app, headers) {
        return true;
    }
    if token.is_empty() {
        return false;
    }
    let ip = ip.to_string();
    let res = app
        .http
        .post(&app.cfg.turnstile_verify_url)
        .timeout(Duration::from_secs(10))
        .form(&[
            ("secret", t.secret.as_str()),
            ("response", token),
            ("remoteip", ip.as_str()),
        ])
        .send()
        .await;
    match res {
        Ok(r) => match r.json::<Siteverify>().await {
            Ok(v) => v.success,
            Err(e) => {
                tracing::warn!(error = %e, "turnstile siteverify: bad response");
                false
            }
        },
        Err(e) => {
            tracing::warn!(error = %e, "turnstile siteverify failed");
            false
        }
    }
}
