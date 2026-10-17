//! Device pairing and WebSocket behaviour (spec 002) against a fake device.

mod common;

use common::*;
use futures_util::SinkExt;
use rustybee::store::{devices, webhooks};
use serde_json::json;
use std::time::Duration;
use tokio_tungstenite::tungstenite;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;

async fn internal_id(s: &Server, public: &str) -> i64 {
    let p = public.to_string();
    s.app()
        .store
        .read(move |c| devices::by_public(c, &p))
        .await
        .unwrap()
        .unwrap()
        .id
}

#[tokio::test]
async fn ac_002_02_pairing_code_single_use_and_expiry() {
    let s = server().await;
    let code = s.pairing_code().await;
    let body = json!({"code": code, "name": "A"});
    let r = s
        .http
        .post(s.url("/api/device/pair"))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let v: serde_json::Value = r.json().await.unwrap();
    assert!(v["device_token"].as_str().unwrap().starts_with("rbd_"));
    assert_eq!(v["ws_url"], "ws://127.0.0.1/api/device/ws");
    // The token opens the socket.
    let d = Device::connect(&s, v["device_token"].as_str().unwrap()).await;
    d.close().await;
    // Reuse fails.
    let r = s
        .http
        .post(s.url("/api/device/pair"))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
    let e: serde_json::Value = r.json().await.unwrap();
    assert_eq!(e["error"]["code"], "pairing_invalid");
    // An expired code fails.
    let code = s.pairing_code().await;
    s.app().clock.advance_ms(11 * 60 * 1000);
    let r = s
        .http
        .post(s.url("/api/device/pair"))
        .json(&json!({"code": code}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
    s.stop().await;
}

#[tokio::test]
async fn ac_002_03_bad_token_is_401_before_upgrade() {
    let s = server().await;
    for token in ["rbd_nope", "rbk_wrongkind", ""] {
        match connect_raw(&s.ws_url(), token).await {
            Err(tungstenite::Error::Http(resp)) => assert_eq!(resp.status(), 401),
            other => panic!("expected 401, got {:?}", other.map(|_| ())),
        }
    }
    s.stop().await;
}

#[tokio::test]
async fn ac_002_06_second_connection_replaces_first_with_4000() {
    let s = server().await;
    let (_, token) = s.pair("A").await;
    let mut first = Device::connect(&s, &token).await;
    let mut second = Device::connect(&s, &token).await;
    assert_eq!(first.close_code(2000).await, Some(4000));
    // The new socket keeps working.
    let h = frame("health", json!({"outbox": 1}));
    second.send(&h).await;
    assert_eq!(second.recv_type("ack").await["data"]["ref"], h["id"]);
    s.stop().await;
}

#[tokio::test]
async fn ac_002_07_removing_device_closes_with_4003() {
    let s = server().await;
    let (pid, token) = s.pair("A").await;
    let mut d = Device::connect(&s, &token).await;
    let id = internal_id(&s, &pid).await;
    s.app().remove_device(id).await.unwrap();
    assert_eq!(d.close_code(2000).await, Some(4003));
    // The token no longer works.
    assert!(connect_raw(&s.ws_url(), &token).await.is_err());
    s.stop().await;
}

#[tokio::test]
async fn ac_002_08_malformed_frame_gets_nack_and_stays_open() {
    let s = server().await;
    let (_, token) = s.pair("A").await;
    let mut d = Device::connect(&s, &token).await;
    d.send_text(
        r#"{"v":1,"id":"01JABADFRAME0000000000000X","type":"status","data":{"state":"sent"}}"#,
    )
    .await;
    let n = d.recv_type("nack").await;
    assert_eq!(n["data"]["ref"], "01JABADFRAME0000000000000X");
    assert_eq!(n["data"]["code"], "invalid");
    d.send_text(r#"{"v":1,"id":"01JABADFRAME0000000000000Y","type":"teleport","data":{}}"#)
        .await;
    assert_eq!(d.recv_type("nack").await["data"]["code"], "unsupported");
    // Garbage without an id is ignored; the socket stays open.
    d.send_text("not json").await;
    let h = frame("health", json!({"outbox": 0}));
    d.send(&h).await;
    assert_eq!(d.recv_type("ack").await["data"]["ref"], h["id"]);
    s.stop().await;
}

#[tokio::test]
async fn ac_002_04_unacked_send_is_replayed_with_same_id() {
    let s = server().await;
    let key = s.api_key().await;
    let (_, token) = s.pair("A").await;
    let (st, v) = s
        .api_post(
            &key,
            "/api/v1/messages",
            json!({"to": ["+15550001111"], "body": "hi"}),
        )
        .await;
    assert_eq!(st, 202);
    let msg = v["messages"][0]["id"].as_str().unwrap().to_string();
    let mut d = Device::connect(&s, &token).await;
    let send = d.recv_type("send").await;
    assert_eq!(send["data"]["msg_id"], msg.as_str());
    d.close().await; // drop without acking
    let mut d = Device::connect(&s, &token).await;
    let again = d.recv_type("send").await;
    assert_eq!(again["id"], send["id"]);
    assert_eq!(again["data"]["msg_id"], msg.as_str());
    s.stop().await;
}

#[tokio::test]
async fn ac_002_05_duplicate_inbound_and_status_are_idempotent() {
    let s = server().await;
    let key = s.api_key().await;
    let (_, token) = s.pair("A").await;
    let now = s.app().clock.now_ms();
    s.app()
        .store
        .write(move |c| {
            webhooks::create(
                c,
                "sink",
                "http://127.0.0.1:9/none",
                &["message.sent".to_string(), "message.received".to_string()],
                true,
                now,
            )
        })
        .await
        .unwrap();
    let mut d = Device::connect(&s, &token).await;
    // Inbound twice with the same uid, different frame ids.
    let inbound = json!({"uid": "3b0368d22af9d295abcbe52c2ce7fdaa", "from": "+15551234567",
        "body": "Your code is 123456", "sim": 1, "received_at": 1_760_000_000_000i64});
    for _ in 0..2 {
        let f = frame("inbound", inbound.clone());
        d.send(&f).await;
        assert_eq!(d.recv_type("ack").await["data"]["ref"], f["id"]);
    }
    // Same frame re-sent verbatim is also acked.
    let f = frame("inbound", inbound.clone());
    d.send(&f).await;
    d.recv_type("ack").await;
    d.send(&f).await;
    d.recv_type("ack").await;
    let (_, list) = s.api_get(&key, "/api/v1/messages?direction=in").await;
    assert_eq!(list["items"].as_array().unwrap().len(), 1);
    // Status `sent` twice.
    let (_, v) = s
        .api_post(
            &key,
            "/api/v1/messages",
            json!({"to": ["+15550001111"], "body": "x"}),
        )
        .await;
    let msg = v["messages"][0]["id"].as_str().unwrap().to_string();
    let send = d.recv_type("send").await;
    d.ack(&send["id"]).await;
    d.status(&msg, "sent").await;
    d.status(&msg, "sent").await;
    let count = s
        .app()
        .store
        .read(|c| {
            c.query_row(
                "SELECT count(*) FROM webhook_deliveries WHERE event='message.sent'",
                [],
                |r| r.get::<_, i64>(0),
            )
        })
        .await
        .unwrap();
    assert_eq!(count, 1);
    let (_, m) = s.api_get(&key, &format!("/api/v1/messages/{msg}")).await;
    assert_eq!(m["state"], "sent");
    s.stop().await;
}

#[tokio::test]
async fn ac_002_09_idle_socket_is_closed() {
    let s = server_with(|c| c.idle_timeout = Duration::from_millis(300)).await;
    let (_, token) = s.pair("A").await;
    let mut d = Device::connect(&s, &token).await;
    assert_eq!(d.close_code(2000).await, Some(1001));
    s.stop().await;
}

#[tokio::test]
async fn ac_002_09_traffic_keeps_socket_open() {
    let s = server_with(|c| c.idle_timeout = Duration::from_millis(400)).await;
    let (_, token) = s.pair("A").await;
    let mut d = Device::connect(&s, &token).await;
    for _ in 0..4 {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let h = frame("health", json!({"outbox": 0}));
        d.send(&h).await;
        assert_eq!(d.recv_type("ack").await["data"]["ref"], h["id"]);
    }
    s.stop().await;
}

#[tokio::test]
async fn ac_002_10_set_forwarding_checks_rev() {
    let s = server().await;
    let (_, token) = s.pair("A").await;
    let mut d = Device::connect(&s, &token).await;
    let rules = json!({"mode": "block", "rules": [{"field": "body", "op": "contains", "value": "promo", "ignore_case": true}]});
    let f = frame(
        "set_forwarding",
        json!({"base_rev": 0, "receive_enabled": false, "filters": rules}),
    );
    d.send(&f).await;
    let a = d.recv_type("ack").await;
    assert_eq!(a["data"]["ref"], f["id"]);
    let cfg = d.recv_type("config").await;
    assert_eq!(cfg["data"]["filters_rev"], 1);
    assert_eq!(cfg["data"]["receive_enabled"], false);
    assert_eq!(cfg["data"]["filters"], rules);

    // A replay of the same frame (its ack was lost) is acked again, not reapplied or refused.
    d.send(&f).await;
    assert_eq!(d.recv_type("ack").await["data"]["ref"], f["id"]);
    assert_eq!(d.recv_type("config").await["data"]["filters_rev"], 1);

    // Stale base: refused, and the current config comes back.
    let f = frame(
        "set_forwarding",
        json!({"base_rev": 0, "receive_enabled": true, "filters": {"mode": "off", "rules": []}}),
    );
    d.send(&f).await;
    let n = d.recv_type("nack").await;
    assert_eq!(n["data"]["code"], "filters_conflict");
    let cfg = d.recv_type("config").await;
    assert_eq!(cfg["data"]["filters_rev"], 1);
    assert_eq!(cfg["data"]["filters"]["mode"], "block");
    assert_eq!(cfg["data"]["receive_enabled"], false);

    // Invalid rules are refused without a change.
    let f = frame(
        "set_forwarding",
        json!({"base_rev": 1, "receive_enabled": true, "filters": {"mode": "allow", "rules": [{"field": "body", "op": "equals", "value": " "}]}}),
    );
    d.send(&f).await;
    assert_eq!(d.recv_type("nack").await["data"]["code"], "invalid");
    s.stop().await;
}

async fn count(s: &Server, sql: &'static str) -> i64 {
    s.app()
        .store
        .read(move |c| c.query_row(sql, [], |r| r.get::<_, i64>(0)))
        .await
        .unwrap()
}

async fn sink_webhook(s: &Server, events: &[&str]) {
    let now = s.app().clock.now_ms();
    let events: Vec<String> = events.iter().map(|e| e.to_string()).collect();
    s.app()
        .store
        .write(move |c| webhooks::create(c, "sink", "http://127.0.0.1:9/none", &events, true, now))
        .await
        .unwrap();
}

#[tokio::test]
async fn ac_002_14_inbound_dropped_while_forwarding_off() {
    let s = server().await;
    let (_, token) = s.pair("A").await;
    sink_webhook(&s, &["message.received"]).await;
    let mut d = Device::connect(&s, &token).await;
    let off = frame(
        "set_forwarding",
        json!({"base_rev": 0, "receive_enabled": false, "filters": {"mode": "off", "rules": []}}),
    );
    d.send(&off).await;
    assert_eq!(d.recv_type("ack").await["data"]["ref"], off["id"]);
    // An inbound frame queued before the phone turned forwarding off is acked and dropped.
    let f = frame(
        "inbound",
        json!({"uid": "3b0368d22af9d295abcbe52c2ce7fdaa", "from": "+15551234567",
            "body": "hi", "sim": 1, "received_at": 1_760_000_000_000i64}),
    );
    d.send(&f).await;
    assert_eq!(d.recv_type("ack").await["data"]["ref"], f["id"]);
    // Its id is still recorded, so a replay is acked and does nothing.
    d.send(&f).await;
    assert_eq!(d.recv_type("ack").await["data"]["ref"], f["id"]);
    assert_eq!(count(&s, "SELECT count(*) FROM seen_frames").await, 2);
    assert_eq!(
        count(&s, "SELECT count(*) FROM messages WHERE direction='in'").await,
        0
    );
    assert_eq!(
        count(&s, "SELECT count(*) FROM webhook_deliveries").await,
        0
    );
    s.stop().await;
}

#[tokio::test]
async fn ac_002_15_status_at_is_clamped_and_parts_checked() {
    let s = server().await;
    let key = s.api_key().await;
    let (_, token) = s.pair("A").await;
    let mut d = Device::connect(&s, &token).await;
    let (_, v) = s
        .api_post(
            &key,
            "/api/v1/messages",
            json!({"to": ["+15550001111"], "body": "x"}),
        )
        .await;
    let msg = v["messages"][0]["id"].as_str().unwrap().to_string();
    let send = d.recv_type("send").await;
    d.ack(&send["id"]).await;
    // `parts` below 1 is a bad frame and changes nothing.
    let bad = frame(
        "status",
        json!({"msg_id": msg, "state": "sent", "at": 1, "parts": 0, "error": null}),
    );
    d.send(&bad).await;
    let n = d.recv_type("nack").await;
    assert_eq!(n["data"]["ref"], bad["id"]);
    assert_eq!(n["data"]["code"], "bad_frame");
    assert_eq!(s.message_state(&key, &msg).await, "dispatched");
    // A time before dispatch becomes the dispatch time.
    let f = frame(
        "status",
        json!({"msg_id": msg, "state": "sent", "at": 1, "parts": 1, "error": null}),
    );
    d.send(&f).await;
    d.recv_type("ack").await;
    // A time in the future becomes now.
    let before = s.app().clock.now_ms();
    let f = frame(
        "status",
        json!({"msg_id": msg, "state": "delivered", "at": i64::MAX, "parts": 1, "error": null}),
    );
    d.send(&f).await;
    d.recv_type("ack").await;
    let after = s.app().clock.now_ms();
    let pid = msg.clone();
    let m = s
        .app()
        .store
        .read(move |c| rustybee::store::messages::by_public(c, &pid))
        .await
        .unwrap()
        .unwrap();
    assert!(m.dispatched_at.is_some());
    assert_eq!(m.sent_at, m.dispatched_at);
    let at = m.delivered_at.unwrap();
    assert!(before <= at && at <= after, "{before} <= {at} <= {after}");
    s.stop().await;
}

#[tokio::test]
async fn ac_004_08_expired_message_takes_late_sent() {
    let s = server().await;
    let key = s.api_key().await;
    let (_, token) = s.pair("A").await;
    sink_webhook(
        &s,
        &["message.expired", "message.sent", "message.delivered"],
    )
    .await;
    let mut d = Device::connect(&s, &token).await;
    let (_, v) = s
        .api_post(
            &key,
            "/api/v1/messages",
            json!({"to": ["+15550001111"], "body": "x"}),
        )
        .await;
    let msg = v["messages"][0]["id"].as_str().unwrap().to_string();
    // The `send` arrives but its ack is lost, so the TTL expires it.
    d.recv_type("send").await;
    s.app().clock.advance_ms(25 * 3600 * 1000);
    rustybee::sweeper::sweep_once(s.app()).await.unwrap();
    assert_eq!(s.message_state(&key, &msg).await, "expired");
    // The phone sent it anyway.
    d.status(&msg, "sent").await;
    assert_eq!(s.message_state(&key, &msg).await, "sent");
    d.status(&msg, "delivered").await;
    assert_eq!(s.message_state(&key, &msg).await, "delivered");
    for ev in ["message.expired", "message.sent", "message.delivered"] {
        let n = s
            .app()
            .store
            .read(move |c| {
                c.query_row(
                    "SELECT count(*) FROM webhook_deliveries WHERE event=?1",
                    [ev],
                    |r| r.get::<_, i64>(0),
                )
            })
            .await
            .unwrap();
        assert_eq!(n, 1, "{ev}");
    }
    s.stop().await;
}

async fn wake_row(s: &Server, id: i64) -> devices::DeviceRow {
    s.app()
        .store
        .read(move |c| devices::by_id(c, id))
        .await
        .unwrap()
        .unwrap()
}

async fn dirty_wake(s: &Server, id: i64) {
    s.app()
        .store
        .write(move |c| {
            c.execute(
                "UPDATE devices SET wake_attempts=4, unreachable_at=11, gateway_off_at=22 WHERE id=?1",
                [id],
            )?;
            Ok(())
        })
        .await
        .unwrap();
}

async fn wait_down(s: &Server, id: i64) -> bool {
    eventually(|| async {
        !s.app()
            .store
            .read(move |c| devices::by_id(c, id))
            .await
            .unwrap()
            .unwrap()
            .connected
    })
    .await
}

#[tokio::test]
async fn ac_002_16_hello_lifecycle_clears_wake_state() {
    let s = server().await;
    let (pid, token) = s.pair("A").await;
    let id = internal_id(&s, &pid).await;
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("../../protocol/fixtures/hello.json")).unwrap();
    dirty_wake(&s, id).await;
    let d = Device::connect(&s, &token).await;
    let row = wake_row(&s, id).await;
    assert_eq!(row.wake_attempts, 0);
    assert!(row.unreachable_at.is_none());
    assert!(row.gateway_off_at.is_none());
    assert!(row.connected);
    let stored: serde_json::Value =
        serde_json::from_str(row.lifecycle_json.as_deref().unwrap()).unwrap();
    assert_eq!(stored, fixture["data"]["lifecycle"]);
    d.close().await;
    assert!(wait_down(&s, id).await);

    // Malformed lifecycle is ignored: acked, connected, previous lifecycle kept.
    dirty_wake(&s, id).await;
    let mut hello = hello_frame();
    hello["data"]["lifecycle"] = json!({"start": "nope"});
    let mut d = Device {
        ws: connect_raw(&s.ws_url(), &token).await.unwrap(),
    };
    d.send(&hello).await;
    assert_eq!(d.recv_type("ack").await["data"]["ref"], hello["id"]);
    d.recv_type("welcome").await;
    let row = wake_row(&s, id).await;
    assert_eq!(row.wake_attempts, 0);
    assert!(row.unreachable_at.is_none());
    assert!(row.gateway_off_at.is_none());
    assert!(row.connected);
    let stored: serde_json::Value =
        serde_json::from_str(row.lifecycle_json.as_deref().unwrap()).unwrap();
    assert_eq!(stored, fixture["data"]["lifecycle"]);
    d.close().await;
    assert!(wait_down(&s, id).await);

    // No lifecycle field at all: same, and the stored one stays.
    dirty_wake(&s, id).await;
    let mut hello = hello_frame();
    hello["data"].as_object_mut().unwrap().remove("lifecycle");
    let mut d = Device {
        ws: connect_raw(&s.ws_url(), &token).await.unwrap(),
    };
    d.send(&hello).await;
    assert_eq!(d.recv_type("ack").await["data"]["ref"], hello["id"]);
    d.recv_type("welcome").await;
    let row = wake_row(&s, id).await;
    assert_eq!(row.wake_attempts, 0);
    assert!(row.unreachable_at.is_none());
    assert!(row.gateway_off_at.is_none());
    let stored: serde_json::Value =
        serde_json::from_str(row.lifecycle_json.as_deref().unwrap()).unwrap();
    assert_eq!(stored, fixture["data"]["lifecycle"]);
    s.stop().await;
}

#[tokio::test]
async fn ac_002_17_gateway_off_close_and_token_update() {
    let s = server().await;
    let (pid, token) = s.pair("A").await;
    let id = internal_id(&s, &pid).await;
    s.app()
        .store
        .write(move |c| {
            c.execute("UPDATE devices SET fcm_token='old-token' WHERE id=?1", [id])?;
            Ok(())
        })
        .await
        .unwrap();
    let mut d = Device::connect(&s, &token).await;
    // The fixture hello carries a null token, which replaces the seeded one.
    assert!(wake_row(&s, id).await.fcm_token.is_none());
    let h = frame("health", json!({"outbox": 1, "fcm_token": "tok-new"}));
    d.send(&h).await;
    assert_eq!(d.recv_type("ack").await["data"]["ref"], h["id"]);
    assert_eq!(wake_row(&s, id).await.fcm_token.as_deref(), Some("tok-new"));
    let h = frame("health", json!({"outbox": 2}));
    d.send(&h).await;
    d.recv_type("ack").await;
    assert_eq!(wake_row(&s, id).await.fcm_token.as_deref(), Some("tok-new"));
    d.close().await;
    assert!(wait_down(&s, id).await);
    assert!(wake_row(&s, id).await.gateway_off_at.is_none());

    let mut d = Device::connect(&s, &token).await;
    let _ =
        d.ws.send(tungstenite::Message::Close(Some(CloseFrame {
            code: 4010.into(),
            reason: "gateway_off".into(),
        })))
        .await;
    let flagged = eventually(|| async {
        s.app()
            .store
            .read(move |c| devices::by_id(c, id))
            .await
            .unwrap()
            .unwrap()
            .gateway_off_at
            .is_some()
    })
    .await;
    assert!(flagged);
    assert!(!wake_row(&s, id).await.connected);
    // The app does not open another socket after 4010.
    s.stop().await;
}

async fn stored_radios(s: &Server, id: i64) -> serde_json::Value {
    wake_row(s, id).await.telemetry["radios"].clone()
}

#[tokio::test]
async fn ac_015_04_radios_cleaned_on_ingest() {
    let s = server().await;
    let (pid, token) = s.pair("A").await;
    let id = internal_id(&s, &pid).await;
    let mut d = Device::connect(&s, &token).await;
    let long = "x".repeat(500);
    let h = frame(
        "health",
        json!({"radios": {
            "cellular": {"name": format!("  {long}  "), "level": 9, "dbm": 20, "tech": "6G"},
            "wifi": {"name": "   ", "level": -3, "dbm": -60, "tech": "4G"},
        }}),
    );
    d.send(&h).await;
    d.recv_type("ack").await;
    assert_eq!(
        stored_radios(&s, id).await,
        json!({
            "cellular": {"name": "x".repeat(64), "level": 4},
            "wifi": {"level": 0, "dbm": -60},
        })
    );
    // Malformed radios count as absent: the health is still acked and what was stored stays.
    let before = stored_radios(&s, id).await;
    let h = frame(
        "health",
        json!({"outbox": 0, "radios": {"wifi": {"level": 2.5, "dbm": "loud"}}}),
    );
    d.send(&h).await;
    d.recv_type("ack").await;
    assert_eq!(stored_radios(&s, id).await, before);
    d.close().await;
    s.stop().await;
}

#[tokio::test]
async fn ac_015_02_radios_stored_replaced_kept_and_cleared() {
    let s = server().await;
    let (pid, token) = s.pair("A").await;
    let id = internal_id(&s, &pid).await;
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("../../protocol/fixtures/hello.json")).unwrap();

    // A hello with radios stores them.
    let mut d = Device::connect(&s, &token).await;
    assert_eq!(stored_radios(&s, id).await, fixture["data"]["radios"]);

    // A health with radios replaces them.
    let cell = json!({"cellular": {"name": "Other", "level": 1, "dbm": -110}});
    let h = frame("health", json!({"radios": cell}));
    d.send(&h).await;
    assert_eq!(d.recv_type("ack").await["data"]["ref"], h["id"]);
    assert_eq!(stored_radios(&s, id).await, cell);

    // A health without radios keeps them.
    let h = frame("health", json!({"outbox": 3}));
    d.send(&h).await;
    d.recv_type("ack").await;
    assert_eq!(stored_radios(&s, id).await, cell);

    // `{}` replaces them too.
    let h = frame("health", json!({"radios": {}}));
    d.send(&h).await;
    d.recv_type("ack").await;
    assert_eq!(stored_radios(&s, id).await, json!({}));
    d.close().await;
    assert!(wait_down(&s, id).await);

    // A hello without radios clears them.
    let mut hello = hello_frame();
    hello["data"].as_object_mut().unwrap().remove("radios");
    let mut d = Device {
        ws: connect_raw(&s.ws_url(), &token).await.unwrap(),
    };
    d.send(&hello).await;
    d.recv_type("ack").await;
    d.recv_type("welcome").await;
    let row = wake_row(&s, id).await;
    assert!(row.telemetry.get("radios").is_none());
    assert!(row.radios_json().is_null());
    s.stop().await;
}
