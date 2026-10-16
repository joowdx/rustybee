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

/// Client IP: `X-Forwarded-For` only when `RUSTYBEE_TRUST_PROXY=1`.
pub fn client_ip(app: &Ctx, headers: &HeaderMap, peer: Option<SocketAddr>) -> IpAddr {
    if app.cfg.trust_proxy
        && let Some(ip) = headers
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(',').next())
            .and_then(|v| v.trim().parse().ok())
    {
        return ip;
    }
    peer.map(|p| p.ip())
        .unwrap_or(IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED))
}
