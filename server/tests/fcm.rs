//! FCM wake (spec 009) against a fake OAuth + FCM server.

mod common;

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{StatusCode, Uri};
use axum::response::IntoResponse;
use common::*;
use rustybee::store::devices;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

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
    // Not again within 15 minutes.
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
    // After the window it fires again, reusing the cached access token.
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
