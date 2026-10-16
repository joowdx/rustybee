//! Login codes texted to the admin phone; password only as a fallback (spec 005).

mod common;

use common::*;
use rustybee::otp;
use rustybee::store::messages;
use serde_json::Value;

const PHONE: &str = "+15550001111";

async fn set_phone(s: &Server) {
    s.app()
        .store
        .write(|c| otp::set_admin_phone(c, Some(PHONE)))
        .await
        .unwrap();
}

async fn post(s: &Server, path: &str, form: &[(&str, &str)]) -> reqwest::Response {
    s.http
        .post(s.url(path))
        .header("Origin", &s.base)
        .form(form)
        .send()
        .await
        .unwrap()
}

/// Ask for a code and read it off the device's `send` frame.
async fn request_code(s: &Server, d: &mut Device) -> (String, String) {
    let r = post(s, "/login/code", &[]).await;
    assert_eq!(r.status(), 200);
    let f = d.recv_type("send").await;
    d.ack(&f["id"]).await;
    assert_eq!(f["data"]["to"], PHONE);
    let body = f["data"]["body"].as_str().unwrap();
    let code: String = body.chars().filter(char::is_ascii_digit).take(6).collect();
    assert_eq!(code.len(), 6, "{body}");
    (f["data"]["msg_id"].as_str().unwrap().to_string(), code)
}

async fn failures(s: &Server) -> u32 {
    s.app().store.read(otp::send_failures).await.unwrap()
}

#[tokio::test]
async fn code_login_and_password_locked_while_device_online() {
    let s = server().await;
    s.set_password().await;
    set_phone(&s).await;
    let key = s.api_key().await;
    let (_, token) = s.pair("A").await;
    let mut d = Device::connect(&s, &token).await;

    // A device can text a code, so the password is refused.
    let r = post(&s, "/login", &[("password", PASSWORD)]).await;
    assert_eq!(r.status(), 403);
    let page = s.http.get(s.url("/login")).send().await.unwrap().text().await.unwrap();
    assert!(page.contains("Text me a code") && !page.contains("name=\"password\""));

    let (msg_id, code) = request_code(&s, &mut d).await;
    // Hidden from the public API while it carries the code.
    let (st, _) = s.api_get(&key, &format!("/api/v1/messages/{msg_id}")).await;
    assert_eq!(st, 404);
    let (_, list) = s.api_get(&key, "/api/v1/messages").await;
    assert!(list["items"].as_array().unwrap().is_empty(), "{list}");

    d.status(&msg_id, "sent").await;
    let r = post(&s, "/login/verify", &[("code", &code)]).await;
    assert_eq!(r.status(), 303);
    assert!(r.headers()["set-cookie"].to_str().unwrap().starts_with("rb_session="));
    // Single use.
    let r = post(&s, "/login/verify", &[("code", &code)]).await;
    assert_eq!(r.status(), 401);

    // Once sent, the body is redacted.
    otp::resolve(s.app()).await.unwrap();
    let m = s
        .app()
        .store
        .read(move |c| messages::by_public(c, &msg_id))
        .await
        .unwrap()
        .unwrap();
    assert!(m.internal && !m.body.contains(&code), "{}", m.body);
    s.stop().await;
}

#[tokio::test]
async fn password_opens_after_n_failed_sends() {
    let s = server_with(|c| c.otp_max_failures = 2).await;
    s.set_password().await;
    set_phone(&s).await;
    let (_, token) = s.pair("A").await;
    s.app()
        .store
        .write(|c| c.execute("UPDATE devices SET send_delay_ms=0", []))
        .await
        .unwrap();
    let mut d = Device::connect(&s, &token).await;
    for n in 1..=2 {
        let (msg_id, _) = request_code(&s, &mut d).await;
        d.status(&msg_id, "failed").await;
        otp::resolve(s.app()).await.unwrap();
        assert_eq!(failures(&s).await, n);
        s.app().clock.advance_ms(31_000);
    }
    let r = post(&s, "/login", &[("password", PASSWORD)]).await;
    assert_eq!(r.status(), 303);
    // A successful login resets the counter.
    assert_eq!(failures(&s).await, 0);
    s.stop().await;
}

#[tokio::test]
async fn unsent_code_counts_as_failure_after_timeout() {
    let s = server_with(|c| c.otp_max_failures = 1).await;
    s.set_password().await;
    set_phone(&s).await;
    let (_, token) = s.pair("A").await;
    let mut d = Device::connect(&s, &token).await;
    request_code(&s, &mut d).await;
    otp::resolve(s.app()).await.unwrap();
    assert_eq!(failures(&s).await, 0);
    s.app().clock.advance_ms(91_000);
    otp::resolve(s.app()).await.unwrap();
    otp::resolve(s.app()).await.unwrap();
    assert_eq!(failures(&s).await, 1, "counted once");
    let r = post(&s, "/login", &[("password", PASSWORD)]).await;
    assert_eq!(r.status(), 303);
    s.stop().await;
}

#[tokio::test]
async fn password_allowed_when_no_device_online_or_no_phone() {
    let s = server().await;
    s.set_password().await;
    // No phone yet: bootstrap.
    let r = post(&s, "/login", &[("password", PASSWORD)]).await;
    assert_eq!(r.status(), 303);
    // Phone set, device paired but offline.
    set_phone(&s).await;
    s.pair("A").await;
    let r = post(&s, "/login", &[("password", PASSWORD)]).await;
    assert_eq!(r.status(), 303);
    let r = post(&s, "/login/code", &[]).await;
    assert_eq!(r.status(), 409);
    s.stop().await;
}

#[tokio::test]
async fn code_dies_after_five_wrong_tries_and_sends_are_spaced() {
    let s = server().await;
    set_phone(&s).await;
    let (_, token) = s.pair("A").await;
    let mut d = Device::connect(&s, &token).await;
    let (_, code) = request_code(&s, &mut d).await;
    // A second send right away is refused.
    let r = post(&s, "/login/code", &[]).await;
    assert_eq!(r.status(), 429);
    let wrong = if code == "000000" { "111111" } else { "000000" };
    for _ in 0..otp::MAX_ATTEMPTS {
        assert_eq!(
            otp::verify(s.app(), otp::Purpose::Login, wrong),
            otp::Verify::Wrong
        );
    }
    assert_eq!(
        otp::verify(s.app(), otp::Purpose::Login, &code),
        otp::Verify::Expired
    );
    s.stop().await;
}

#[tokio::test]
async fn settings_confirms_phone_by_code() {
    let s = server().await;
    let cookie = s.login().await;
    let (_, token) = s.pair("A").await;
    let mut d = Device::connect(&s, &token).await;
    let r = s
        .http
        .post(s.url("/settings/phone"))
        .header("Origin", &s.base)
        .header("Cookie", &cookie)
        .form(&[("phone", PHONE)])
        .send()
        .await
        .unwrap();
    assert_eq!(r.headers()["location"], "/settings?ok=phone_code");
    let f: Value = d.recv_type("send").await;
    let code: String = f["data"]["body"]
        .as_str()
        .unwrap()
        .chars()
        .filter(char::is_ascii_digit)
        .take(6)
        .collect();
    assert_eq!(s.app().store.read(otp::admin_phone).await.unwrap(), None);
    let r = s
        .http
        .post(s.url("/settings/phone/verify"))
        .header("Origin", &s.base)
        .header("Cookie", &cookie)
        .form(&[("code", code.as_str())])
        .send()
        .await
        .unwrap();
    assert_eq!(r.headers()["location"], "/settings?ok=phone");
    assert_eq!(
        s.app().store.read(otp::admin_phone).await.unwrap().as_deref(),
        Some(PHONE)
    );
    s.stop().await;
}
