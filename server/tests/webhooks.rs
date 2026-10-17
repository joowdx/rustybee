//! Webhooks (spec 008) against a local axum catcher.

mod common;

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use common::*;
use rustybee::domain::lifecycle::Event;
use rustybee::domain::signing;
use rustybee::protocol::PairRequest;
use rustybee::store::{Store, devices, messages, webhooks};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
struct Catcher {
    hits: Arc<Mutex<Vec<(HeaderMap, String)>>>,
    /// Statuses to answer with, in order; 200 once exhausted.
    script: Arc<Mutex<VecDeque<u16>>>,
}

async fn catch(State(c): State<Catcher>, headers: HeaderMap, body: Bytes) -> StatusCode {
    c.hits
        .lock()
        .unwrap()
        .push((headers, String::from_utf8_lossy(&body).to_string()));
    let s = c.script.lock().unwrap().pop_front().unwrap_or(200);
    StatusCode::from_u16(s).unwrap()
}

async fn catcher(script: &[u16]) -> (Catcher, String) {
    let c = Catcher::default();
    c.script.lock().unwrap().extend(script.iter().copied());
    let app = Router::new().fallback(catch).with_state(c.clone());
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/hook", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    (c, url)
}

impl Catcher {
    fn count(&self) -> usize {
        self.hits.lock().unwrap().len()
    }
}

async fn hook(s: &Server, url: &str, events: &[&str], active: bool) -> webhooks::WebhookRow {
    let url = url.to_string();
    let events: Vec<String> = events.iter().map(|e| e.to_string()).collect();
    let now = s.app().clock.now_ms();
    s.app()
        .store
        .write(move |c| webhooks::create(c, "h", &url, &events, active, now))
        .await
        .unwrap()
}

/// Queue a message and apply events to it; returns its public id.
async fn message_through(s: &Server, key: &str, events: Vec<Event>) -> String {
    let (_, v) = s
        .api_post(
            key,
            "/api/v1/messages",
            serde_json::json!({"to": ["+15550000001"], "body": "hi"}),
        )
        .await;
    let pid = v["messages"][0]["id"].as_str().unwrap().to_string();
    let p = pid.clone();
    s.app()
        .store
        .write(move |c| {
            let m = messages::by_public(c, &p)?.unwrap();
            for e in events {
                messages::apply_event(c, m.id, e, 10, Some(1), None, 10)?;
            }
            Ok(())
        })
        .await
        .unwrap();
    s.app().webhook_notify.notify_one();
    pid
}

async fn deliveries(s: &Server, hook_id: i64) -> Vec<webhooks::DeliveryRow> {
    s.app()
        .store
        .read(move |c| webhooks::deliveries_for_webhook(c, hook_id, None, 100))
        .await
        .unwrap()
}

#[tokio::test]
async fn ac_008_01_one_delivery_per_subscribed_active_hook() {
    let s = server().await;
    let key = s.api_key().await;
    s.pair("A").await;
    let (_c, url) = catcher(&[]).await;
    let sent = hook(&s, &url, &["message.sent"], true).await;
    let other = hook(&s, &url, &["message.delivered"], true).await;
    let inactive = hook(&s, &url, &["message.sent"], false).await;
    message_through(
        &s,
        &key,
        vec![Event::Dispatch, Event::StatusSent, Event::StatusSent],
    )
    .await;
    assert_eq!(deliveries(&s, sent.id).await.len(), 1);
    assert_eq!(deliveries(&s, other.id).await.len(), 0);
    assert_eq!(deliveries(&s, inactive.id).await.len(), 0);
    let d = &deliveries(&s, sent.id).await[0];
    assert_eq!(d.event, "message.sent");
    let p: serde_json::Value = serde_json::from_str(&d.payload).unwrap();
    assert_eq!(p["event"], "message.sent");
    assert_eq!(p["data"]["state"], "sent");
    s.stop().await;
}

#[tokio::test]
async fn ac_008_02_signature_verifies_with_recipe() {
    let s = server().await;
    let key = s.api_key().await;
    s.pair("A").await;
    let (c, url) = catcher(&[]).await;
    let h = hook(&s, &url, &["message.sent"], true).await;
    message_through(&s, &key, vec![Event::Dispatch, Event::StatusSent]).await;
    assert!(eventually(|| async { c.count() == 1 }).await);
    let (headers, body) = c.hits.lock().unwrap()[0].clone();
    assert_eq!(headers["x-rustybee-event"], "message.sent");
    assert!(headers.contains_key("x-rustybee-delivery"));
    let sig = headers["x-rustybee-signature"].to_str().unwrap();
    let now = s.app().clock.now_ms() / 1000;
    assert!(signing::verify(&h.secret, sig, body.as_bytes(), now, 300));
    assert!(!signing::verify(
        "another secret",
        sig,
        body.as_bytes(),
        now,
        300
    ));
    assert!(!signing::verify(&h.secret, sig, b"tampered", now, 300));
    s.stop().await;
}

#[tokio::test]
async fn ac_008_03_retry_on_500_then_delivered() {
    let s = server().await;
    let key = s.api_key().await;
    s.pair("A").await;
    let (c, url) = catcher(&[500, 503]).await;
    let h = hook(&s, &url, &["message.sent"], true).await;
    message_through(&s, &key, vec![Event::Dispatch, Event::StatusSent]).await;
    assert!(
        eventually(|| async { c.count() == 3 }).await,
        "hits: {}",
        c.count()
    );
    assert!(eventually(|| async { deliveries(&s, h.id).await[0].delivered_at.is_some() }).await);
    let d = &deliveries(&s, h.id).await[0];
    assert_eq!(d.attempts, 3);
    assert_eq!(d.last_status, Some(200));
    assert_eq!(d.status_label(), "delivered");
    // The same delivery id is reused on every attempt.
    let ids: Vec<String> = c
        .hits
        .lock()
        .unwrap()
        .iter()
        .map(|(h, _)| h["x-rustybee-delivery"].to_str().unwrap().to_string())
        .collect();
    assert!(ids.windows(2).all(|w| w[0] == w[1]));
    s.stop().await;
}

#[tokio::test]
async fn ac_008_03_abort_after_third_4xx() {
    let s = server().await;
    let key = s.api_key().await;
    s.pair("A").await;
    let (c, url) = catcher(&[400, 400, 400, 400, 400]).await;
    let h = hook(&s, &url, &["message.sent"], true).await;
    message_through(&s, &key, vec![Event::Dispatch, Event::StatusSent]).await;
    assert!(eventually(|| async { deliveries(&s, h.id).await[0].aborted_at.is_some() }).await);
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert_eq!(c.count(), 3);
    let d = &deliveries(&s, h.id).await[0];
    assert_eq!(d.attempts, 3);
    assert_eq!(d.last_status, Some(400));
    assert_eq!(d.status_label(), "aborted");
    s.stop().await;
}

#[tokio::test]
async fn ac_008_06_endless_response_body_is_cut_short() {
    // A receiver that answers 400 with a body that never ends.
    let endless = Router::new().fallback(|| async {
        let chunk = Bytes::from(vec![b'z'; 1024]);
        let body = futures_util::stream::repeat_with(move || {
            Ok::<_, std::convert::Infallible>(chunk.clone())
        });
        (StatusCode::BAD_REQUEST, axum::body::Body::from_stream(body))
    });
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/hook", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, endless).await.unwrap() });
    let s = server_with(|c| c.webhook_timeout = std::time::Duration::from_secs(30)).await;
    let key = s.api_key().await;
    s.pair("A").await;
    let h = hook(&s, &url, &["message.sent"], true).await;
    message_through(&s, &key, vec![Event::Dispatch, Event::StatusSent]).await;
    assert!(eventually(|| async { deliveries(&s, h.id).await[0].attempts >= 1 }).await);
    let d = &deliveries(&s, h.id).await[0];
    assert_eq!(d.last_status, Some(400));
    assert_eq!(d.last_error.as_deref(), Some("z".repeat(500).as_str()));
    s.stop().await;
}

#[tokio::test]
async fn ac_008_05_redeliver_sends_again() {
    let s = server().await;
    let key = s.api_key().await;
    s.pair("A").await;
    let (c, url) = catcher(&[]).await;
    let h = hook(&s, &url, &["message.sent"], true).await;
    message_through(&s, &key, vec![Event::Dispatch, Event::StatusSent]).await;
    assert!(eventually(|| async { c.count() == 1 }).await);
    assert!(eventually(|| async { deliveries(&s, h.id).await[0].delivered_at.is_some() }).await);
    let d = deliveries(&s, h.id).await.remove(0);
    // Through the UI button.
    let cookie = s.login().await;
    let r = s
        .http
        .post(s.url(&format!("/webhooks/deliveries/{}/redeliver", d.public_id)))
        .header("Origin", &s.base)
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 303);
    assert!(eventually(|| async { c.count() == 2 }).await);
    let bodies: Vec<String> = c.hits.lock().unwrap().iter().map(|h| h.1.clone()).collect();
    assert_eq!(bodies[0], bodies[1]);
    s.stop().await;
}

#[tokio::test]
async fn ac_008_04_lease_prevents_double_claim() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("t.db")).unwrap();
    let ids = store
        .write(|c| {
            let req: PairRequest = serde_json::from_str(r#"{"code":"X","name":"A"}"#).unwrap();
            let d = devices::create(c, &req, b"sha", 0)?;
            webhooks::create(
                c,
                "h",
                "http://127.0.0.1:9/",
                &["message.sent".into()],
                true,
                0,
            )?;
            let (_, rows) = messages::create_batch(
                c,
                d.id,
                &[messages::NewMessage {
                    to: "+15550000001".into(),
                    body: "x".into(),
                }],
                None,
                0,
            )?;
            messages::apply_event(c, rows[0].id, Event::Dispatch, 1, None, None, 1)?;
            messages::apply_event(c, rows[0].id, Event::StatusSent, 2, None, None, 2)?;
            Ok(())
        })
        .await;
    ids.unwrap();
    let claim = |now: i64| store.write(move |c| webhooks::claim(c, now, 60_000, 16));
    // Two workers racing at the same instant: only one gets the row.
    let (a, b) = tokio::join!(claim(10), claim(10));
    let (a, b) = (a.unwrap(), b.unwrap());
    assert_eq!(a.len() + b.len(), 1);
    // A "restart" before the lease ends does not pick it up again.
    assert!(claim(30_000).await.unwrap().is_empty());
    // After the lease expires it is claimed again.
    let again = claim(60_011).await.unwrap();
    assert_eq!(again.len(), 1);
    let first = if a.is_empty() { &b[0] } else { &a[0] };
    assert_eq!(again[0].public_id, first.public_id);
}

#[tokio::test]
async fn webhook_test_button_reports_status() {
    let s = server().await;
    let (c, url) = catcher(&[204]).await;
    let h = hook(&s, &url, &["message.received"], true).await;
    let cookie = s.login().await;
    let r = s
        .http
        .post(s.url(&format!("/webhooks/{}/test", h.public_id)))
        .header("Origin", &s.base)
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 303);
    assert!(
        r.headers()["location"]
            .to_str()
            .unwrap()
            .ends_with("ok=test_204")
    );
    assert_eq!(c.count(), 1);
    s.stop().await;
}

#[tokio::test]
async fn ac_008_07_redirect_is_a_failed_delivery() {
    // The target of the redirect counts any request that reaches it.
    let (target, target_url) = catcher(&[]).await;
    let posts = Arc::new(Mutex::new(Vec::<String>::new()));
    let p = posts.clone();
    let loc = target_url.clone();
    let redirecting = Router::new().fallback(move |method: axum::http::Method| {
        let p = p.clone();
        let loc = loc.clone();
        async move {
            p.lock().unwrap().push(method.to_string());
            (StatusCode::FOUND, [(axum::http::header::LOCATION, loc)])
        }
    });
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/hook", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, redirecting).await.unwrap() });

    let s = server().await;
    let key = s.api_key().await;
    s.pair("A").await;
    let h = hook(&s, &url, &["message.sent"], true).await;
    message_through(&s, &key, vec![Event::Dispatch, Event::StatusSent]).await;
    assert!(eventually(|| async { deliveries(&s, h.id).await[0].aborted_at.is_some() }).await);
    let d = &deliveries(&s, h.id).await[0];
    // Treated like a 4xx: three attempts, then aborted.
    assert_eq!(d.attempts, 3);
    assert!(d.delivered_at.is_none());
    assert_eq!(d.last_status, Some(302));
    let err = d.last_error.as_deref().unwrap();
    assert!(err.contains("302") && err.contains(&target_url), "{err}");
    assert_eq!(*posts.lock().unwrap(), vec!["POST"; 3]);
    assert_eq!(target.count(), 0, "the redirect was followed");
    s.stop().await;
}

#[tokio::test]
async fn ac_008_08_inactive_hook_deliveries_wait_until_reenabled() {
    let s =
        server_with(|c| c.webhook_backoff = vec![std::time::Duration::from_millis(300); 6]).await;
    let key = s.api_key().await;
    s.pair("A").await;
    let (c, url) = catcher(&[500]).await;
    let h = hook(&s, &url, &["message.sent"], true).await;
    message_through(&s, &key, vec![Event::Dispatch, Event::StatusSent]).await;
    assert!(eventually(|| async { c.count() == 1 }).await);
    let set_active = |active: bool| {
        let (pid, url) = (h.public_id.clone(), url.clone());
        s.app()
            .store
            .write(move |c| webhooks::update(c, &pid, "h", &url, &["message.sent".into()], active))
    };
    set_active(false).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(800)).await;
    assert_eq!(c.count(), 1, "a disabled hook was posted to");
    assert_eq!(deliveries(&s, h.id).await[0].status_label(), "retrying");
    // Re-enabling resumes the pending delivery.
    set_active(true).await.unwrap();
    s.app().webhook_notify.notify_one();
    assert!(eventually(|| async { deliveries(&s, h.id).await[0].delivered_at.is_some() }).await);
    assert_eq!(c.count(), 2);
    s.stop().await;
}

#[tokio::test]
async fn ac_008_09_shared_http_client_has_an_overall_timeout() {
    // A server that accepts connections and never answers.
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", l.local_addr().unwrap());
    tokio::spawn(async move {
        let mut held = vec![];
        while let Ok((sock, _)) = l.accept().await {
            held.push(sock);
        }
    });
    let s = server_with(|c| c.http_timeout = std::time::Duration::from_millis(300)).await;
    let t = std::time::Instant::now();
    let e = s.app().http.get(&url).send().await.unwrap_err();
    assert!(e.is_timeout(), "{e}");
    assert!(t.elapsed() < std::time::Duration::from_secs(3));
    s.stop().await;
}
