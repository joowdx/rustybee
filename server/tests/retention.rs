//! Opt-in retention in the daily prune (spec 003).

mod common;

use common::*;
use rustybee::domain::lifecycle::Event;
use rustybee::store::messages;
use serde_json::json;

const DAY_MS: i64 = 24 * 3600 * 1000;

/// Queue one message per event list and apply the events; returns the ids.
async fn seed(s: &Server, key: &str, plans: Vec<Vec<Event>>) -> Vec<String> {
    let to: Vec<String> = (0..plans.len())
        .map(|i| format!("+155500000{i:02}"))
        .collect();
    let (st, v) = s
        .api_post(key, "/api/v1/messages", json!({"to": to, "body": "x"}))
        .await;
    assert_eq!(st, 202, "{v}");
    let ids: Vec<String> = v["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["id"].as_str().unwrap().to_string())
        .collect();
    let pids = ids.clone();
    let now = s.app().clock.now_ms();
    s.app()
        .store
        .write(move |c| {
            for (pid, events) in pids.iter().zip(plans) {
                let m = messages::by_public(c, pid)?.unwrap();
                for e in events {
                    messages::apply_event(c, m.id, e, now, None, None, now)?;
                }
            }
            // One delivered, one aborted and one pending webhook delivery.
            c.execute(
                "INSERT INTO webhooks(public_id, name, url, events_json, secret, active, created_at)
                 VALUES('w','h','http://127.0.0.1:9/','[]','s',0,?1)",
                [now],
            )?;
            let w = c.last_insert_rowid();
            for (pid, delivered, aborted) in [
                ("d1", Some(now), None),
                ("d2", None, Some(now)),
                ("d3", None, None),
            ] {
                c.execute(
                    "INSERT INTO webhook_deliveries(public_id, webhook_id, event, payload, created_at,
                       delivered_at, aborted_at) VALUES(?1,?2,'message.sent','{}',?3,?4,?5)",
                    rusqlite::params![pid, w, now, delivered, aborted],
                )?;
            }
            Ok(())
        })
        .await
        .unwrap();
    ids
}

async fn remaining(s: &Server) -> (Vec<String>, Vec<String>, i64) {
    s.app()
        .store
        .read(|c| {
            let col = |sql: &str| -> rusqlite::Result<Vec<String>> {
                let mut st = c.prepare(sql)?;
                st.query_map([], |r| r.get(0))?.collect()
            };
            Ok((
                col("SELECT state FROM messages ORDER BY id")?,
                col("SELECT public_id FROM webhook_deliveries ORDER BY id")?,
                c.query_row("SELECT count(*) FROM batches", [], |r| r.get(0))?,
            ))
        })
        .await
        .unwrap()
}

fn plans() -> Vec<Vec<Event>> {
    vec![
        vec![Event::Dispatch, Event::StatusSent, Event::StatusDelivered],
        vec![Event::Dispatch, Event::StatusFailed],
        vec![Event::Dispatch, Event::StatusSent],
        vec![],
    ]
}

#[tokio::test]
async fn ac_003_05_retention_prunes_finished_rows() {
    let s = server_with(|c| c.retention_days = Some(7)).await;
    let key = s.api_key().await;
    s.pair("A").await;
    seed(&s, &key, plans()).await;
    // Younger than the retention period: nothing goes.
    s.app().clock.advance_ms(6 * DAY_MS);
    assert_eq!(
        rustybee::sweeper::prune_once(s.app()).await.unwrap(),
        (0, 0)
    );
    s.app().clock.advance_ms(2 * DAY_MS);
    assert_eq!(
        rustybee::sweeper::prune_once(s.app()).await.unwrap(),
        (2, 2)
    );
    let (states, deliveries, batches) = remaining(&s).await;
    assert_eq!(states, vec!["sent", "queued"]);
    assert_eq!(deliveries, vec!["d3"]);
    assert_eq!(batches, 1);
    s.stop().await;
}

#[tokio::test]
async fn ac_003_05_retention_drops_emptied_batches() {
    let s = server_with(|c| c.retention_days = Some(1)).await;
    let key = s.api_key().await;
    s.pair("A").await;
    seed(
        &s,
        &key,
        vec![vec![
            Event::Dispatch,
            Event::StatusSent,
            Event::StatusDelivered,
        ]],
    )
    .await;
    s.app().clock.advance_ms(2 * DAY_MS);
    assert_eq!(
        rustybee::sweeper::prune_once(s.app()).await.unwrap(),
        (1, 2)
    );
    let (states, _, batches) = remaining(&s).await;
    assert!(states.is_empty());
    assert_eq!(batches, 0);
    s.stop().await;
}

#[tokio::test]
async fn ac_003_06_no_retention_keeps_everything() {
    let s = server().await;
    let key = s.api_key().await;
    s.pair("A").await;
    seed(&s, &key, plans()).await;
    s.app().clock.advance_ms(3650 * DAY_MS);
    assert_eq!(
        rustybee::sweeper::prune_once(s.app()).await.unwrap(),
        (0, 0)
    );
    let (states, deliveries, batches) = remaining(&s).await;
    assert_eq!(states.len(), 4);
    assert_eq!(deliveries.len(), 3);
    assert_eq!(batches, 1);
    s.stop().await;
}

fn backups(dir: &std::path::Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    v.sort();
    v
}

#[tokio::test]
async fn ac_003_10_daily_backup_is_checked_and_rotated() {
    let backup_dir = tempfile::tempdir().unwrap().keep().join("backups");
    let bd = backup_dir.clone();
    let s = server_with(move |c| {
        c.backup_dir = Some(bd);
        c.backup_keep = 2;
    })
    .await;
    let key = s.api_key().await;
    s.pair("A").await;
    seed(&s, &key, vec![vec![]]).await;

    let first = rustybee::sweeper::backup_once(s.app())
        .await
        .unwrap()
        .expect("backup written");
    assert!(first.starts_with(&backup_dir));
    let c = rusqlite::Connection::open(&first).unwrap();
    let n: i64 = c
        .query_row("SELECT count(*) FROM messages", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 1);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&first), 0o600);
        assert_eq!(mode(&backup_dir), 0o700);
    }
    // Once per day: a second run the same day writes nothing.
    assert!(
        rustybee::sweeper::backup_once(s.app())
            .await
            .unwrap()
            .is_none()
    );
    // Other files in the directory are never rotated away.
    std::fs::write(backup_dir.join("notes.txt"), "keep me").unwrap();
    for _ in 0..2 {
        s.app().clock.advance_ms(DAY_MS);
        assert!(
            rustybee::sweeper::backup_once(s.app())
                .await
                .unwrap()
                .is_some()
        );
    }
    let names = backups(&backup_dir);
    assert_eq!(names.len(), 3, "{names:?}");
    assert!(names.contains(&"notes.txt".to_string()));
    let first_name = first.file_name().unwrap().to_str().unwrap().to_string();
    assert!(!names.contains(&first_name), "oldest kept: {names:?}");
    assert!(!names.iter().any(|n| n.ends_with(".tmp")));
    s.stop().await;
    let _ = std::fs::remove_dir_all(backup_dir.parent().unwrap());
}
