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
