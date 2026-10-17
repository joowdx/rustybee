//! FCM wake (spec 009) against a fake OAuth + FCM server.

mod common;

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{StatusCode, Uri};
use axum::response::IntoResponse;
use common::*;
use rustybee::app::UiEvent;
use rustybee::store::devices;
use rustybee::store::rusqlite::params;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone, Default)]
struct Fake {
    calls: Arc<Mutex<Vec<(String, String)>>>,
    sends: Arc<Mutex<VecDeque<(u16, &'static str)>>>,
}

impl Fake {
    fn paths(&self, prefix: &str) -> usize {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(p, _)| p.starts_with(prefix))
            .count()
    }
}

async fn handle(State(f): State<Fake>, uri: Uri, body: Bytes) -> axum::response::Response {
    let path = uri.path().to_string();
    let body = String::from_utf8_lossy(&body).to_string();
    f.calls.lock().unwrap().push((path.clone(), body));
    if path == "/token" {
        return axum::Json(serde_json::json!({"access_token": "ya29.test", "expires_in": 3600}))
            .into_response();
    }
    let (status, text) = f.sends.lock().unwrap().pop_front().unwrap_or((200, "{}"));
    (StatusCode::from_u16(status).unwrap(), text).into_response()
}

async fn fake() -> (Fake, String) {
    let f = Fake::default();
    let app = Router::new().fallback(handle).with_state(f.clone());
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    (f, base)
}

async fn fcm_server(base: &str) -> Server {
    let dir = tempfile::tempdir().unwrap();
    let creds = dir.path().join("fcm.json");
    std::fs::write(
        &creds,
        serde_json::json!({
            "type": "service_account",
            "project_id": "demo-project",
            "client_email": "wake@demo-project.iam.gserviceaccount.com",
            "private_key": include_str!("data/fcm_test_key.pem"),
        })
        .to_string(),
    )
    .unwrap();
    let mut cfg = test_config(&dir);
    cfg.fcm_credentials = Some(creds);
    cfg.fcm_oauth_url = format!("{base}/token");
    cfg.fcm_base_url = base.to_string();
    cfg.fcm_interval = std::time::Duration::from_secs(3600);
    boot(dir, cfg).await
}

/// Pair a device with an FCM token and queue a message for it while offline.
async fn offline_device_with_work(s: &Server) -> i64 {
    let code = s.pairing_code().await;
    let r = s
        .http
        .post(s.url("/api/device/pair"))
        .json(&serde_json::json!({"code": code, "name": "A", "fcm_token": "tok-1"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let id = s.app().store.read(devices::list).await.unwrap()[0].id;
    let key = s.api_key().await;
    let (st, _) = s
        .api_post(
            &key,
            "/api/v1/messages",
            serde_json::json!({"to": ["+15550000001"], "body": "wake up"}),
        )
        .await;
    assert_eq!(st, 202);
    // Pairing counts as "seen"; the rule waits until the device has been quiet for 3 min.
    s.app().clock.advance_ms(4 * 60 * 1000);
    id
}

#[tokio::test]
async fn ac_009_01_no_credentials_no_fcm() {
    let s = server().await;
    assert!(s.app().fcm.is_none());
    s.stop().await;
}

#[tokio::test]
async fn ac_009_02_wake_once_per_window() {
    let (f, base) = fake().await;
    let s = fcm_server(&base).await;
    let id = offline_device_with_work(&s).await;
    let fcm = s.app().fcm.clone().unwrap();
    assert_eq!(
        rustybee::fcm::check_once(s.app(), &fcm).await.unwrap(),
        vec![id]
    );
    assert_eq!(f.paths("/v1/"), 1);
    assert_eq!(f.paths("/token"), 1);
    let calls = f.calls.lock().unwrap().clone();
    let (path, body) = calls.iter().find(|(p, _)| p.starts_with("/v1/")).unwrap();
    assert_eq!(path, "/v1/projects/demo-project/messages:send");
    let v: serde_json::Value = serde_json::from_str(body).unwrap();
    assert_eq!(v["message"]["token"], "tok-1");
    assert_eq!(v["message"]["data"], serde_json::json!({"t": "wake"}));
    let (tp, tb) = calls.iter().find(|(p, _)| p == "/token").unwrap();
    assert_eq!(tp, "/token");
    assert!(tb.contains("grant-type%3Ajwt-bearer&assertion="));
    // Not again inside the first backoff step (15 min while wake_attempts is 1).
    assert!(
        rustybee::fcm::check_once(s.app(), &fcm)
            .await
            .unwrap()
            .is_empty()
    );
    s.app().clock.advance_ms(10 * 60 * 1000);
    assert!(
        rustybee::fcm::check_once(s.app(), &fcm)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(f.paths("/v1/"), 1);
    // Sixteen minutes later the first step has elapsed, so it fires again.
    // Later steps grow; see ac_009_07. The cached access token is reused.
    s.app().clock.advance_ms(6 * 60 * 1000);
    assert_eq!(
        rustybee::fcm::check_once(s.app(), &fcm).await.unwrap(),
        vec![id]
    );
    assert_eq!(f.paths("/v1/"), 2);
    assert_eq!(f.paths("/token"), 1);
    s.stop().await;
}

#[tokio::test]
async fn ac_009_02_connected_device_is_not_woken() {
    let (f, base) = fake().await;
    let s = fcm_server(&base).await;
    let code = s.pairing_code().await;
    let r = s
        .http
        .post(s.url("/api/device/pair"))
        .json(&serde_json::json!({"code": code, "name": "A", "fcm_token": "tok-1"}))
        .send()
        .await
        .unwrap();
    let token = r.json::<serde_json::Value>().await.unwrap()["device_token"]
        .as_str()
        .unwrap()
        .to_string();
    let _d = Device::connect(&s, &token).await;
    let fcm = s.app().fcm.clone().unwrap();
    let key = s.api_key().await;
    s.api_post(
        &key,
        "/api/v1/messages",
        serde_json::json!({"to": ["+15550000001"], "body": "x"}),
    )
    .await;
    assert!(
        rustybee::fcm::check_once(s.app(), &fcm)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(f.paths("/v1/"), 0);
    s.stop().await;
}

#[tokio::test]
async fn ac_009_03_unregistered_clears_token() {
    let (f, base) = fake().await;
    f.sends.lock().unwrap().push_back((
        404,
        r#"{"error":{"code":404,"status":"NOT_FOUND","details":[{"errorCode":"UNREGISTERED"}]}}"#,
    ));
    let s = fcm_server(&base).await;
    let id = offline_device_with_work(&s).await;
    s.app().fcm_notify.notify_one();
    let cleared = eventually(|| async {
        s.app()
            .store
            .read(move |c| devices::by_id(c, id))
            .await
            .unwrap()
            .unwrap()
            .fcm_token
            .is_none()
    })
    .await;
    assert!(cleared);
    assert_eq!(f.paths("/v1/"), 1);
    s.stop().await;
}

async fn pair_fcm(s: &Server, name: &str, token: &str) -> (i64, String) {
    let code = s.pairing_code().await;
    let r = s
        .http
        .post(s.url("/api/device/pair"))
        .json(&serde_json::json!({"code": code, "name": name, "fcm_token": token}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let v: serde_json::Value = r.json().await.unwrap();
    let pid = v["device_id"].as_str().unwrap().to_string();
    let id = s
        .app()
        .store
        .read({
            let pid = pid.clone();
            move |c| devices::by_public(c, &pid)
        })
        .await
        .unwrap()
        .unwrap()
        .id;
    (id, pid)
}

async fn device(s: &Server, id: i64) -> devices::DeviceRow {
    s.app()
        .store
        .read(move |c| devices::by_id(c, id))
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn ac_009_06_idle_quiet_is_five_minutes() {
    let (_f, base) = fake().await;
    let s = fcm_server(&base).await;
    let (id, _) = pair_fcm(&s, "A", "tok-1").await;
    let fcm = s.app().fcm.clone().unwrap();
    assert!(
        rustybee::fcm::check_once(s.app(), &fcm)
            .await
            .unwrap()
            .is_empty()
    );
    // Idle, and quiet for under 5 min: no wake.
    s.app().clock.advance_ms(4 * 60 * 1000);
    assert!(
        rustybee::fcm::check_once(s.app(), &fcm)
            .await
            .unwrap()
            .is_empty()
    );
    // Quiet for over 5 min: one wake.
    s.app().clock.advance_ms(2 * 60 * 1000);
    assert_eq!(
        rustybee::fcm::check_once(s.app(), &fcm).await.unwrap(),
        vec![id]
    );
    s.stop().await;
}

#[tokio::test]
async fn ac_009_07_backoff_schedule() {
    let (_f, base) = fake().await;
    let s = fcm_server(&base).await;
    let (id, _) = pair_fcm(&s, "A", "tok-1").await;
    let old = s.app().clock.now_ms() - 10 * 60 * 1000;
    s.app()
        .store
        .write(move |c| {
            c.execute(
                "UPDATE devices SET last_seen_at=?2 WHERE id=?1",
                params![id, old],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let fcm = s.app().fcm.clone().unwrap();
    // +0, then +15, +45, +105, +225, +465 min, then every 8 h.
    let gaps_min = [15_i64, 30, 60, 120, 240, 480, 480];
    assert_eq!(
        rustybee::fcm::check_once(s.app(), &fcm).await.unwrap(),
        vec![id]
    );
    for gap in gaps_min {
        let gap_ms = gap * 60 * 1000;
        s.app().clock.advance_ms(gap_ms - 5_000);
        assert!(
            rustybee::fcm::check_once(s.app(), &fcm)
                .await
                .unwrap()
                .is_empty(),
            "woke {gap} min early"
        );
        s.app().clock.advance_ms(5_000 + 1);
        assert_eq!(
            rustybee::fcm::check_once(s.app(), &fcm).await.unwrap(),
            vec![id],
            "missing wake after {gap} min"
        );
    }
    s.stop().await;
}

#[tokio::test]
async fn ac_009_08_unreachable_after_unanswered_wake() {
    let (f, base) = fake().await;
    // First device is accepted, second call fails. Default after the queue is 200.
    f.sends.lock().unwrap().push_back((200, "{}"));
    f.sends.lock().unwrap().push_back((500, "temporary"));
    let s = fcm_server(&base).await;
    let (id_a, pid_a) = pair_fcm(&s, "A", "tok-a").await;
    let (id_b, _) = pair_fcm(&s, "B", "tok-b").await;
    let old = s.app().clock.now_ms() - 10 * 60 * 1000;
    s.app()
        .store
        .write(move |c| {
            c.execute("UPDATE devices SET last_seen_at=?1", [old])?;
            Ok(())
        })
        .await
        .unwrap();
    let fcm = s.app().fcm.clone().unwrap();
    assert_eq!(
        rustybee::fcm::check_once(s.app(), &fcm).await.unwrap(),
        vec![id_a],
        "only the wake FCM accepted counts as woken"
    );
    assert_eq!(device(&s, id_a).await.wake_attempts, 1);
    assert_eq!(device(&s, id_b).await.wake_attempts, 0);
    let mut rx = s.app().bus.subscribe();
    s.app().clock.advance_ms(2 * 60 * 1000 + 1000);
    assert!(
        rustybee::fcm::check_once(s.app(), &fcm)
            .await
            .unwrap()
            .is_empty()
    );
    let a = device(&s, id_a).await;
    let b = device(&s, id_b).await;
    assert_eq!(a.unreachable_at, a.last_wake_at);
    assert!(a.unreachable_at.is_some());
    assert!(
        b.unreachable_at.is_none(),
        "a failed FCM call never sets it"
    );
    let ev = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("device event")
        .unwrap();
    assert_eq!(ev, UiEvent::Device(pid_a));
    assert!(rx.try_recv().is_err());
    s.stop().await;
}

#[tokio::test]
async fn ac_009_09_hello_resets_backoff() {
    let (_f, base) = fake().await;
    let s = fcm_server(&base).await;
    let code = s.pairing_code().await;
    let r = s
        .http
        .post(s.url("/api/device/pair"))
        .json(&serde_json::json!({"code": code, "name": "A", "fcm_token": "tok-1"}))
        .send()
        .await
        .unwrap();
    let v: serde_json::Value = r.json().await.unwrap();
    let token = v["device_token"].as_str().unwrap().to_string();
    let (id, _) = pair_id(&s, &v).await;
    let wake_at = s.app().clock.now_ms() - 20 * 60 * 1000;
    s.app()
        .store
        .write(move |c| {
            c.execute(
                "UPDATE devices SET wake_attempts=6, unreachable_at=1, gateway_off_at=2, last_wake_at=?2
                 WHERE id=?1",
                params![id, wake_at],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let mut hello = hello_frame();
    hello["data"]["fcm_token"] = serde_json::json!("tok-1");
    let mut d = Device {
        ws: connect_raw(&s.ws_url(), &token).await.unwrap(),
    };
    d.send(&hello).await;
    assert_eq!(d.recv_type("ack").await["data"]["ref"], hello["id"]);
    d.recv_type("welcome").await;
    let row = device(&s, id).await;
    assert_eq!(row.wake_attempts, 0);
    assert!(row.unreachable_at.is_none());
    assert!(row.gateway_off_at.is_none());
    assert_eq!(row.last_wake_at, Some(wake_at));
    assert_eq!(row.fcm_token.as_deref(), Some("tok-1"));
    d.close().await;
    let down = eventually(|| async {
        !s.app()
            .store
            .read(move |c| devices::by_id(c, id))
            .await
            .unwrap()
            .unwrap()
            .connected
    })
    .await;
    assert!(down);
    // Quiet for an idle phone, and the 15 min step from the old wake has passed.
    // Attempts were 6, so this would still be inside the 8 h cap if hello had not reset them.
    s.app().clock.advance_ms(5 * 60 * 1000 + 1000);
    let fcm = s.app().fcm.clone().unwrap();
    assert_eq!(
        rustybee::fcm::check_once(s.app(), &fcm).await.unwrap(),
        vec![id]
    );
    s.stop().await;
}

async fn pair_id(s: &Server, v: &serde_json::Value) -> (i64, String) {
    let pid = v["device_id"].as_str().unwrap().to_string();
    let id = s
        .app()
        .store
        .read({
            let pid = pid.clone();
            move |c| devices::by_public(c, &pid)
        })
        .await
        .unwrap()
        .unwrap()
        .id;
    (id, pid)
}

#[tokio::test]
async fn ac_009_10_disabled_removed_tokenless_and_off_are_skipped() {
    let (f, base) = fake().await;
    let s = fcm_server(&base).await;
    let (control, _) = pair_fcm(&s, "control", "tok-c").await;
    let (disabled, _) = pair_fcm(&s, "disabled", "tok-d").await;
    let (tokenless, _) = pair_fcm(&s, "tokenless", "tok-t").await;
    let (off, _) = pair_fcm(&s, "off", "tok-o").await;
    let (removed, _) = pair_fcm(&s, "removed", "tok-r").await;
    let old = s.app().clock.now_ms() - 10 * 60 * 1000;
    s.app()
        .store
        .write(move |c| {
            c.execute("UPDATE devices SET last_seen_at=?1", [old])?;
            c.execute("UPDATE devices SET enabled=0 WHERE id=?1", [disabled])?;
            c.execute("UPDATE devices SET fcm_token=NULL WHERE id=?1", [tokenless])?;
            c.execute(
                "UPDATE devices SET gateway_off_at=?2 WHERE id=?1",
                params![off, old],
            )?;
            c.execute(
                "UPDATE devices SET wake_attempts=4, unreachable_at=99 WHERE id=?1",
                [removed],
            )?;
            devices::delete(c, removed, old)?;
            Ok(())
        })
        .await
        .unwrap();
    let (attempts, unreach): (i64, Option<i64>) = s
        .app()
        .store
        .read(move |c| {
            c.query_row(
                "SELECT wake_attempts, unreachable_at FROM devices WHERE id=?1",
                [removed],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
        })
        .await
        .unwrap();
    assert_eq!(attempts, 0);
    assert!(unreach.is_none());
    // Put a token back on the removed row so only `removed_at` keeps it out.
    s.app()
        .store
        .write(move |c| {
            c.execute(
                "UPDATE devices SET fcm_token='ghost', last_seen_at=?2 WHERE id=?1",
                params![removed, old],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let fcm = s.app().fcm.clone().unwrap();
    assert_eq!(
        rustybee::fcm::check_once(s.app(), &fcm).await.unwrap(),
        vec![control]
    );
    assert_eq!(f.paths("/v1/"), 1);
    // Now make the skipped rows look unanswered, which would flag them if the filters were wrong.
    let stale = s.app().clock.now_ms() - 3 * 60 * 1000;
    s.app()
        .store
        .write(move |c| {
            c.execute(
                "UPDATE devices SET wake_attempts=2, last_wake_at=?2
                 WHERE id IN (?1, ?3, ?4, ?5)",
                params![disabled, stale, tokenless, off, removed],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    assert!(
        rustybee::fcm::check_once(s.app(), &fcm)
            .await
            .unwrap()
            .is_empty()
    );
    for id in [disabled, tokenless, off] {
        assert!(device(&s, id).await.unreachable_at.is_none(), "{id}");
    }
    let removed_unreach: Option<i64> = s
        .app()
        .store
        .read(move |c| {
            c.query_row(
                "SELECT unreachable_at FROM devices WHERE id=?1",
                [removed],
                |r| r.get(0),
            )
        })
        .await
        .unwrap();
    assert!(removed_unreach.is_none());
    assert_eq!(f.paths("/v1/"), 1);
    s.stop().await;
}

#[tokio::test]
async fn ac_009_11_wake_body_and_health_token() {
    let (f, base) = fake().await;
    let s = fcm_server(&base).await;
    let code = s.pairing_code().await;
    let r = s
        .http
        .post(s.url("/api/device/pair"))
        .json(&serde_json::json!({"code": code, "name": "A", "fcm_token": "tok-1"}))
        .send()
        .await
        .unwrap();
    let v: serde_json::Value = r.json().await.unwrap();
    let token = v["device_token"].as_str().unwrap().to_string();
    let (id, _) = pair_id(&s, &v).await;
    // Idle (nothing queued), so no `send` frame races the acks below on reconnect.
    s.app().clock.advance_ms(6 * 60 * 1000);
    let fcm = s.app().fcm.clone().unwrap();
    assert_eq!(
        rustybee::fcm::check_once(s.app(), &fcm).await.unwrap(),
        vec![id]
    );
    let calls = f.calls.lock().unwrap().clone();
    let (_, body) = calls.iter().find(|(p, _)| p.starts_with("/v1/")).unwrap();
    let v: serde_json::Value = serde_json::from_str(body).unwrap();
    assert_eq!(v["message"]["android"]["priority"], "HIGH");
    assert_eq!(v["message"]["android"]["ttl"], "3600s");
    assert_eq!(v["message"]["android"]["collapse_key"], "wake");
    // hello from the fixture clears the token; health puts a new one in place.
    let mut d = Device::connect(&s, &token).await;
    assert!(device(&s, id).await.fcm_token.is_none());
    let h = frame(
        "health",
        serde_json::json!({"outbox": 1, "fcm_token": "tok-2"}),
    );
    d.send(&h).await;
    assert_eq!(d.recv_type("ack").await["data"]["ref"], h["id"]);
    assert_eq!(device(&s, id).await.fcm_token.as_deref(), Some("tok-2"));
    let h = frame("health", serde_json::json!({"outbox": 2}));
    d.send(&h).await;
    assert_eq!(d.recv_type("ack").await["data"]["ref"], h["id"]);
    assert_eq!(device(&s, id).await.fcm_token.as_deref(), Some("tok-2"));
    s.stop().await;
}
