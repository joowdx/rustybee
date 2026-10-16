//! HTTP surface: public API, device endpoints, web UI and static assets.

pub mod api;
pub mod auth;
pub mod device;
pub mod ui;

use crate::app::{App, Ctx};
use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::http::HeaderMap;
use axum::routing::get;
use std::net::{IpAddr, SocketAddr};
use tower_http::trace::TraceLayer;

pub const BODY_LIMIT: usize = 2 * 1024 * 1024;

pub fn router(app: App) -> Router {
    Router::new()
        .merge(api::routes())
        .merge(device::routes())
        .merge(ui::routes())
        .route("/static/{file}", get(crate::assets::serve))
        .layer(DefaultBodyLimit::max(BODY_LIMIT))
        .layer(TraceLayer::new_for_http())
        .with_state(app)
}

pub fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(str::trim)
}

/// Client IP (spec 005). With `RUSTYBEE_TRUST_PROXY=1`: `CF-Connecting-IP`,
/// else the rightmost `X-Forwarded-For` entry, the one the proxy appended.
pub fn client_ip(app: &Ctx, headers: &HeaderMap, peer: Option<SocketAddr>) -> IpAddr {
    if app.cfg.trust_proxy {
        let header = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
        let ip = header("cf-connecting-ip")
            .and_then(|v| v.trim().parse().ok())
            .or_else(|| {
                header("x-forwarded-for")
                    .and_then(|v| v.rsplit(',').next())
                    .and_then(|v| v.trim().parse().ok())
            });
        if let Some(ip) = ip {
            return ip;
        }
    }
    peer.map(|p| p.ip())
        .unwrap_or(IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED))
}

/// The request came through Cloudflare: proxy trusted and `CF-Connecting-IP` set.
pub fn via_cloudflare(app: &Ctx, headers: &HeaderMap) -> bool {
    app.cfg.trust_proxy && headers.contains_key("cf-connecting-ip")
}
