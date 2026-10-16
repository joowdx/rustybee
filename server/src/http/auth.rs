//! Request authentication (spec 005): API keys, UI sessions, CSRF.

use crate::app::App;
use crate::error::ApiError;
use crate::store::auth;
use axum::extract::{FromRequestParts, Request};
use axum::http::request::Parts;
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Redirect, Response};

pub const COOKIE: &str = "rb_session";

/// A valid, unrevoked API key.
pub struct ApiKey;

fn key_in_query(query: Option<&str>) -> bool {
    query.is_some_and(|q| {
        q.split('&').any(|kv| {
            let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
            k.eq_ignore_ascii_case("api_key") || v.starts_with("rbk_") || v.starts_with("rbk%5F")
        })
    })
}

impl FromRequestParts<App> for ApiKey {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, app: &App) -> Result<Self, ApiError> {
        if key_in_query(parts.uri.query()) {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "key_in_query",
                "send the API key in the Authorization header, not the query string",
            ));
        }
        let Some(token) = super::bearer(&parts.headers).filter(|t| t.starts_with("rbk_")) else {
            return Err(ApiError::unauthorized("missing API key"));
        };
        let token = token.to_string();
        let key = app
            .store
            .read(move |c| auth::key_by_secret(c, &token))
            .await?;
        let Some(key) = key.filter(|k| k.revoked_at.is_none()) else {
            return Err(ApiError::unauthorized("invalid or revoked API key"));
        };
        let now = app.clock.now_ms();
        if key.last_used_at.is_none_or(|t| now - t >= 60_000) {
            let id = key.id;
            app.store
                .write(move |c| auth::touch_key(c, id, now))
                .await?;
        }
        Ok(ApiKey)
    }
}

pub fn cookie<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v)
}

/// A logged-in UI session.
#[derive(Debug, Clone, Copy)]
pub struct Session {
    pub id: i64,
}

fn to_login(parts: &Parts) -> Response {
    if parts.headers.contains_key("hx-request") {
        ([("HX-Redirect", "/login")], StatusCode::UNAUTHORIZED).into_response()
    } else {
        Redirect::to("/login").into_response()
    }
}

impl FromRequestParts<App> for Session {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, app: &App) -> Result<Self, Response> {
        let Some(token) = cookie(&parts.headers, COOKIE).map(str::to_string) else {
            return Err(to_login(parts));
        };
        let now = app.clock.now_ms();
        let s = app
            .store
            .read(move |c| auth::session_by_token(c, &token, now))
            .await
            .map_err(|e| ApiError::internal(e).into_response())?;
        let Some(s) = s else {
            return Err(to_login(parts));
        };
        if now - s.last_seen_at >= auth::SESSION_BUMP_MS {
            let id = s.id;
            let _ = app
                .store
                .write(move |c| auth::bump_session(c, id, now))
                .await;
        }
        Ok(Session { id: s.id })
    }
}

pub fn session_cookie(app: &App, token: &str) -> String {
    let secure = if app.cfg.secure_cookies() {
        "; Secure"
    } else {
        ""
    };
    format!(
        "{COOKIE}={token}; HttpOnly; SameSite=Strict; Path=/; Max-Age={}{secure}",
        auth::SESSION_TTL_MS / 1000
    )
}

pub fn clear_cookie() -> String {
    format!("{COOKIE}=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0")
}

/// Host part of an `Origin` or `Referer` value.
fn origin_host(v: &str) -> Option<&str> {
    let rest = v
        .strip_prefix("https://")
        .or_else(|| v.strip_prefix("http://"))?;
    Some(rest.split('/').next().unwrap_or(rest))
}

/// CSRF (spec 005): non-GET UI requests need an `Origin` (or `Referer`) matching `Host`.
pub async fn csrf(req: Request, next: Next) -> Response {
    if matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS) {
        return next.run(req).await;
    }
    let h = req.headers();
    let host = h.get(header::HOST).and_then(|v| v.to_str().ok());
    let origin = h
        .get(header::ORIGIN)
        .or_else(|| h.get(header::REFERER))
        .and_then(|v| v.to_str().ok())
        .and_then(origin_host);
    match (host, origin) {
        (Some(host), Some(origin)) if host.eq_ignore_ascii_case(origin) => next.run(req).await,
        _ => (StatusCode::FORBIDDEN, "cross-origin request refused").into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_key_in_query() {
        assert!(key_in_query(Some("api_key=x")));
        assert!(key_in_query(Some("a=1&token=rbk_abc")));
        assert!(!key_in_query(Some("limit=2")));
        assert!(!key_in_query(None));
    }

    #[test]
    fn origin_parsing() {
        assert_eq!(origin_host("https://a.b:8080"), Some("a.b:8080"));
        assert_eq!(origin_host("http://a.b/x/y"), Some("a.b"));
        assert_eq!(origin_host("null"), None);
    }
}
