# 016 — Notification stats

Status: implemented
Updated: 2026-11-02

## Context
The gateway notification shows only "Sent today: N". It is the one place the gateway is always visible, but today's sends alone are rarely what the user wants to know. The user should be able to pick the period, see received as well as sent, and see why the gateway is reconnecting.

## Goals / Non-goals
- Goal: a phone-only setting for the period the notification counts: today, this week (any start day), this month, or all time.
- Goal: the counts come from the server, which has the full history, and fall back to the phone's own counts while offline.
- Goal: the notification names the period once, for both numbers, and shows the queue and the reconnect reason.
- Non-goal: notification buttons. The ongoing notification gets no Stop or Retry action.
- Non-goal: showing these periods in the web UI or the API.

## Design

### Phone settings
- A **Notification** button joins the Device buttons, which now take two rows: Forwarding, Network, Notification, then Logs, Unpair. It opens `NotificationActivity`.
- **Period**: radio choices Today, This week, This month and All time. The default is Today, so nothing changes after the update until the user picks another.
- **Week starts on**: a drop-down of the seven days, shown only while This week is picked. The default is the locale's first day of the week.
- Both are phone-only preferences in `Prefs`; the server never sees or changes them.
- Periods follow the phone's time zone and are calendar periods: today from local midnight, the week from the last start day at local midnight, the month from the 1st at local midnight. All time has no start.

### Counts on the phone
- A `daily (day INTEGER PRIMARY KEY, sent INTEGER NOT NULL, received INTEGER NOT NULL)` table in the phone's db, keyed by local epoch day, replaces the `today` counters in `Prefs`. It holds this phone's own counts and is never pruned (one small row a day).
- "Sent" is one message whose `status` went to `sent` (once per `msg_id`, even if it is later delivered); "received" is one `inbound` the phone forwarded. These are the same events the server counts below.
- The main screen's "Today: N sent, M received" line reads the `daily` table. Today's count kept by the old app version is not carried over.

### Protocol
Device → server:

| type | data | when |
|---|---|---|
| `stats_req` | `{since}` | see below |

Server → device:

| type | data | when |
|---|---|---|
| `stats` | `{since, sent, received, at}` | in reply to `stats_req`, after acking it |

- `since` is epoch milliseconds, or `null` for all time. The phone computes it from its period setting.
- The server counts this device's messages (`device_id` of the connection, internal ones included) with `sent_at >= since` for `sent`, and `received_at >= since` for `received`. `at` is the server time of the count. `sent_at` and `received_at` are the phone's own times from the `status` and `inbound` frames, so the server and the phone place each event on the same day.
- `stats` is not stored or resent after a reconnect; a lost one is replaced by the next request. A replayed `stats_req` (same `id`) is acked and answered again.
- An older server nacks `stats_req` with `unsupported`. The phone then shows its local counts and asks again only after the next `welcome`.

### When the phone asks
- After `welcome`, once its outbox is empty, so everything it has sent or received is already counted on the server.
- When the period setting changes, and when the current period ends (local midnight, the week start, the 1st), while connected and the outbox is empty; otherwise at the next point the outbox empties.
- With every `health` (every 15 minutes), so the server count corrects any drift.

### What the notification shows
- Title: "Connected" or "Reconnecting", as now.
- Text: `{Period}: {sent} sent, {received} received`, where Period is Today, This week, This month or All time; then `, {n} queued` when the outbox is not empty. While reconnecting with an error, the text is the last error as the main screen shows it, and the expanded view adds the counts on a second line.
- The counts shown are: the last `stats` reply for the current period, plus what the phone's `daily` counts for the period grew by since the request was sent. A request goes only with an empty outbox and frames arrive in order, so the server has counted everything before it and nothing after it. Without a reply for the current period (offline since it began, or an older server), the phone's `daily` counts for the period alone. A reply that arrives after the connection dropped, or for another period, is ignored.
- `GatewayContent` holds everything shown (connection, period, counts, queued, error), so the notification is re-posted only when one of them changes; `setOnlyAlertOnce` keeps re-posts silent.

## Acceptance criteria
1. AC-016-01 `protocol/fixtures/` gains `stats_req.json` and `stats.json`; both suites round-trip them (AC-002-01).
2. AC-016-02 Server: `stats_req` is acked and answered with `stats` counting only this device's messages with `sent_at` / `received_at` at or after `since`; `since: null` counts all of them; a delivered message counts once as sent.
3. AC-016-03 Server: a replayed `stats_req` is acked and answered again.
4. AC-016-04 App: the period defaults to Today and the week start to the locale's first day; the week-start choice shows only for This week.
5. AC-016-05 App: `since` is local midnight today, the most recent week-start day at local midnight (today when it is that day), the 1st of the month at local midnight, or `null`, across DST changes.
6. AC-016-06 App: the shown counts are the reply plus what `daily` grew by since the request; without a reply for the current period, `daily` alone; a period rollover drops the old reply.
7. AC-016-07 App: `stats_req` is sent after `welcome` only once the outbox is empty, on a period change, on a period rollover and with each `health`; a nack `unsupported` stops requests until the next `welcome`.
8. AC-016-08 App: the text reads `{Period}: {sent} sent, {received} received`, adds `, {n} queued` when the outbox is not empty, and while reconnecting with an error shows the error, with the counts below it when expanded.
9. AC-016-09 App: a `status` going to `sent` bumps today's `sent` once per `msg_id`; a forwarded `inbound` bumps `received`.
10. AC-016-10 Manual, on a real phone: switching the period updates the notification within seconds while connected, and in airplane mode the counts still grow with local events.

## Test plan
| AC | Test |
|---|---|
| 01 | `server/tests/fixtures.rs`; app `ProtocolFixturesTest` |
| 02, 03 | `server/tests/device_ws.rs::ac_016_02_*`, `::ac_016_03_*` |
| 04, 05 | app `StatsPeriodTest` |
| 06–09 | app `NotificationStatsTest` |
| 10 | Manual |

## Assumptions
- Calendar periods, not rolling 7 or 30 days.
- Counts are this device's own, not all devices on the server.
- The `daily` table starts empty: offline counts cover only days since the update. The server's counts cover the device's full history.

## Open questions
- With `RUSTYBEE_RETENTION_DAYS` set, pruned messages drop out of the server's counts, so All time (and any period longer than the retention) undercounts. A per-device daily counter table on the server would fix that; left out until retention is in use.
- A device that unpairs and pairs again is a new device on the server, so its server history starts over.
- A month start day other than the 1st (to match a billing cycle) was suggested in chat and not taken up.
- Buttons: Stop was ruled out as too easy to hit by accident; a "Retry now" action while reconnecting was suggested and is not part of this spec (2026-11-02, chat).
