//! Web UI (spec 010): HTTP-level checks on status, redirects and markup.

mod common;

use common::*;
use rustybee::assets::Assets;
use std::io::Write;

struct Ui<'a> {
    s: &'a Server,
    cookie: String,
}

impl<'a> Ui<'a> {
    async fn new(s: &'a Server) -> Ui<'a> {
        let cookie = s.login().await;
        Ui { s, cookie }
    }

    async fn get(&self, path: &str) -> (u16, String) {
        let r = self
            .s
            .http
            .get(self.s.url(path))
            .header("Cookie", &self.cookie)
            .send()
            .await
            .unwrap();
        let st = r.status().as_u16();
        (st, r.text().await.unwrap())
    }

    async fn post(&self, path: &str, form: &[(&str, &str)]) -> reqwest::Response {
        self.s
            .http
            .post(self.s.url(path))
            .header("Cookie", &self.cookie)
            .header("Origin", &self.s.base)
            .form(form)
            .send()
            .await
            .unwrap()
    }
}

fn between<'h>(html: &'h str, start: &str, end: &str) -> &'h str {
    let i = html
        .find(start)
        .unwrap_or_else(|| panic!("{start} not in page"))
        + start.len();
    let j = html[i..].find(end).unwrap() + i;
    &html[i..j]
}

#[tokio::test]
async fn ac_010_01_routes_need_session() {
    let s = server().await;
    let key = s.api_key().await;
    s.pair("Phone").await;
    let (_, v) = s
        .api_post(
            &key,
            "/api/v1/messages",
            serde_json::json!({"to": ["+15550000001"], "body": "hi"}),
        )
        .await;
    let msg = v["messages"][0]["id"].as_str().unwrap().to_string();
    let now = s.app().clock.now_ms();
    let hook = s
        .app()
        .store
        .write(move |c| {
            rustybee::store::webhooks::create(
                c,
                "h",
                "https://x.test/h",
                &["message.sent".into()],
                true,
                now,
            )
        })
        .await
        .unwrap();
    let routes = [
        "/".to_string(),
        "/send".into(),
        "/bulk".into(),
        "/messages".into(),
        format!("/messages/{msg}"),
        "/devices".into(),
        "/keys".into(),
        "/webhooks".into(),
        format!("/webhooks/{}", hook.public_id),
        "/settings".into(),
        "/partials/dashboard".into(),
        "/messages/rows".into(),
    ];
    let ui = Ui::new(&s).await;
    // Without a session: the login page renders, everything else redirects.
    let r = s.http.get(s.url("/login")).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert!(r.text().await.unwrap().contains("name=\"password\""));
    for path in routes.iter().chain(std::iter::once(&"/events".to_string())) {
        let r = s.http.get(s.url(path)).send().await.unwrap();
        assert_eq!(r.status(), 303, "{path}");
        assert_eq!(r.headers()["location"], "/login", "{path}");
    }
    for path in &routes {
        let (st, _) = ui.get(path).await;
        assert_eq!(st, 200, "{path}");
    }
    let (_, home) = ui.get("/").await;
    assert!(home.contains("sse-connect=\"/events\""));
    assert!(home.contains("Phone"));
    let (_, detail) = ui.get(&format!("/messages/{msg}")).await;
    assert!(detail.contains("+15550000001") && detail.contains("Reply"));
    // SSE stream.
    let r = s
        .http
        .get(s.url("/events"))
        .header("Cookie", &ui.cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert!(
        r.headers()["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/event-stream")
    );
    drop(r);
    // Static assets: hashed names are immutable.
    let css = between(&home, "<link rel=\"stylesheet\" href=\"", "\"").to_string();
    assert!(css.starts_with("/static/app.") && css.ends_with(".css"));
    let r = s.http.get(s.url(&css)).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(
        r.headers()["cache-control"],
        "public, max-age=31536000, immutable"
    );
    let r = s.http.get(s.url("/static/nope.js")).send().await.unwrap();
    assert_eq!(r.status(), 404);
    // The login page with no password set shows the hint.
    let empty = server().await;
    let r = empty.http.get(empty.url("/login")).send().await.unwrap();
    assert!(r.text().await.unwrap().contains("set-password"));
    empty.stop().await;
    s.stop().await;
}

#[tokio::test]
async fn ac_010_02_send_form_creates_visible_messages() {
    let s = server().await;
    s.pair("Phone").await;
    let ui = Ui::new(&s).await;
    let r = ui
        .post(
            "/send",
            &[
                ("to", "+15550000001, +1 555 000 0002"),
                ("body", "From the UI"),
                ("device_id", ""),
                ("sim", ""),
            ],
        )
        .await;
    assert_eq!(r.status(), 200);
    let html = r.text().await.unwrap();
    assert!(html.contains("Queued 2 messages"), "{html}");
    let (_, list) = ui.get("/messages").await;
    assert_eq!(list.matches("From the UI").count(), 2);
    assert!(list.contains("+15550000002"));
    let (_, filtered) = ui.get("/messages?peer=%2B15550000002").await;
    assert_eq!(filtered.matches("From the UI").count(), 1);
    // A bad recipient re-renders the form with the error and keeps the input.
    let r = ui
        .post("/send", &[("to", "abc"), ("body", "keep me")])
        .await;
    assert_eq!(r.status(), 422);
    let html = r.text().await.unwrap();
    assert!(html.contains("class=\"error\"") && html.contains("keep me"));
    s.stop().await;
}

#[tokio::test]
async fn ac_010_03_bulk_csv_flow() {
    let s = server().await;
    let key = s.api_key().await;
    s.pair("Phone").await;
    let ui = Ui::new(&s).await;
    let csv = "name,phone\nAnn,+15550000001\nBob,+15550000002\nCyd,+15550000003\n";
    let boundary = "XyZboundary123";
    let body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"c.csv\"\r\nContent-Type: text/csv\r\n\r\n{csv}\r\n--{boundary}--\r\n"
    );
    let r = s
        .http
        .post(s.url("/bulk/upload"))
        .header("Cookie", &ui.cookie)
        .header("Origin", &s.base)
        .header(
            "Content-Type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(body)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let html = r.text().await.unwrap();
    assert!(html.contains("3 rows"));
    let token = between(&html, "name=\"token\" value=\"", "\"").to_string();
    // The phone column is detected.
    assert!(html.contains("<option value=\"phone\" selected>"));
    let form = [
        ("token", token.as_str()),
        ("phone", "phone"),
        ("template", "Hi {{name}}"),
        ("device_id", ""),
        ("sim", ""),
    ];
    let preview = ui.post("/bulk/preview", &form).await.text().await.unwrap();
    assert!(preview.contains("Hi Ann") && preview.contains("Hi Cyd"));
    let r = ui.post("/bulk/send", &form).await;
    assert_eq!(r.status(), 200);
    let html = r.text().await.unwrap();
    assert!(html.contains("Queued 3 messages"), "{html}");
    let (_, v) = s.api_get(&key, "/api/v1/messages").await;
    let mut bodies: Vec<String> = v["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| {
            format!(
                "{} {}",
                m["peer"].as_str().unwrap(),
                m["body"].as_str().unwrap()
            )
        })
        .collect();
    bodies.sort();
    assert_eq!(
        bodies,
        vec![
            "+15550000001 Hi Ann",
            "+15550000002 Hi Bob",
            "+15550000003 Hi Cyd"
        ]
    );
    // The token is single-use.
    let r = ui.post("/bulk/send", &form).await;
    assert_eq!(r.status(), 422);
    s.stop().await;
}

#[tokio::test]
async fn ac_010_04_pairing_link_is_accepted() {
    let s = server().await;
    let ui = Ui::new(&s).await;
    let r = ui.post("/devices/pair", &[]).await;
    assert_eq!(r.status(), 200);
    let html = r.text().await.unwrap();
    assert!(html.contains("<svg"), "QR code inline");
    let link = between(&html, "<code id=\"pair-link\">", "</code>")
        .replace("&amp;", "&")
        .replace("&#38;", "&");
    assert!(
        link.starts_with("rustybee://pair?u=http%3A%2F%2F127.0.0.1&c="),
        "{link}"
    );
    let code = link.rsplit("&c=").next().unwrap();
    assert_eq!(code.len(), 20);
    let r = s
        .http
        .post(s.url("/api/device/pair"))
        .json(&serde_json::json!({"code": code, "name": "Scanned"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    // The polling endpoint notices the new device.
    let r = s
        .http
        .get(s.url("/devices/paired?n=0"))
        .header("Cookie", &ui.cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(r.headers()["hx-refresh"], "true");
    let (_, list) = ui.get("/devices").await;
    assert!(list.contains("Scanned"));
    s.stop().await;
}

#[test]
fn ac_010_05_static_assets_under_40kb_gzipped() {
    let mut total = 0;
    for name in Assets::iter() {
        let f = Assets::get(&name).unwrap();
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
        gz.write_all(&f.data).unwrap();
        let n = gz.finish().unwrap().len();
        total += n;
    }
    assert!(total < 40 * 1024, "embedded assets gzip to {total} bytes");
}

#[tokio::test]
async fn device_settings_keys_and_webhooks_forms() {
    let s = server().await;
    let (pid, token) = s.pair("Phone").await;
    let mut d = Device::connect(&s, &token).await;
    let ui = Ui::new(&s).await;
    // Device settings are saved and pushed as `config`.
    let r = ui
        .post(
            &format!("/devices/{pid}"),
            &[
                ("name", "Renamed"),
                ("filters_rev", "0"),
                ("enabled", "on"),
                ("send_delay_ms", "1500"),
                ("sim", "1"),
                ("receive_enabled", "on"),
                ("filter_mode", "block"),
                ("rule_field", "sender"),
                ("rule_op", "equals"),
                ("rule_value", "+15550000009"),
                ("rule_case", "match"),
                ("rule_field", "body"),
                ("rule_op", "contains"),
                ("rule_value", "promo"),
                ("rule_case", "ignore"),
                // The spare blank row is dropped.
                ("rule_field", "sender"),
                ("rule_op", "contains"),
                ("rule_value", " "),
                ("rule_case", "ignore"),
            ],
        )
        .await;
    assert_eq!(r.status(), 303);
    assert_eq!(r.headers()["location"], "/devices?ok=saved");
    let cfg = d.recv_type("config").await;
    assert_eq!(cfg["data"]["name"], "Renamed");
    assert_eq!(cfg["data"]["sim"], 1);
    assert_eq!(cfg["data"]["receive_enabled"], true);
    assert_eq!(cfg["data"]["filters"]["mode"], "block");
    let rules = cfg["data"]["filters"]["rules"].as_array().unwrap();
    assert_eq!(rules.len(), 2);
    assert_eq!(rules[0]["ignore_case"], false);
    assert_eq!(rules[1]["op"], "contains");
    assert_eq!(rules[1]["ignore_case"], true);
    assert_eq!(cfg["data"]["filters_rev"], 1, "filter change bumps rev");
    let (_, page) = ui.get("/devices?ok=saved").await;
    assert!(page.contains("Renamed") && page.contains(r#"name="rule_value" value="promo""#));
    let r = ui
        .post(
            &format!("/devices/{pid}"),
            &[
                ("name", "x"),
                ("rule_field", "subject"),
                ("rule_op", "contains"),
                ("rule_value", "x"),
                ("rule_case", "ignore"),
            ],
        )
        .await;
    assert!(r.headers()["location"].to_str().unwrap().contains("err="));
    // Keys: the secret is shown once.
    let html = ui
        .post("/keys", &[("name", "cli")])
        .await
        .text()
        .await
        .unwrap();
    let secret = between(&html, "<code id=\"new-key\">", "</code>").to_string();
    assert!(secret.starts_with("rbk_"));
    let (st, _) = s.api_get(&secret, "/api/v1/devices").await;
    assert_eq!(st, 200);
    let (_, keys) = ui.get("/keys").await;
    assert!(!keys.contains(&secret) && keys.contains("curl"));
    // Webhooks: create, then edit.
    let r = ui
        .post(
            "/webhooks",
            &[
                ("name", "sink"),
                ("url", "https://example.org/h"),
                ("ev_sent", "on"),
                ("active", "on"),
            ],
        )
        .await;
    assert_eq!(r.status(), 303);
    let loc = r.headers()["location"].to_str().unwrap().to_string();
    let (st, page) = ui.get(&loc).await;
    assert_eq!(st, 200);
    assert!(page.contains("Created.") && page.contains("https://example.org/h"));
    // Removing the device needs the confirm box, then closes the socket with 4003.
    let r = ui.post(&format!("/devices/{pid}/delete"), &[]).await;
    assert!(r.headers()["location"].to_str().unwrap().contains("err="));
    let r = ui
        .post(&format!("/devices/{pid}/delete"), &[("confirm", "on")])
        .await;
    assert_eq!(r.headers()["location"], "/devices?ok=removed");
    assert_eq!(d.close_code(2000).await, Some(4003));
    s.stop().await;
}

#[tokio::test]
async fn ac_002_11_web_turns_forwarding_off_not_on() {
    let s = server().await;
    let (pid, token) = s.pair("Phone").await;
    let mut d = Device::connect(&s, &token).await;
    let ui = Ui::new(&s).await;
    let block = [
        ("filter_mode", "block"),
        ("rule_field", "body"),
        ("rule_op", "contains"),
        ("rule_value", "promo"),
        ("rule_case", "ignore"),
    ];
    // Turning forwarding off bumps the rev; the filter fields sent alongside are ignored.
    let mut form = vec![("name", "Phone")];
    form.extend(block);
    ui.post(&format!("/devices/{pid}"), &form).await;
    let cfg = d.recv_type("config").await;
    assert_eq!(cfg["data"]["receive_enabled"], false);
    assert_eq!(cfg["data"]["filters"]["mode"], "off");
    assert_eq!(cfg["data"]["filters_rev"], 1);
    let (_, page) = ui.get("/devices").await;
    assert!(page.contains(r#"name="receive_enabled" data-forwarding disabled"#));
    assert!(page.contains("data-forwarding-filters disabled"));

    // The web cannot turn it back on, nor edit the filters while it is off.
    let mut form = vec![("name", "Phone"), ("receive_enabled", "on")];
    form.extend(block);
    ui.post(&format!("/devices/{pid}"), &form).await;
    let cfg = d.recv_type("config").await;
    assert_eq!(cfg["data"]["receive_enabled"], false);
    assert_eq!(cfg["data"]["filters"]["mode"], "off");
    assert_eq!(cfg["data"]["filters_rev"], 1);

    // The phone can.
    let f = frame(
        "set_forwarding",
        serde_json::json!({"base_rev": 1, "receive_enabled": true, "filters": {"mode": "off", "rules": []}}),
    );
    d.send(&f).await;
    assert_eq!(d.recv_type("ack").await["data"]["ref"], f["id"]);
    let cfg = d.recv_type("config").await;
    assert_eq!(cfg["data"]["receive_enabled"], true);
    assert_eq!(cfg["data"]["filters_rev"], 2);
    s.stop().await;
}

#[tokio::test]
async fn ac_002_13_web_save_keeps_newer_phone_filters() {
    let s = server().await;
    let (pid, token) = s.pair("Phone").await;
    let mut d = Device::connect(&s, &token).await;
    let ui = Ui::new(&s).await;
    // The web form is loaded at rev 0.
    let (_, page) = ui.get("/devices").await;
    assert!(page.contains(r#"name="filters_rev" value="0""#));
    // Then the phone saves new filters: rev 1.
    let f = frame(
        "set_forwarding",
        serde_json::json!({"base_rev": 0, "receive_enabled": true, "filters": {"mode": "block",
            "rules": [{"field": "body", "op": "contains", "value": "promo", "ignore_case": true}]}}),
    );
    d.send(&f).await;
    assert_eq!(d.recv_type("ack").await["data"]["ref"], f["id"]);
    assert_eq!(d.recv_type("config").await["data"]["filters_rev"], 1);
    // The stale web save keeps the phone's filters but saves everything else.
    let stale = [
        ("name", "Renamed"),
        ("filters_rev", "0"),
        ("enabled", "on"),
        ("send_delay_ms", "1500"),
        ("receive_enabled", "on"),
        ("filter_mode", "allow"),
        ("rule_field", "sender"),
        ("rule_op", "equals"),
        ("rule_value", "+15550000009"),
        ("rule_case", "match"),
    ];
    let r = ui.post(&format!("/devices/{pid}"), &stale).await;
    let loc = r.headers()["location"].to_str().unwrap().to_string();
    assert!(
        loc.contains("err=") && loc.contains("changed%20on%20the%20phone"),
        "{loc}"
    );
    let cfg = d.recv_type("config").await;
    assert_eq!(cfg["data"]["name"], "Renamed");
    assert_eq!(cfg["data"]["receive_enabled"], true);
    assert_eq!(cfg["data"]["filters"]["mode"], "block");
    assert_eq!(cfg["data"]["filters_rev"], 1);
    let (_, page) = ui.get(&loc).await;
    assert!(page.contains("Reload to see them."));
    assert!(page.contains(r#"name="filters_rev" value="1""#));
    // Resubmitting the phone's own filters with a stale rev is not a conflict.
    let same = [
        ("name", "Renamed"),
        ("filters_rev", "0"),
        ("receive_enabled", "on"),
        ("filter_mode", "block"),
        ("rule_field", "body"),
        ("rule_op", "contains"),
        ("rule_value", "promo"),
        ("rule_case", "ignore"),
    ];
    let r = ui.post(&format!("/devices/{pid}"), &same).await;
    assert_eq!(r.headers()["location"], "/devices?ok=saved");
    d.recv_type("config").await;
    // A stale save can still turn forwarding off, and keeps the filters.
    let mut off: Vec<_> = stale.to_vec();
    off.retain(|(k, _)| *k != "receive_enabled");
    let r = ui.post(&format!("/devices/{pid}"), &off).await;
    assert_eq!(r.headers()["location"], "/devices?ok=saved");
    let cfg = d.recv_type("config").await;
    assert_eq!(cfg["data"]["receive_enabled"], false);
    assert_eq!(cfg["data"]["filters"]["mode"], "block");
    assert_eq!(cfg["data"]["filters_rev"], 2);
    // With the current rev the web edits the filters.
    let f = frame(
        "set_forwarding",
        serde_json::json!({"base_rev": 2, "receive_enabled": true, "filters": {"mode": "off", "rules": []}}),
    );
    d.send(&f).await;
    assert_eq!(d.recv_type("ack").await["data"]["ref"], f["id"]);
    assert_eq!(d.recv_type("config").await["data"]["filters_rev"], 3);
    let mut fresh: Vec<_> = stale.to_vec();
    fresh[1] = ("filters_rev", "3");
    let r = ui.post(&format!("/devices/{pid}"), &fresh).await;
    assert_eq!(r.headers()["location"], "/devices?ok=saved");
    let cfg = d.recv_type("config").await;
    assert_eq!(cfg["data"]["filters"]["mode"], "allow");
    assert_eq!(cfg["data"]["filters_rev"], 4);
    s.stop().await;
}

#[tokio::test]
async fn messages_load_more_pages() {
    let s = server().await;
    let key = s.api_key().await;
    s.pair("Phone").await;
    let to: Vec<String> = (0..60).map(|i| format!("+155500{i:05}")).collect();
    s.api_post(
        &key,
        "/api/v1/messages",
        serde_json::json!({"to": to, "body": "bulk"}),
    )
    .await;
    let ui = Ui::new(&s).await;
    let (_, page) = ui.get("/messages").await;
    assert_eq!(page.matches("class=\"li msg\"").count(), 50);
    let more = between(&page, "hx-get=\"/messages/rows?", "\"")
        .replace("&amp;", "&")
        .replace("&#38;", "&");
    let (st, rows) = ui.get(&format!("/messages/rows?{more}")).await;
    assert_eq!(st, 200);
    assert_eq!(rows.matches("class=\"li msg\"").count(), 10);
    assert!(!rows.contains("Load more"));
    s.stop().await;
}

#[tokio::test]
async fn ac_010_07_security_headers_on_ui_responses() {
    let s = server().await;
    for path in ["/login", "/"] {
        let r = s.http.get(s.url(path)).send().await.unwrap();
        let h = r.headers();
        assert_eq!(h["x-frame-options"], "DENY", "{path}");
        assert_eq!(h["x-content-type-options"], "nosniff", "{path}");
        assert_eq!(h["referrer-policy"], "same-origin", "{path}");
        let csp = h["content-security-policy"].to_str().unwrap();
        assert_eq!(csp, rustybee::http::CSP, "{path}");
        assert!(csp.contains("script-src 'self' https://challenges.cloudflare.com"));
        assert!(csp.contains("frame-ancestors 'none'") && !csp.contains("default-src"));
    }
    let page = s
        .http
        .get(s.url("/login"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(!page.contains("<script>"), "no inline script");
    assert!(page.contains(r#""allowEval":false,"allowScriptTags":false,"selfRequestsOnly":true"#));
    s.stop().await;
}
