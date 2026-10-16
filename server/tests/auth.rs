//! Authentication (spec 005): sessions, CSRF, login throttle, API keys.

mod common;

use common::*;
use rustybee::store::auth;
use serde_json::Value;

#[tokio::test]
async fn ac_005_01_login_sets_cookie_wrong_password_401() {
    let s = server().await;
    s.set_password().await;
    let r = s
        .http
        .post(s.url("/login"))
        .header("Origin", &s.base)
        .form(&[("password", "wrong password!")])
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
    assert!(r.headers().get("set-cookie").is_none());
    let r = s
        .http
        .post(s.url("/login"))
        .header("Origin", &s.base)
        .form(&[("password", PASSWORD)])
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 303);
    assert_eq!(r.headers()["location"], "/");
    let c = r.headers()["set-cookie"].to_str().unwrap();
    assert!(
        c.starts_with("rb_session=") && c.contains("HttpOnly") && c.contains("SameSite=Strict")
    );
    let cookie = c.split(';').next().unwrap();
    let r = s
        .http
        .get(s.url("/"))
        .header("Cookie", cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    // Logout ends the session.
    let r = s
        .http
        .post(s.url("/logout"))
        .header("Origin", &s.base)
        .header("Cookie", cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 303);
    let r = s
        .http
        .get(s.url("/"))
        .header("Cookie", cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 303);
    s.stop().await;
}

#[tokio::test]
async fn ac_005_02_ui_redirects_api_401() {
    let s = server().await;
    let r = s.http.get(s.url("/messages")).send().await.unwrap();
    assert_eq!(r.status(), 303);
    assert_eq!(r.headers()["location"], "/login");
    let r = s
        .http
        .get(s.url("/devices"))
        .header("Cookie", "rb_session=bogus")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 303);
    // htmx requests get HX-Redirect instead.
    let r = s
        .http
        .get(s.url("/partials/dashboard"))
        .header("HX-Request", "true")
        .send()
        .await
        .unwrap();
    assert_eq!(r.headers()["hx-redirect"], "/login");
    let r = s.http.get(s.url("/api/v1/messages")).send().await.unwrap();
    assert_eq!(r.status(), 401);
    let v: Value = r.json().await.unwrap();
    assert_eq!(v["error"]["code"], "unauthorized");
    s.stop().await;
}

#[tokio::test]
async fn ac_005_03_foreign_origin_is_403() {
    let s = server().await;
    let cookie = s.login().await;
    for origin in [Some("https://evil.example"), None] {
        let mut req = s
            .http
            .post(s.url("/keys"))
            .header("Cookie", &cookie)
            .form(&[("name", "x")]);
        if let Some(o) = origin {
            req = req.header("Origin", o);
        }
        assert_eq!(req.send().await.unwrap().status(), 403);
    }
    // Same origin passes; Referer is accepted when Origin is absent.
    let r = s
        .http
        .post(s.url("/keys"))
        .header("Cookie", &cookie)
        .header("Referer", format!("{}/keys", s.base))
        .form(&[("name", "x")])
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    s.stop().await;
}

#[tokio::test]
async fn ac_005_04_sixth_failed_login_is_429() {
    let s = server().await;
    s.set_password().await;
    let attempt = |pw: &'static str| {
        s.http
            .post(s.url("/login"))
            .header("Origin", &s.base)
            .form(&[("password", pw)])
            .send()
    };
    for _ in 0..5 {
        assert_eq!(attempt("nope nope nope").await.unwrap().status(), 401);
    }
    assert_eq!(attempt("nope nope nope").await.unwrap().status(), 429);
    // Even the right password is refused while throttled.
    assert_eq!(attempt(PASSWORD).await.unwrap().status(), 429);
    s.stop().await;
}

#[tokio::test]
async fn ac_005_04_throttle_window_is_injectable() {
    let s = server_with(|c| c.login_window = std::time::Duration::from_millis(300)).await;
    s.set_password().await;
    for _ in 0..5 {
        s.http
            .post(s.url("/login"))
            .header("Origin", &s.base)
            .form(&[("password", "nope nope nope")])
            .send()
            .await
            .unwrap();
    }
    tokio::time::sleep(std::time::Duration::from_millis(350)).await;
    let r = s
        .http
        .post(s.url("/login"))
        .header("Origin", &s.base)
        .form(&[("password", PASSWORD)])
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 303);
    s.stop().await;
}

#[tokio::test]
async fn ac_005_05_api_key_until_revoked_and_not_in_query() {
    let s = server().await;
    let key = s.api_key().await;
    let (st, _) = s.api_get(&key, "/api/v1/devices").await;
    assert_eq!(st, 200);
    let used = s.app().store.read(auth::list_keys).await.unwrap();
    assert!(used[0].last_used_at.is_some());
    let r = s
        .http
        .get(s.url(&format!("/api/v1/devices?api_key={key}")))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 400);
    let v: Value = r.json().await.unwrap();
    assert_eq!(v["error"]["code"], "key_in_query");
    let pid = used[0].public_id.clone();
    let now = s.app().clock.now_ms();
    s.app()
        .store
        .write(move |c| auth::revoke_key(c, &pid, now))
        .await
        .unwrap();
    let (st, _) = s.api_get(&key, "/api/v1/devices").await;
    assert_eq!(st, 401);
    let (st, _) = s.api_get("rbk_unknown", "/api/v1/devices").await;
    assert_eq!(st, 401);
    s.stop().await;
}

#[tokio::test]
async fn ac_005_06_password_change_drops_other_sessions() {
    let s = server().await;
    let a = s.login().await;
    let b = s.login().await;
    let r = s
        .http
        .post(s.url("/settings/password"))
        .header("Origin", &s.base)
        .header("Cookie", &a)
        .form(&[
            ("current", PASSWORD),
            ("new", "another password 2"),
            ("repeat", "another password 2"),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 303);
    assert_eq!(r.headers()["location"], "/settings?ok=password");
    let get = |c: &str| {
        s.http
            .get(s.url("/settings"))
            .header("Cookie", c.to_string())
            .send()
    };
    assert_eq!(get(&a).await.unwrap().status(), 200);
    assert_eq!(get(&b).await.unwrap().status(), 303);
    // A wrong current password changes nothing.
    let r = s
        .http
        .post(s.url("/settings/password"))
        .header("Origin", &s.base)
        .header("Cookie", &a)
        .form(&[
            ("current", "not it at all"),
            ("new", "third password 3"),
            ("repeat", "third password 3"),
        ])
        .send()
        .await
        .unwrap();
    assert!(r.headers()["location"].to_str().unwrap().contains("err="));
    let hash = s
        .app()
        .store
        .read(auth::password_hash)
        .await
        .unwrap()
        .unwrap();
    assert!(auth::verify_password(&hash, "another password 2"));
    s.stop().await;
}

#[tokio::test]
async fn env_password_applies_only_when_unset() {
    let s = server_with(|c| c.admin_password = Some("from the environment".into())).await;
    let hash = s
        .app()
        .store
        .read(auth::password_hash)
        .await
        .unwrap()
        .unwrap();
    assert!(auth::verify_password(&hash, "from the environment"));
    let s = s
        .restart(|c| c.admin_password = Some("a different one!".into()))
        .await;
    let hash = s
        .app()
        .store
        .read(auth::password_hash)
        .await
        .unwrap()
        .unwrap();
    assert!(auth::verify_password(&hash, "from the environment"));
    s.stop().await;
}

// ---------- Turnstile and client IP ----------

/// Fake Siteverify: accepts the token "good" and records each `remoteip`.
async fn siteverify() -> (std::sync::Arc<std::sync::Mutex<Vec<String>>>, String) {
    use axum::{Form, Json, Router, extract::State, routing::post};
    use std::collections::HashMap;
    type Seen = std::sync::Arc<std::sync::Mutex<Vec<String>>>;
    async fn handle(
        State(seen): State<Seen>,
        Form(f): Form<HashMap<String, String>>,
    ) -> Json<Value> {
        seen.lock()
            .unwrap()
            .push(f.get("remoteip").cloned().unwrap_or_default());
        let ok = f.get("secret").map(String::as_str) == Some("sekret")
            && f.get("response").map(String::as_str) == Some("good");
        Json(serde_json::json!({ "success": ok }))
    }
    let seen = Seen::default();
    let app = Router::new()
        .route("/", post(handle))
        .with_state(seen.clone());
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    (seen, url)
}

async fn turnstile_server(verify_url: &str) -> Server {
    let url = verify_url.to_string();
    let s = server_with(move |c| {
        c.trust_proxy = true;
        c.turnstile = Some(rustybee::config::Turnstile {
            site_key: "site-key-1".into(),
            secret: "sekret".into(),
        });
        c.turnstile_verify_url = url;
    })
    .await;
    s.set_password().await;
    s
}

async fn login_via(s: &Server, cf_ip: Option<&str>, token: Option<&str>) -> u16 {
    let mut form = vec![("password", PASSWORD)];
    if let Some(t) = token {
        form.push(("cf-turnstile-response", t));
    }
    let mut r = s
        .http
        .post(s.url("/login"))
        .header("Origin", &s.base)
        .form(&form);
    if let Some(ip) = cf_ip {
        r = r.header("CF-Connecting-IP", ip);
    }
    r.send().await.unwrap().status().as_u16()
}

#[tokio::test]
async fn ac_005_13_cloudflare_login_needs_turnstile() {
    let (seen, url) = siteverify().await;
    let s = turnstile_server(&url).await;
    assert_eq!(login_via(&s, Some("203.0.113.7"), None).await, 403);
    assert_eq!(login_via(&s, Some("203.0.113.7"), Some("bad")).await, 403);
    assert_eq!(login_via(&s, Some("203.0.113.7"), Some("good")).await, 303);
    assert_eq!(
        seen.lock().unwrap().as_slice(),
        ["203.0.113.7", "203.0.113.7"]
    );
    // Code sends are gated before anything else (409 would mean no phone).
    let r = s
        .http
        .post(s.url("/login/code"))
        .header("Origin", &s.base)
        .header("CF-Connecting-IP", "203.0.113.7")
        .form(&[("x", "")])
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);
    s.stop().await;
}

#[tokio::test]
async fn ac_005_13_siteverify_unreachable_fails_closed() {
    let s = turnstile_server("http://127.0.0.1:1/").await;
    assert_eq!(login_via(&s, Some("203.0.113.7"), Some("good")).await, 403);
    s.stop().await;
}

#[tokio::test]
async fn ac_005_14_lan_login_skips_turnstile() {
    let (seen, url) = siteverify().await;
    let s = turnstile_server(&url).await;
    let page = |ip: Option<&'static str>| {
        let mut r = s.http.get(s.url("/login"));
        if let Some(ip) = ip {
            r = r.header("CF-Connecting-IP", ip);
        }
        r.send()
    };
    let lan = page(None).await.unwrap().text().await.unwrap();
    assert!(!lan.contains("cf-turnstile") && !lan.contains("challenges.cloudflare.com"));
    let cf = page(Some("203.0.113.7"))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(cf.contains(r#"data-sitekey="site-key-1""#) && cf.contains("data-ts"));
    let r = s
        .http
        .post(s.url("/login/code"))
        .header("Origin", &s.base)
        .form(&[("x", "")])
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 409);
    assert_eq!(login_via(&s, None, None).await, 303);
    assert!(seen.lock().unwrap().is_empty());
    s.stop().await;
}

#[tokio::test]
async fn ac_005_15_spoofed_forwarded_for_does_not_dodge_throttle() {
    let s = server_with(|c| c.trust_proxy = true).await;
    s.set_password().await;
    for i in 0..5 {
        let r = s
            .http
            .post(s.url("/login"))
            .header("Origin", &s.base)
            .header("X-Forwarded-For", format!("10.0.0.{i}, 198.51.100.9"))
            .form(&[("password", "nope nope nope")])
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 401);
    }
    let r = s
        .http
        .post(s.url("/login"))
        .header("Origin", &s.base)
        .header("X-Forwarded-For", "10.0.0.99, 198.51.100.9")
        .form(&[("password", PASSWORD)])
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 429);
    // CF-Connecting-IP wins over X-Forwarded-For: a different client is not throttled.
    let r = s
        .http
        .post(s.url("/login"))
        .header("Origin", &s.base)
        .header("X-Forwarded-For", "198.51.100.9")
        .header("CF-Connecting-IP", "203.0.113.50")
        .form(&[("password", PASSWORD)])
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 303);
    s.stop().await;
}

#[tokio::test]
async fn ac_005_21_proxy_headers_ignored_from_public_peer() {
    use axum::http::{HeaderMap, HeaderValue};
    let s = server_with(|c| c.trust_proxy = true).await;
    let mut h = HeaderMap::new();
    h.insert("x-forwarded-for", HeaderValue::from_static("10.9.9.9"));
    h.insert("cf-connecting-ip", HeaderValue::from_static("203.0.113.50"));
    let public: std::net::SocketAddr = "198.51.100.7:40000".parse().unwrap();
    let ip = rustybee::http::client_ip(s.app(), &h, Some(public));
    assert_eq!(ip, public.ip());
    assert!(!rustybee::http::via_cloudflare(s.app(), &h, Some(public)));
    for local in [
        "127.0.0.1:1",
        "10.0.0.2:1",
        "[fd00::2]:1",
        "[::ffff:192.168.1.2]:1",
    ] {
        let peer = Some(local.parse().unwrap());
        assert_eq!(
            rustybee::http::client_ip(s.app(), &h, peer).to_string(),
            "203.0.113.50"
        );
        assert!(rustybee::http::via_cloudflare(s.app(), &h, peer));
    }
    h.remove("cf-connecting-ip");
    let peer = Some("172.20.0.3:1".parse().unwrap());
    assert_eq!(
        rustybee::http::client_ip(s.app(), &h, peer).to_string(),
        "10.9.9.9"
    );
    s.stop().await;
}
