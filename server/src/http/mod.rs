//! HTTP surface: public API, device endpoints, web UI and static assets.

pub mod api;
pub mod auth;
pub mod device;
pub mod ui;

use crate::app::{App, Ctx};
use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::http::HeaderMap;
use axum::http::HeaderValue;
use axum::http::header::{
    CONTENT_SECURITY_POLICY, REFERRER_POLICY, X_CONTENT_TYPE_OPTIONS, X_FRAME_OPTIONS,
};
use axum::routing::get;
use std::net::{IpAddr, SocketAddr};
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::trace::TraceLayer;

pub const BODY_LIMIT: usize = 2 * 1024 * 1024;

/// Content-Security-Policy for every response (spec 010). Scripts only from
/// this origin and Turnstile; no inline or eval'd script.
pub const CSP: &str = "script-src 'self' https://challenges.cloudflare.com; \
    frame-src https://challenges.cloudflare.com; frame-ancestors 'none'; \
    object-src 'none'; base-uri 'none'";

pub fn router(app: App) -> Router {
    Router::new()
        .merge(api::routes())
        .merge(device::routes())
        .merge(ui::routes())
        .route("/static/{file}", get(crate::assets::serve))
        .layer(DefaultBodyLimit::max(BODY_LIMIT))
        .layer(SetResponseHeaderLayer::if_not_present(
            X_FRAME_OPTIONS,
            HeaderValue::from_static("DENY"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            REFERRER_POLICY,
            HeaderValue::from_static("same-origin"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(CSP),
        ))
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

/// The TCP peer may be a reverse proxy: loopback, IPv4 RFC 1918 or IPv6
/// unique-local (spec 005). IPv4-mapped IPv6 peers are checked as IPv4.
pub fn local_peer(ip: IpAddr) -> bool {
    let ip = match ip {
        IpAddr::V6(v6) => v6
            .to_ipv4_mapped()
            .map(IpAddr::V4)
            .unwrap_or(IpAddr::V6(v6)),
        v4 => v4,
    };
    match ip {
        IpAddr::V4(v4) => v4.is_loopback() || v4.is_private(),
        IpAddr::V6(v6) => v6.is_loopback() || (v6.segments()[0] & 0xfe00) == 0xfc00,
    }
}

/// Proxy headers count only with `RUSTYBEE_TRUST_PROXY=1` and a local peer.
fn trusted(app: &Ctx, peer: Option<SocketAddr>) -> bool {
    app.cfg.trust_proxy && peer.is_some_and(|p| local_peer(p.ip()))
}

/// Client IP (spec 005). With `RUSTYBEE_TRUST_PROXY=1` and a local peer:
/// `CF-Connecting-IP`, else the rightmost `X-Forwarded-For` entry, the one
/// the proxy appended. Otherwise the peer address.
pub fn client_ip(app: &Ctx, headers: &HeaderMap, peer: Option<SocketAddr>) -> IpAddr {
    if trusted(app, peer) {
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

/// The request came through Cloudflare: proxy trusted, the peer local and
/// `CF-Connecting-IP` set.
pub fn via_cloudflare(app: &Ctx, headers: &HeaderMap, peer: Option<SocketAddr>) -> bool {
    trusted(app, peer) && headers.contains_key("cf-connecting-ip")
}

#[cfg(test)]
mod tests {
    use super::local_peer;

    #[test]
    fn local_peers() {
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "172.31.255.255",
            "192.168.1.1",
            "::1",
            "fd00::1",
            "fc00::1",
            "::ffff:10.0.0.1",
            "::ffff:127.0.0.1",
        ] {
            assert!(local_peer(ip.parse().unwrap()), "{ip}");
        }
        for ip in [
            "203.0.113.9",
            "172.32.0.1",
            "8.8.8.8",
            "2001:db8::1",
            "fe80::1",
            "::ffff:203.0.113.9",
        ] {
            assert!(!local_peer(ip.parse().unwrap()), "{ip}");
        }
    }
}
