//! Dispatch, pacing, device choice, replay and the TTL sweeper (specs 004, 006).

mod common;

use common::*;
use rustybee::domain::lifecycle::Event;
use rustybee::store::messages;
use serde_json::json;
use std::time::{Duration, Instant};

async fn queue(s: &Server, key: &str, n: usize) -> Vec<String> {
    let to: Vec<String> = (0..n).map(|i| format!("+1555000{i:04}")).collect();
    let (st, v) = s
        .api_post(key, "/api/v1/messages", json!({"to": to, "body": "hello"}))
        .await;
    assert_eq!(st, 202, "{v}");
    v["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["id"].as_str().unwrap().to_string())
        .collect()
}

async fn set_delay(s: &Server, ms: i64) {
    s.app()
        .store
        .write(move |c| c.execute("UPDATE devices SET send_delay_ms=?1", [ms]))
        .await
        .unwrap();
}

#[tokio::test]
async fn ac_006_01_paced_in_order_one_in_flight() {
    let s = server().await;
    let key = s.api_key().await;
    let (_, token) = s.pair("A").await;
    set_delay(&s, 200).await;
    let ids = queue(&s, &key, 3).await;
    let mut d = Device::connect(&s, &token).await;
    let mut released: Option<Instant> = None;
    for id in &ids {
        let f = d.recv_type("send").await;
        if let Some(t) = released {
            assert!(
                t.elapsed() >= Duration::from_millis(190),
                "paced: {:?}",
                t.elapsed()
            );
        }
        assert_eq!(f["data"]["msg_id"], id.as_str(), "in order");
        d.ack(&f["id"]).await;
        // Nothing else is sent while this one is in flight.
        assert!(d.recv_within(300).await.is_none(), "window of one");
        d.status(id, "sent").await;
        released = Some(Instant::now());
    }
    for id in &ids {
        assert_eq!(s.message_state(&key, id).await, "sent");
    }
    s.stop().await;
}

#[tokio::test]
async fn ac_006_02_inflight_timeout_releases_window() {
    let s = server_with(|c| c.inflight_timeout = Duration::from_millis(500)).await;
    let key = s.api_key().await;
    let (_, token) = s.pair("A").await;
    set_delay(&s, 0).await;
    let ids = queue(&s, &key, 2).await;
    let mut d = Device::connect(&s, &token).await;
    let first = d.recv_type("send").await;
    assert_eq!(first["data"]["msg_id"], ids[0].as_str());
    let t = Instant::now();
    d.ack(&first["id"]).await;
    // No status: the next one still comes after the timeout.
    let second = d.recv_type("send").await;
    assert_eq!(second["data"]["msg_id"], ids[1].as_str());
    assert!(
        t.elapsed() >= Duration::from_millis(450),
        "{:?}",
        t.elapsed()
    );
    s.stop().await;
}

#[tokio::test]
async fn ac_006_03_reconnect_before_ack_resends_same_frame() {
    let s = server().await;
    let key = s.api_key().await;
    let (_, token) = s.pair("A").await;
    let ids = queue(&s, &key, 1).await;
    let mut d = Device::connect(&s, &token).await;
    let f = d.recv_type("send").await;
    d.close().await;
    let mut d = Device::connect(&s, &token).await;
    let g = d.recv_type("send").await;
    assert_eq!(g["id"], f["id"]);
    assert_eq!(g["data"]["msg_id"], ids[0].as_str());
    s.stop().await;
}

#[tokio::test]
async fn ac_006_04_queued_offline_sent_soon_after_hello() {
    let s = server().await;
    let key = s.api_key().await;
    let (_, token) = s.pair("A").await;
    let ids = queue(&s, &key, 1).await;
    let t = Instant::now();
    let mut d = Device::connect(&s, &token).await;
    let f = d.recv_type("send").await;
    assert!(t.elapsed() < Duration::from_secs(1));
    assert_eq!(f["data"]["msg_id"], ids[0].as_str());
    s.stop().await;
}

#[tokio::test]
async fn ac_006_05_device_choice_order() {
    let s = server().await;
    let key = s.api_key().await;
    let (a, _) = s.pair("A").await;
    let (b, _) = s.pair("B").await;
    let send = |dev: Option<&str>| {
        let mut body = json!({"to": ["+15550001111"], "body": "x"});
        if let Some(d) = dev {
            body["device_id"] = json!(d);
        }
        body
    };
    // The first paired device is the default.
    let (st, v) = s.api_post(&key, "/api/v1/messages", send(None)).await;
    assert_eq!(st, 202);
    assert_eq!(v["messages"][0]["device_id"], a.as_str());
    // Explicit wins.
    let (_, v) = s.api_post(&key, "/api/v1/messages", send(Some(&b))).await;
    assert_eq!(v["messages"][0]["device_id"], b.as_str());
    // Unknown → 404.
    let (st, _) = s
        .api_post(&key, "/api/v1/messages", send(Some("nope")))
        .await;
    assert_eq!(st, 404);
    // No default and two enabled → 400 device_required.
    s.app()
        .store
        .write(|c| c.execute("UPDATE devices SET is_default=0", []))
        .await
        .unwrap();
    let (st, v) = s.api_post(&key, "/api/v1/messages", send(None)).await;
    assert_eq!(st, 400);
    assert_eq!(v["error"]["code"], "device_required");
    // Only one enabled → that one.
    let ap = a.clone();
    s.app()
        .store
        .write(move |c| c.execute("UPDATE devices SET enabled=0 WHERE public_id=?1", [ap]))
        .await
        .unwrap();
    let (st, v) = s.api_post(&key, "/api/v1/messages", send(None)).await;
    assert_eq!(st, 202);
    assert_eq!(v["messages"][0]["device_id"], b.as_str());
    // Explicit disabled → 409.
    let (st, v) = s.api_post(&key, "/api/v1/messages", send(Some(&a))).await;
    assert_eq!(st, 409);
    assert_eq!(v["error"]["code"], "device_disabled");
    s.stop().await;
}

#[tokio::test]
async fn ac_006_06_restart_replays_unacked_dispatched() {
    let s = server().await;
    let key = s.api_key().await;
    let (_, token) = s.pair("A").await;
    let ids = queue(&s, &key, 1).await;
    let mut d = Device::connect(&s, &token).await;
    let f = d.recv_type("send").await;
    let s = s.restart(|_| {}).await;
    assert!(d.recv_within(500).await.is_none(), "old socket closed");
    let mut d = Device::connect(&s, &token).await;
    let g = d.recv_type("send").await;
    assert_eq!(g["id"], f["id"]);
    assert_eq!(g["data"]["msg_id"], ids[0].as_str());
    assert_eq!(s.message_state(&key, &ids[0]).await, "dispatched");
    s.stop().await;
}

#[tokio::test]
async fn ac_004_02_same_status_twice_one_timestamp() {
    let s = server().await;
    let key = s.api_key().await;
    s.pair("A").await;
    let ids = queue(&s, &key, 1).await;
    let pid = ids[0].clone();
    let (first, second) = s
        .app()
        .store
        .write(move |c| {
            let m = messages::by_public(c, &pid)?.unwrap();
            messages::apply_event(c, m.id, Event::Dispatch, 1, None, None, 1)?;
            let a = messages::apply_event(c, m.id, Event::StatusSent, 100, Some(1), None, 100)?;
            let b = messages::apply_event(c, m.id, Event::StatusSent, 200, Some(1), None, 200)?;
            Ok((a, b))
        })
        .await
        .unwrap();
    assert_eq!(first.unwrap().sent_at, Some(100));
    assert!(second.is_none(), "second status is a no-op");
    s.stop().await;
}

#[tokio::test]
async fn ac_004_04_sweeper_expires_and_times_out() {
    let s = server().await;
    let key = s.api_key().await;
    s.pair("A").await;
    let ids = queue(&s, &key, 3).await;
    // ids[0] stays queued; ids[1] dispatched unacked; ids[2] dispatched and acked.
    let (b, c2) = (ids[1].clone(), ids[2].clone());
    s.app()
        .store
        .write(move |c| {
            for (pid, acked) in [(b, false), (c2, true)] {
                let m = messages::by_public(c, &pid)?.unwrap();
                messages::apply_event(
                    c,
                    m.id,
                    Event::Dispatch,
                    m.created_at,
                    None,
                    None,
                    m.created_at,
                )?;
                if acked {
                    c.execute(
                        "UPDATE messages SET device_acked_at=created_at WHERE id=?1",
                        [m.id],
                    )?;
                }
            }
            Ok(())
        })
        .await
        .unwrap();
    // Within the TTL nothing changes.
    assert!(
        rustybee::sweeper::sweep_once(s.app())
            .await
            .unwrap()
            .is_empty()
    );
    s.app().clock.advance_ms(25 * 3600 * 1000);
    let changed = rustybee::sweeper::sweep_once(s.app()).await.unwrap();
    assert_eq!(changed.len(), 3);
    assert_eq!(s.message_state(&key, &ids[0]).await, "expired");
    assert_eq!(s.message_state(&key, &ids[1]).await, "expired");
    assert_eq!(s.message_state(&key, &ids[2]).await, "failed");
    let (_, m) = s
        .api_get(&key, &format!("/api/v1/messages/{}", ids[2]))
        .await;
    assert_eq!(m["error"]["code"], "status_timeout");
    s.stop().await;
}

async fn acked_at(s: &Server, id: &str) -> Option<i64> {
    let id = id.to_string();
    s.app()
        .store
        .read(move |c| messages::by_public(c, &id))
        .await
        .unwrap()
        .and_then(|m| m.device_acked_at)
}

#[tokio::test]
async fn ac_006_04_stale_inflight_row_does_not_delay_reconnect() {
    // A long in-flight timeout: the old pacer seeding would wait all of it.
    let s = server_with(|c| c.inflight_timeout = Duration::from_secs(30)).await;
    let key = s.api_key().await;
    let (_, token) = s.pair("A").await;
    set_delay(&s, 0).await;
    let first = queue(&s, &key, 1).await.remove(0);
    let mut d = Device::connect(&s, &token).await;
    let f = d.recv_type("send").await;
    d.ack(&f["id"]).await;
    assert!(eventually(|| async { acked_at(&s, &first).await.is_some() }).await);
    d.close().await;
    // The acked row never got a status, and its timeout ran out while offline.
    s.app().clock.advance_ms(60_000);
    let dev_id: i64 = s
        .app()
        .store
        .read(|c| c.query_row("SELECT id FROM devices", [], |r| r.get(0)))
        .await
        .unwrap();
    assert!(eventually(|| async { !s.app().hub.is_connected(dev_id) }).await);
    let second = queue(&s, &key, 1).await.remove(0);
    let mut d = Device::connect(&s, &token).await;
    let t = Instant::now();
    let f = d.recv_type("send").await;
    assert_eq!(f["data"]["msg_id"], second.as_str());
    assert!(t.elapsed() < Duration::from_secs(1), "{:?}", t.elapsed());
    s.stop().await;
}

#[tokio::test]
async fn ac_006_07_dispatcher_survives_a_store_error() {
    let s = server().await;
    let key = s.api_key().await;
    let (_, token) = s.pair("A").await;
    set_delay(&s, 0).await;
    let mut d = Device::connect(&s, &token).await;
    let rename = |from: &'static str, to: &'static str| {
        s.app()
            .store
            .with_writer(move |c| c.execute_batch(&format!("ALTER TABLE {from} RENAME TO {to}")))
    };
    // Break the messages table and wake the dispatcher so its query fails.
    rename("messages", "messages_gone").await.unwrap();
    let dev_id: i64 = s
        .app()
        .store
        .read(|c| c.query_row("SELECT id FROM devices", [], |r| r.get(0)))
        .await
        .unwrap();
    s.app().hub.notify(dev_id);
    tokio::time::sleep(Duration::from_millis(400)).await;
    rename("messages_gone", "messages").await.unwrap();
    // The same socket still gets new messages.
    let id = queue(&s, &key, 1).await.remove(0);
    let f = d.recv_type("send").await;
    assert_eq!(f["data"]["msg_id"], id.as_str());
    s.stop().await;
}
