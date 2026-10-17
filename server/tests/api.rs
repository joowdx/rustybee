//! Public API `/api/v1` (spec 007).

mod common;

use common::*;
use rustybee::domain::lifecycle::Event;
use rustybee::store::messages;
use serde_json::{Value, json};

async fn count_messages(s: &Server) -> i64 {
    s.app()
        .store
        .read(|c| c.query_row("SELECT count(*) FROM messages", [], |r| r.get(0)))
        .await
        .unwrap()
}

#[tokio::test]
async fn ac_007_01_send_three_recipients() {
    let s = server().await;
    let key = s.api_key().await;
    let (_, token) = s.pair("A").await;
    let mut d = Device::connect(&s, &token).await;
    let (st, v) = s
        .api_post(
            &key,
            "/api/v1/messages",
            json!({"to": ["+15550000001", "+1 (555) 000-0002", "+15550000003"], "body": "hello"}),
        )
        .await;
    assert_eq!(st, 202);
    let batch = v["batch_id"].as_str().unwrap();
    let msgs = v["messages"].as_array().unwrap();
    assert_eq!(msgs.len(), 3);
    for m in msgs {
        assert_eq!(m["state"], "queued");
        assert_eq!(m["batch_id"], batch);
        assert_eq!(m["direction"], "out");
    }
    assert_eq!(msgs[1]["peer"], "+15550000002");
    // The dispatcher was woken.
    let f = d.recv_type("send").await;
    assert_eq!(f["data"]["msg_id"], msgs[0]["id"]);
    s.stop().await;
}

#[tokio::test]
async fn ac_007_02_invalid_recipient_writes_nothing() {
    let s = server().await;
    let key = s.api_key().await;
    s.pair("A").await;
    let (st, v) = s
        .api_post(
            &key,
            "/api/v1/messages",
            json!({"to": ["+15550000001", "abc"], "body": "hello"}),
        )
        .await;
    assert_eq!(st, 400);
    assert_eq!(v["error"]["code"], "invalid_recipient");
    assert!(v["error"]["message"].as_str().unwrap().contains("to[1]"));
    assert_eq!(count_messages(&s).await, 0);
    // Body limits and malformed JSON.
    let long = "x".repeat(1601);
    let (st, v) = s
        .api_post(
            &key,
            "/api/v1/messages",
            json!({"to": ["+15550000001"], "body": long}),
        )
        .await;
    assert_eq!(st, 400);
    assert_eq!(v["error"]["code"], "body_too_long");
    let r = s
        .http
        .post(s.url("/api/v1/messages"))
        .bearer_auth(&key)
        .header("content-type", "application/json")
        .body("{nope")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 400);
    // Batch items report their index too.
    let (st, v) = s
        .api_post(
            &key,
            "/api/v1/batches",
            json!({"items": [{"to": "+15550000001", "body": "a"}, {"to": "x", "body": "b"}]}),
        )
        .await;
    assert_eq!(st, 400);
    assert!(v["error"]["message"].as_str().unwrap().contains("items[1]"));
    assert_eq!(count_messages(&s).await, 0);
    s.stop().await;
}

#[tokio::test]
async fn ac_007_03_cursor_pages_every_row_once() {
    let s = server().await;
    let key = s.api_key().await;
    s.pair("A").await;
    let to: Vec<String> = (0..5).map(|i| format!("+155500000{i:02}")).collect();
    let (_, v) = s
        .api_post(&key, "/api/v1/messages", json!({"to": to, "body": "x"}))
        .await;
    let mut want: Vec<String> = v["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["id"].as_str().unwrap().to_string())
        .collect();
    let mut seen = vec![];
    let mut cursor: Option<String> = None;
    loop {
        let path = match &cursor {
            Some(c) => format!("/api/v1/messages?limit=2&cursor={c}"),
            None => "/api/v1/messages?limit=2".into(),
        };
        let (st, page) = s.api_get(&key, &path).await;
        assert_eq!(st, 200);
        let items = page["items"].as_array().unwrap();
        assert!(items.len() <= 2);
        seen.extend(items.iter().map(|m| m["id"].as_str().unwrap().to_string()));
        match page["next_cursor"].as_str() {
            Some(c) => cursor = Some(c.to_string()),
            None => break,
        }
    }
    want.reverse(); // newest first
    assert_eq!(seen, want);
    let (st, _) = s.api_get(&key, "/api/v1/messages?limit=0").await;
    assert_eq!(st, 400);
    let (st, _) = s.api_get(&key, "/api/v1/messages?cursor=!!").await;
    assert_eq!(st, 400);
    s.stop().await;
}

fn ids(v: &Value) -> Vec<String> {
    v["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["id"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn ac_007_04_filters_narrow_results() {
    let s = server().await;
    let key = s.api_key().await;
    let (_, token) = s.pair("A").await;
    let t0 = s.app().clock.now_ms();
    let (_, a) = s
        .api_post(
            &key,
            "/api/v1/messages",
            json!({"to": ["+15550000001"], "body": "Apple pie"}),
        )
        .await;
    let a = a["messages"][0]["id"].as_str().unwrap().to_string();
    s.app().clock.advance_ms(3_600_000);
    let (_, b) = s
        .api_post(
            &key,
            "/api/v1/messages",
            json!({"to": ["+15550000002"], "body": "banana"}),
        )
        .await;
    let b = b["messages"][0]["id"].as_str().unwrap().to_string();
    // Mark b sent.
    let bp = b.clone();
    s.app()
        .store
        .write(move |c| {
            let m = messages::by_public(c, &bp)?.unwrap();
            messages::apply_event(c, m.id, Event::Dispatch, 1, None, None, 1)?;
            messages::apply_event(c, m.id, Event::StatusSent, 2, Some(1), None, 2)?;
            Ok(())
        })
        .await
        .unwrap();
    // One inbound message.
    let mut d = Device::connect(&s, &token).await;
    let f = frame(
        "inbound",
        json!({"uid": "u1", "from": "+15550000009", "body": "Cherry", "sim": null, "received_at": 1}),
    );
    d.send(&f).await;
    // The dispatcher may push the queued "Apple pie" send before this ack.
    loop {
        let v = d.recv().await;
        if v["type"] == "ack" && v["data"]["ref"] == f["id"] {
            break;
        }
        assert!(
            v["type"] == "send" || v["type"] == "config",
            "unexpected {v}"
        );
    }
    let get = |q: &str| {
        let q = q.to_string();
        let s = &s;
        let key = key.clone();
        async move {
            let (st, v) = s.api_get(&key, &format!("/api/v1/messages?{q}")).await;
            assert_eq!(st, 200, "{q}: {v}");
            ids(&v)
        }
    };
    assert_eq!(get("direction=out").await.len(), 2);
    assert_eq!(get("direction=in").await.len(), 1);
    assert_eq!(get("state=sent").await, vec![b.clone()]);
    // `a` may have been dispatched to the connected device by now.
    let a_state = s.message_state(&key, &a).await;
    assert_eq!(get(&format!("state={a_state}")).await, vec![a.clone()]);
    assert_eq!(get("peer=%2B15550000001").await, vec![a.clone()]);
    assert_eq!(get("q=APPLE").await, vec![a.clone()]);
    let mid = rustybee::util::rfc3339(t0 + 1_800_000);
    assert_eq!(get(&format!("until={mid}")).await, vec![a.clone()]);
    let since = get(&format!("since={mid}")).await;
    assert!(since.contains(&b) && !since.contains(&a));
    let (st, _) = s.api_get(&key, "/api/v1/messages?state=bogus").await;
    assert_eq!(st, 400);
    let (st, _) = s.api_get(&key, "/api/v1/messages?since=yesterday").await;
    assert_eq!(st, 400);
    s.stop().await;
}

#[tokio::test]
async fn ac_007_05_auth_and_health() {
    let s = server().await;
    let r = s.http.get(s.url("/api/v1/health")).send().await.unwrap();
    assert_eq!(r.status(), 200);
    let v: Value = r.json().await.unwrap();
    assert_eq!(v["ok"], true);
    for path in ["/api/v1/messages", "/api/v1/devices"] {
        let r = s.http.get(s.url(path)).send().await.unwrap();
        assert_eq!(r.status(), 401);
        let v: Value = r.json().await.unwrap();
        assert_eq!(v["error"]["code"], "unauthorized");
    }
    let key = s.api_key().await;
    let (st, v) = s.api_get(&key, "/api/v1/nothing-here").await;
    assert_eq!(st, 404);
    assert_eq!(v["error"]["code"], "not_found");
    let (st, v) = s.api_get(&key, "/api/v1/devices").await;
    assert_eq!(st, 200);
    assert!(v.as_array().unwrap().is_empty());
    s.stop().await;
}

#[tokio::test]
async fn ac_007_06_batch_counts_and_status() {
    let s = server().await;
    let key = s.api_key().await;
    s.pair("A").await;
    let (st, v) = s
        .api_post(
            &key,
            "/api/v1/batches",
            json!({"items": [{"to": "+15550000001", "body": "a"}, {"to": "+15550000002", "body": "b"}]}),
        )
        .await;
    assert_eq!(st, 202);
    let batch = v["batch_id"].as_str().unwrap().to_string();
    let ids: Vec<String> = v["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["id"].as_str().unwrap().to_string())
        .collect();
    let (_, b) = s.api_get(&key, &format!("/api/v1/batches/{batch}")).await;
    assert_eq!(b["total"], 2);
    assert_eq!(b["counts"]["queued"], 2);
    assert_eq!(b["status"], "pending");
    let app = s.app().clone();
    let apply = |pid: String, events: Vec<Event>| {
        let app = app.clone();
        async move {
            app.store
                .write(move |c| {
                    let m = messages::by_public(c, &pid)?.unwrap();
                    for e in events {
                        messages::apply_event(c, m.id, e, 5, None, None, 5)?;
                    }
                    Ok(())
                })
                .await
                .unwrap();
        }
    };
    apply(
        ids[0].clone(),
        vec![Event::Dispatch, Event::StatusSent, Event::StatusDelivered],
    )
    .await;
    let (_, b) = s.api_get(&key, &format!("/api/v1/batches/{batch}")).await;
    assert_eq!(b["status"], "pending");
    apply(ids[1].clone(), vec![Event::Dispatch, Event::StatusFailed]).await;
    let (_, b) = s.api_get(&key, &format!("/api/v1/batches/{batch}")).await;
    assert_eq!(b["counts"]["delivered"], 1);
    assert_eq!(b["counts"]["failed"], 1);
    assert_eq!(b["status"], "partial");
    let (st, _) = s.api_get(&key, "/api/v1/batches/nope").await;
    assert_eq!(st, 404);
    s.stop().await;
}

fn send_n(n: usize) -> Value {
    let to: Vec<String> = (0..n).map(|i| format!("+155500000{i:02}")).collect();
    json!({"to": to, "body": "x"})
}

#[tokio::test]
async fn ac_007_07_queue_full_returns_429() {
    let s = server_with(|c| c.max_queued = 3).await;
    let key = s.api_key().await;
    s.pair("A").await;
    let (st, _) = s.api_post(&key, "/api/v1/messages", send_n(2)).await;
    assert_eq!(st, 202);
    let (st, v) = s.api_post(&key, "/api/v1/messages", send_n(2)).await;
    assert_eq!(st, 429);
    assert_eq!(v["error"]["code"], "queue_full");
    assert_eq!(count_messages(&s).await, 2);
    let (st, _) = s.api_post(&key, "/api/v1/messages", send_n(1)).await;
    assert_eq!(st, 202);
    assert_eq!(count_messages(&s).await, 3);
    s.stop().await;
}

#[tokio::test]
async fn ac_007_08_short_codes_need_opt_in() {
    let s = server().await;
    let key = s.api_key().await;
    s.pair("A").await;
    let (st, v) = s
        .api_post(
            &key,
            "/api/v1/messages",
            json!({"to": ["2336"], "body": "x"}),
        )
        .await;
    assert_eq!(st, 400);
    assert_eq!(v["error"]["code"], "invalid_recipient");
    s.stop().await;

    let s = server_with(|c| c.allow_short_codes = true).await;
    let key = s.api_key().await;
    s.pair("A").await;
    let (st, v) = s
        .api_post(
            &key,
            "/api/v1/messages",
            json!({"to": ["2336"], "body": "x"}),
        )
        .await;
    assert_eq!(st, 202);
    assert_eq!(v["messages"][0]["peer"], "2336");
    s.stop().await;
}

async fn post_keyed(
    s: &Server,
    api_key: &str,
    path: &str,
    idem: &str,
    body: &str,
) -> (u16, Option<String>, Value) {
    let r = s
        .http
        .post(s.url(path))
        .bearer_auth(api_key)
        .header("Idempotency-Key", idem)
        .header("Content-Type", "application/json")
        .body(body.to_string())
        .send()
        .await
        .unwrap();
    let st = r.status().as_u16();
    let replayed = r
        .headers()
        .get("idempotent-replayed")
        .map(|v| v.to_str().unwrap().to_string());
    (st, replayed, r.json().await.unwrap_or(Value::Null))
}

#[tokio::test]
async fn ac_007_09_idempotency_key_replays_the_original_response() {
    let s = server().await;
    let key = s.api_key().await;
    s.pair("A").await;
    let body = r#"{"to": ["+15550000001", "+15550000002"], "body": "hi"}"#;
    let (st, replayed, first) = post_keyed(&s, &key, "/api/v1/messages", "k-1", body).await;
    assert_eq!((st, replayed), (202, None), "{first}");
    // Same key, same request (different formatting): same ids, nothing new written.
    let same = r#"{"body":"hi","to":["+15550000001","+15550000002"]}"#;
    let (st, replayed, again) = post_keyed(&s, &key, "/api/v1/messages", "k-1", same).await;
    assert_eq!((st, replayed.as_deref()), (202, Some("true")));
    assert_eq!(again, first);
    assert_eq!(count_messages(&s).await, 2);
    // Same key, different request: 422 and nothing written.
    let other = r#"{"to": ["+15550000003"], "body": "hi"}"#;
    let (st, _, v) = post_keyed(&s, &key, "/api/v1/messages", "k-1", other).await;
    assert_eq!(st, 422);
    assert_eq!(v["error"]["code"], "idempotency_key_reused");
    // The same body on the other endpoint counts as a different request.
    let items = r#"{"items": [{"to": "+15550000001", "body": "hi"}]}"#;
    let (st, _, _) = post_keyed(&s, &key, "/api/v1/batches", "k-1", items).await;
    assert_eq!(st, 422);
    assert_eq!(count_messages(&s).await, 2);
    // Batches replay too.
    let (st, _, b1) = post_keyed(&s, &key, "/api/v1/batches", "k-2", items).await;
    assert_eq!(st, 202);
    let (st, replayed, b2) = post_keyed(&s, &key, "/api/v1/batches", "k-2", items).await;
    assert_eq!((st, replayed.as_deref()), (202, Some("true")));
    assert_eq!(b1["batch_id"], b2["batch_id"]);
    assert_eq!(count_messages(&s).await, 3);
    // A malformed key is refused.
    let (st, _, _) = post_keyed(&s, &key, "/api/v1/messages", &"x".repeat(256), body).await;
    assert_eq!(st, 400);
    // Without a key, every request queues.
    let (st, v) = s
        .api_post(
            &key,
            "/api/v1/messages",
            serde_json::from_str(body).unwrap(),
        )
        .await;
    assert_eq!(st, 202);
    assert_ne!(v["batch_id"], first["batch_id"]);
    s.stop().await;
}

#[tokio::test]
async fn ac_007_10_idempotency_key_expires_after_24_hours() {
    let s = server().await;
    let key = s.api_key().await;
    s.pair("A").await;
    let body = r#"{"to": ["+15550000001"], "body": "hi"}"#;
    let (_, _, first) = post_keyed(&s, &key, "/api/v1/messages", "k", body).await;
    s.app().clock.advance_ms(23 * 3600 * 1000);
    let (_, _, again) = post_keyed(&s, &key, "/api/v1/messages", "k", body).await;
    assert_eq!(again["batch_id"], first["batch_id"]);
    s.app().clock.advance_ms(3600 * 1000);
    let (st, replayed, later) = post_keyed(&s, &key, "/api/v1/messages", "k", body).await;
    assert_eq!((st, replayed), (202, None));
    assert_ne!(later["batch_id"], first["batch_id"]);
    // The daily prune forgets expired keys.
    s.app().clock.advance_ms(25 * 3600 * 1000);
    rustybee::sweeper::prune_once(s.app()).await.unwrap();
    let n: i64 = s
        .app()
        .store
        .read(|c| c.query_row("SELECT count(*) FROM idempotency_keys", [], |r| r.get(0)))
        .await
        .unwrap();
    assert_eq!(n, 0);
    s.stop().await;
}
