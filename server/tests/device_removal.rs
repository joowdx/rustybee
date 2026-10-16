//! Removing a device is a soft delete (specs 003, 004).

mod common;

use common::*;
use rustybee::store::{devices, messages};
use serde_json::json;
use tokio_tungstenite::tungstenite;

#[tokio::test]
async fn ac_003_ac_004_removing_device_keeps_history_and_cancels_pending() {
    let s = server().await;
    let key = s.api_key().await;
    let (a, token) = s.pair("A").await;
    let (b, _) = s.pair("B").await;
    s.app()
        .store
        .write(|c| c.execute("UPDATE devices SET send_delay_ms=0", []))
        .await
        .unwrap();
    let to: Vec<String> = (0..3).map(|i| format!("+1555000{i:04}")).collect();
    let (st, v) = s
        .api_post(
            &key,
            "/api/v1/messages",
            json!({"to": to, "body": "hi", "device_id": a}),
        )
        .await;
    assert_eq!(st, 202, "{v}");
    let ids: Vec<String> = v["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["id"].as_str().unwrap().to_string())
        .collect();

    // First delivered, second dispatched and acked, third still queued.
    let mut d = Device::connect(&s, &token).await;
    let f = d.recv_type("send").await;
    assert_eq!(f["data"]["msg_id"], ids[0].as_str());
    d.ack(&f["id"]).await;
    d.status(&ids[0], "delivered").await;
    let f = d.recv_type("send").await;
    assert_eq!(f["data"]["msg_id"], ids[1].as_str());
    d.ack(&f["id"]).await;
    let (k, id1) = (key.clone(), ids[1].clone());
    let sref = &s;
    assert!(
        eventually(|| {
            let (k, id1) = (k.clone(), id1.clone());
            async move { sref.message_state(&k, &id1).await == "dispatched" }
        })
        .await
    );
    assert_eq!(s.message_state(&key, &ids[2]).await, "queued");

    let pa = a.clone();
    let internal = s
        .app()
        .store
        .read(move |c| devices::by_public(c, &pa))
        .await
        .unwrap()
        .unwrap()
        .id;
    s.app().remove_device(internal).await.unwrap();

    // Socket closed with 4003; the old token is rejected with 401.
    assert_eq!(d.close_code(2000).await, Some(4003));
    match connect_raw(&s.ws_url(), &token).await {
        Err(tungstenite::Error::Http(resp)) => assert_eq!(resp.status(), 401),
        other => panic!("expected 401, got {:?}", other.map(|_| ())),
    }

    // History is kept: delivered stays, queued and dispatched fail as cancelled.
    let (st, page) = s.api_get(&key, "/api/v1/messages?limit=50").await;
    assert_eq!(st, 200);
    let items = page["items"].as_array().unwrap();
    let find = |id: &str| {
        items
            .iter()
            .find(|m| m["id"] == id)
            .unwrap_or_else(|| panic!("{id} missing from history"))
            .clone()
    };
    assert_eq!(find(&ids[0])["state"], "delivered");
    for id in &ids[1..] {
        let m = find(id);
        assert_eq!(m["state"], "failed", "{m}");
        assert_eq!(m["error"]["code"], "cancelled", "{m}");
        assert_eq!(m["device_id"], a.as_str());
    }
    let id0 = ids[0].clone();
    let row = s
        .app()
        .store
        .read(move |c| messages::by_public(c, &id0))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.device_label(), "A (removed)");

    // Hidden from the device list; B does not become the default.
    let (st, list) = s.api_get(&key, "/api/v1/devices").await;
    assert_eq!(st, 200);
    let list = list.as_array().unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["id"], b.as_str());
    assert_eq!(list[0]["is_default"], false);

    // An explicit removed device is unknown; otherwise the only enabled device is used.
    let (st, _) = s
        .api_post(
            &key,
            "/api/v1/messages",
            json!({"to": ["+15550001111"], "body": "x", "device_id": a}),
        )
        .await;
    assert_eq!(st, 404);
    let (st, v) = s
        .api_post(
            &key,
            "/api/v1/messages",
            json!({"to": ["+15550001111"], "body": "x"}),
        )
        .await;
    assert_eq!(st, 202);
    assert_eq!(v["messages"][0]["device_id"], b.as_str());
    s.stop().await;
}
