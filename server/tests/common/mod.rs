//! Shared helpers for integration tests: a server on an ephemeral port with a
//! tempdir database, an HTTP client, and a fake device over WebSocket.
#![allow(dead_code, clippy::result_large_err)]

use futures_util::{SinkExt, StreamExt};
use rustybee::app::{Running, start};
use rustybee::config::Config;
use rustybee::store::auth;
use serde_json::{Value, json};
use std::time::Duration;
use tempfile::TempDir;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

pub const PASSWORD: &str = "correct horse battery";

pub struct Server {
    pub running: Option<Running>,
    pub dir: TempDir,
    pub base: String,
    pub http: reqwest::Client,
}

/// Short timings so tests stay fast.
pub fn test_config(dir: &TempDir) -> Config {
    let mut c = Config::for_tests(dir.path().to_path_buf());
    c.inflight_timeout = Duration::from_millis(800);
    c.sweep_interval = Duration::from_secs(3600);
    c.webhook_backoff = vec![Duration::from_millis(50); 6];
    c.webhook_timeout = Duration::from_secs(2);
    c.webhook_lease = Duration::from_secs(5);
    c
}

pub async fn server() -> Server {
    server_with(|_| {}).await
}

pub async fn server_with(f: impl FnOnce(&mut Config)) -> Server {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = test_config(&dir);
    f(&mut cfg);
    boot(dir, cfg).await
}

pub async fn boot(dir: TempDir, cfg: Config) -> Server {
    let running = start(cfg).await.expect("server starts");
    let base = format!("http://{}", running.addr);
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    Server {
        running: Some(running),
        dir,
        base,
        http,
    }
}

impl Server {
    pub fn app(&self) -> &rustybee::app::App {
        &self.running.as_ref().unwrap().app
    }

    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    pub fn ws_url(&self) -> String {
        format!(
            "{}/api/device/ws",
            self.base.replacen("http://", "ws://", 1)
        )
    }

    /// Stop the server and start a new one on the same data directory.
    pub async fn restart(mut self, f: impl FnOnce(&mut Config)) -> Server {
        if let Some(r) = self.running.take() {
            r.stop().await;
        }
        let mut cfg = test_config(&self.dir);
        f(&mut cfg);
        boot(self.dir, cfg).await
    }

    pub async fn stop(mut self) {
        if let Some(r) = self.running.take() {
            r.stop().await;
        }
    }

    pub async fn api_key(&self) -> String {
        let now = self.app().clock.now_ms();
        let (_, secret) = self
            .app()
            .store
            .write(move |c| auth::create_key(c, "test", now))
            .await
            .unwrap();
        secret
    }

    pub async fn pairing_code(&self) -> String {
        let now = self.app().clock.now_ms();
        self.app()
            .store
            .write(move |c| auth::create_pairing_code(c, now))
            .await
            .unwrap()
            .0
    }

    /// Pair a device; returns `(device_id, device_token)`.
    pub async fn pair(&self, name: &str) -> (String, String) {
        let code = self.pairing_code().await;
        let r = self
            .http
            .post(self.url("/api/device/pair"))
            .json(
                &json!({"code": code, "name": name, "manufacturer": "Google", "model": "Pixel 10",
                "os_version": "16", "app_version": "0.1.0", "app_build": 1, "fcm_token": null}),
            )
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200);
        let v: Value = r.json().await.unwrap();
        (
            v["device_id"].as_str().unwrap().to_string(),
            v["device_token"].as_str().unwrap().to_string(),
        )
    }

    pub async fn set_password(&self) {
        let h = auth::hash_password(PASSWORD);
        self.app()
            .store
            .write(move |c| auth::set_password_hash(c, &h))
            .await
            .unwrap();
    }

    /// Log in through the form; returns the cookie pair `rb_session=…`.
    pub async fn login(&self) -> String {
        self.set_password().await;
        let r = self
            .http
            .post(self.url("/login"))
            .header("Origin", &self.base)
            .form(&[("password", PASSWORD)])
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 303);
        let c = r.headers()["set-cookie"].to_str().unwrap();
        c.split(';').next().unwrap().to_string()
    }

    pub async fn api_post(&self, key: &str, path: &str, body: Value) -> (u16, Value) {
        let r = self
            .http
            .post(self.url(path))
            .bearer_auth(key)
            .json(&body)
            .send()
            .await
            .unwrap();
        let s = r.status().as_u16();
        (s, r.json().await.unwrap_or(Value::Null))
    }

    pub async fn api_get(&self, key: &str, path: &str) -> (u16, Value) {
        let r = self
            .http
            .get(self.url(path))
            .bearer_auth(key)
            .send()
            .await
            .unwrap();
        let s = r.status().as_u16();
        (s, r.json().await.unwrap_or(Value::Null))
    }

    pub async fn message_state(&self, key: &str, id: &str) -> String {
        let (_, v) = self.api_get(key, &format!("/api/v1/messages/{id}")).await;
        v["state"].as_str().unwrap_or("").to_string()
    }
}

pub type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

pub struct Device {
    pub ws: Ws,
}

pub async fn connect_raw(
    url: &str,
    token: &str,
) -> Result<Ws, tokio_tungstenite::tungstenite::Error> {
    let mut req = url.into_client_request().unwrap();
    req.headers_mut()
        .insert("Authorization", format!("Bearer {token}").parse().unwrap());
    tokio_tungstenite::connect_async(req)
        .await
        .map(|(ws, _)| ws)
}

pub fn hello_frame() -> Value {
    let mut v: Value =
        serde_json::from_str(include_str!("../../../protocol/fixtures/hello.json")).unwrap();
    v["id"] = json!(ulid::Ulid::new().to_string());
    v
}

pub fn frame(kind: &str, data: Value) -> Value {
    json!({"v": 1, "id": ulid::Ulid::new().to_string(), "type": kind, "data": data})
}

impl Device {
    /// Connect and complete `hello` → `ack` + `welcome`.
    pub async fn connect(s: &Server, token: &str) -> Device {
        let ws = connect_raw(&s.ws_url(), token).await.expect("ws connects");
        let mut d = Device { ws };
        let hello = hello_frame();
        d.send(&hello).await;
        let ack = d.recv_type("ack").await;
        assert_eq!(ack["data"]["ref"], hello["id"]);
        let w = d.recv_type("welcome").await;
        assert!(w["data"]["config"].is_object());
        d
    }

    pub async fn send(&mut self, v: &Value) {
        self.ws
            .send(Message::Text(v.to_string().into()))
            .await
            .unwrap();
    }

    pub async fn send_text(&mut self, t: &str) {
        self.ws
            .send(Message::Text(t.to_string().into()))
            .await
            .unwrap();
    }

    /// Next text frame within `secs`, or `None` on timeout/close.
    pub async fn recv_within(&mut self, ms: u64) -> Option<Value> {
        let deadline = tokio::time::Instant::now() + Duration::from_millis(ms);
        loop {
            let msg = tokio::time::timeout_at(deadline, self.ws.next())
                .await
                .ok()??;
            match msg {
                Ok(Message::Text(t)) => return Some(serde_json::from_str(t.as_str()).unwrap()),
                Ok(Message::Close(_)) | Err(_) => return None,
                Ok(_) => continue,
            }
        }
    }

    pub async fn recv(&mut self) -> Value {
        self.recv_within(3000).await.expect("frame from server")
    }

    /// Next frame of a type, skipping `config` pushes.
    pub async fn recv_type(&mut self, kind: &str) -> Value {
        loop {
            let v = self.recv().await;
            if v["type"] == kind {
                return v;
            }
            assert!(v["type"] == "config", "expected {kind}, got {v}");
        }
    }

    /// Wait for the close frame and return its code.
    pub async fn close_code(&mut self, ms: u64) -> Option<u16> {
        let deadline = tokio::time::Instant::now() + Duration::from_millis(ms);
        loop {
            let msg = tokio::time::timeout_at(deadline, self.ws.next())
                .await
                .ok()?;
            match msg {
                Some(Ok(Message::Close(Some(f)))) => return Some(u16::from(f.code)),
                Some(Ok(Message::Close(None))) => return Some(u16::from(CloseCode::Status)),
                Some(Ok(_)) => continue,
                Some(Err(_)) | None => return None,
            }
        }
    }

    pub async fn ack(&mut self, id: &Value) {
        let f = frame("ack", json!({"ref": id}));
        self.send(&f).await;
    }

    /// Send a status for a message and wait for the server's ack.
    pub async fn status(&mut self, msg_id: &str, state: &str) {
        let f = frame(
            "status",
            json!({"msg_id": msg_id, "state": state, "at": 1_760_000_001_000i64, "parts": 1, "error": null}),
        );
        self.send(&f).await;
        let a = self.recv_type("ack").await;
        assert_eq!(a["data"]["ref"], f["id"]);
    }

    pub async fn close(mut self) {
        let _ = self.ws.close(None).await;
    }
}

/// Poll `f` until it returns true or ~3 s pass.
pub async fn eventually<F, Fut>(mut f: F) -> bool
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    for _ in 0..150 {
        if f().await {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    false
}
