//! Login codes texted to the admin phone; password only as a fallback (spec 005).

mod common;

use common::*;
use rustybee::otp;
use rustybee::store::{devices, messages};
use serde_json::Value;

const PHONE: &str = "+15550001111";
const OTHER: &str = "+15550002222";

async fn set_phone(s: &Server) {
    s.app()
        .store
        .write(|c| otp::add_admin_phone(c, PHONE))
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
    let r = post(s, "/login/code", &[("last4", "1111")]).await;
    assert_eq!(r.status(), 200);
    let f = d.recv_type("send").await;
    d.ack(&f["id"]).await;
    assert_eq!(f["data"]["to"], PHONE);
    let body = f["data"]["body"].as_str().unwrap();
    let code: String = body.chars().filter(char::is_ascii_digit).take(6).collect();
    assert_eq!(code.len(), 6, "{body}");
    (f["data"]["msg_id"].as_str().unwrap().to_string(), code)
}

fn ip(n: u8) -> std::net::IpAddr {
    std::net::IpAddr::from([198, 51, 100, n])
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
    let page = s
        .http
        .get(s.url("/login"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(page.contains("Text me a code") && !page.contains("name=\"password\""));
    assert!(page.contains("name=\"last4\""));

    let (msg_id, code) = request_code(&s, &mut d).await;
    // Hidden from the public API while it carries the code.
    let (st, _) = s.api_get(&key, &format!("/api/v1/messages/{msg_id}")).await;
    assert_eq!(st, 404);
    let (_, list) = s.api_get(&key, "/api/v1/messages").await;
    assert!(list["items"].as_array().unwrap().is_empty(), "{list}");

    d.status(&msg_id, "sent").await;
    let r = post(&s, "/login/verify", &[("code", &code)]).await;
    assert_eq!(r.status(), 303);
    assert!(
        r.headers()["set-cookie"]
            .to_str()
            .unwrap()
            .starts_with("rb_session=")
    );
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
    let r = post(&s, "/login/code", &[("last4", "1111")]).await;
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
    let r = post(&s, "/login/code", &[("last4", "1111")]).await;
    assert_eq!(r.status(), 429);
    let wrong = if code == "000000" { "111111" } else { "000000" };
    for _ in 0..otp::MAX_ATTEMPTS {
        assert_eq!(
            otp::verify(s.app(), otp::Purpose::Login, wrong, ip(1)),
            otp::Verify::Wrong
        );
    }
    assert_eq!(
        otp::verify(s.app(), otp::Purpose::Login, &code, ip(1)),
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
    assert!(
        s.app()
            .store
            .read(otp::admin_phones)
            .await
            .unwrap()
            .is_empty()
    );
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
    assert_eq!(phones(&s).await, [PHONE]);
    s.stop().await;
}

async fn phones(s: &Server) -> Vec<String> {
    s.app().store.read(otp::admin_phones).await.unwrap()
}

async fn settings_post(s: &Server, cookie: &str, path: &str, form: &[(&str, &str)]) -> String {
    let r = s
        .http
        .post(s.url(path))
        .header("Origin", &s.base)
        .header("Cookie", cookie)
        .form(form)
        .send()
        .await
        .unwrap();
    r.headers()["location"].to_str().unwrap().to_string()
}

#[tokio::test]
async fn ac_005_12_second_phone_adds_and_remove_keeps_others() {
    let s = server().await;
    let cookie = s.login().await;
    set_phone(&s).await;
    let (_, token) = s.pair("A").await;
    let mut d = Device::connect(&s, &token).await;

    // A duplicate is refused before anything is texted.
    let loc = settings_post(&s, &cookie, "/settings/phone", &[("phone", PHONE)]).await;
    assert!(loc.starts_with("/settings?err="), "{loc}");

    let loc = settings_post(&s, &cookie, "/settings/phone", &[("phone", OTHER)]).await;
    assert_eq!(loc, "/settings?ok=phone_code");
    let f: Value = d.recv_type("send").await;
    assert_eq!(f["data"]["to"], OTHER);
    let code: String = f["data"]["body"]
        .as_str()
        .unwrap()
        .chars()
        .filter(char::is_ascii_digit)
        .take(6)
        .collect();
    let loc = settings_post(&s, &cookie, "/settings/phone/verify", &[("code", &code)]).await;
    assert_eq!(loc, "/settings?ok=phone");
    assert_eq!(phones(&s).await, [PHONE, OTHER]);

    let page = s
        .http
        .get(s.url("/settings"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(page.contains(PHONE) && page.contains(OTHER));

    let loc = settings_post(&s, &cookie, "/settings/phone/remove", &[("phone", PHONE)]).await;
    assert_eq!(loc, "/settings?ok=phone_removed");
    assert_eq!(phones(&s).await, [OTHER]);
    s.stop().await;
}

async fn message_count(s: &Server) -> i64 {
    s.app()
        .store
        .read(|c| c.query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0)))
        .await
        .unwrap()
}

#[tokio::test]
async fn ac_005_16_unknown_last4_sends_nothing_and_is_throttled() {
    let s = server().await;
    set_phone(&s).await;
    s.app()
        .store
        .write(|c| otp::add_admin_phone(c, OTHER))
        .await
        .unwrap();
    let (_, token) = s.pair("A").await;
    let _d = Device::connect(&s, &token).await;

    let r = post(&s, "/login/code", &[("last4", "9999")]).await;
    assert_eq!(r.status(), 404);
    assert!(r.text().await.unwrap().contains("that number isn"));
    for _ in 0..4 {
        assert_eq!(
            post(&s, "/login/code", &[("last4", "9999")]).await.status(),
            404
        );
    }
    assert_eq!(
        post(&s, "/login/code", &[("last4", "2222")]).await.status(),
        429
    );
    assert_eq!(
        post(&s, "/login/code", &[("last4", "12")]).await.status(),
        429
    );

    assert_eq!(message_count(&s).await, 0);

    // A matching number gets the code, and only that number.
    let s2 = server().await;
    set_phone(&s2).await;
    s2.app()
        .store
        .write(|c| otp::add_admin_phone(c, OTHER))
        .await
        .unwrap();
    let (_, token2) = s2.pair("A").await;
    let mut d2 = Device::connect(&s2, &token2).await;
    let r = post(&s2, "/login/code", &[("last4", "2222")]).await;
    assert_eq!(r.status(), 200);
    assert!(r.text().await.unwrap().contains("•••• 2222"));
    let f = d2.recv_type("send").await;
    assert_eq!(f["data"]["to"], OTHER);
    assert_eq!(message_count(&s2).await, 1);
    s.stop().await;
    s2.stop().await;
}

#[tokio::test]
async fn ac_005_17_login_page_shows_no_phone_digits() {
    let s = server().await;
    set_phone(&s).await;
    let (_, token) = s.pair("A").await;
    let _d = Device::connect(&s, &token).await;
    let page = s
        .http
        .get(s.url("/login"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(page.contains("Text me a code"));
    assert!(!page.contains("1111") && !page.contains("•••• "), "{page}");
    s.stop().await;
}

#[tokio::test]
async fn ac_005_18_wrong_codes_from_one_ip_keep_anothers_code() {
    let s = server().await;
    set_phone(&s).await;
    let (_, token) = s.pair("A").await;
    let mut d = Device::connect(&s, &token).await;
    let (_, code) = request_code(&s, &mut d).await;
    let wrong = if code == "000000" { "111111" } else { "000000" };
    for _ in 0..otp::MAX_ATTEMPTS {
        assert_eq!(
            otp::verify(s.app(), otp::Purpose::Login, wrong, ip(1)),
            otp::Verify::Wrong
        );
    }
    // The guessing IP is locked out, even with the right code.
    assert_eq!(
        otp::verify(s.app(), otp::Purpose::Login, &code, ip(1)),
        otp::Verify::Expired
    );
    assert!(otp::pending(s.app(), otp::Purpose::Login));
    assert_eq!(
        otp::verify(s.app(), otp::Purpose::Login, &code, ip(2)),
        otp::Verify::Ok(PHONE.to_string())
    );
    s.stop().await;
}

#[tokio::test]
async fn ac_005_18_code_dropped_after_global_ceiling() {
    let s = server().await;
    set_phone(&s).await;
    let (_, token) = s.pair("A").await;
    let mut d = Device::connect(&s, &token).await;
    let (_, code) = request_code(&s, &mut d).await;
    let wrong = if code == "000000" { "111111" } else { "000000" };
    for n in 0..otp::MAX_TOTAL_ATTEMPTS {
        let who = ip((n / otp::MAX_ATTEMPTS) as u8 + 10);
        assert_eq!(
            otp::verify(s.app(), otp::Purpose::Login, wrong, who),
            otp::Verify::Wrong
        );
    }
    assert!(!otp::pending(s.app(), otp::Purpose::Login));
    assert_eq!(
        otp::verify(s.app(), otp::Purpose::Login, &code, ip(99)),
        otp::Verify::Expired
    );
    s.stop().await;
}

#[tokio::test]
async fn ac_005_19_failed_verify_does_not_name_the_number() {
    let s = server().await;
    set_phone(&s).await;
    let (_, token) = s.pair("A").await;
    let mut d = Device::connect(&s, &token).await;
    let r = post(&s, "/login/code", &[("last4", "1111")]).await;
    assert_eq!(r.status(), 200);
    assert!(r.text().await.unwrap().contains("•••• 1111"));
    let f = d.recv_type("send").await;
    d.ack(&f["id"]).await;
    let body = f["data"]["body"].as_str().unwrap();
    let code: String = body.chars().filter(char::is_ascii_digit).take(6).collect();
    let wrong = if code == "000000" { "111111" } else { "000000" };
    let r = post(&s, "/login/verify", &[("code", wrong)]).await;
    assert_eq!(r.status(), 401);
    let page = r.text().await.unwrap();
    assert!(page.contains("name=\"code\""), "{page}");
    assert!(!page.contains("1111"), "{page}");
    // Throttled responses don't name it either.
    for _ in 0..5 {
        post(&s, "/login/verify", &[("code", wrong)]).await;
    }
    let r = post(&s, "/login/verify", &[("code", wrong)]).await;
    assert_eq!(r.status(), 429);
    assert!(!r.text().await.unwrap().contains("1111"));
    s.stop().await;
}

#[tokio::test]
async fn ac_005_20_orphaned_code_message_is_redacted() {
    let s = server().await;
    s.pair("A").await;
    let device = s.app().store.read(devices::list).await.unwrap()[0].id;
    let old = s.app().clock.now_ms() - otp::CODE_TTL_MS - 1;
    let fresh = s.app().clock.now_ms();
    let ids = s
        .app()
        .store
        .write(move |c| {
            let mut ids = vec![];
            for (at, internal) in [(old, true), (fresh, true), (old, false)] {
                let item = messages::NewMessage {
                    to: PHONE.into(),
                    body: "rustybee login code: 123456. Expires in 5 minutes.".into(),
                };
                let (_, rows) = messages::create_batch(c, device, &[item], None, at)?;
                if internal {
                    messages::mark_internal(c, rows[0].id)?;
                }
                ids.push(rows[0].public_id.clone());
            }
            Ok(ids)
        })
        .await
        .unwrap();
    otp::resolve(s.app()).await.unwrap();
    let mut bodies = vec![];
    for id in ids {
        let m = s
            .app()
            .store
            .read(move |c| messages::by_public(c, &id))
            .await
            .unwrap()
            .unwrap();
        bodies.push(m.body);
    }
    assert_eq!(bodies[0], "Login code (hidden)");
    assert!(
        bodies[1].contains("123456"),
        "fresh code kept: {}",
        bodies[1]
    );
    assert!(
        bodies[2].contains("123456"),
        "user message kept: {}",
        bodies[2]
    );
    s.stop().await;
}
