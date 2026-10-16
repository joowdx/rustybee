# 008 — Webhooks

Status: implemented
Updated: 2026-10-17

## Design
- **Events:** `message.sent`, `message.delivered`, `message.failed`, `message.expired`, `message.received`.
- **Subscription fields:**
  - `name`;
  - `url` (http or https; LAN and loopback addresses are allowed);
  - `events` (non-empty subset);
  - `secret` (generated as 32 random bytes in hex and shown in the UI; it can be rotated);
  - `active`.
- **Outbox:** spec 004 inserts the delivery rows inside the state-change transaction, with:
  - `payload = {"id":"<delivery public_id>","event":"…","created_at":"RFC3339","data":<Message object>}`;
  - `next_attempt_at = now`.

  The worker is woken through `Notify`.
- **Worker:** one tokio task, with a concurrency of 4 requests.
  1. Claim due rows: `UPDATE webhook_deliveries SET lease_until=now+60s WHERE id IN (SELECT id … WHERE next_attempt_at<=now AND (lease_until IS NULL OR lease_until<now) AND delivered_at IS NULL AND aborted_at IS NULL ORDER BY next_attempt_at LIMIT 16) RETURNING …`.
  2. POST each claimed row.
  3. On success: set `delivered_at` and clear the lease.
  4. On failure: `attempts += 1`, then set `next_attempt_at` from the backoff schedule, or abort.
- **Request:**
  - Method and timeout: `POST`, 10 s.
  - Headers:
    - `Content-Type: application/json`
    - `User-Agent: rustybee/<version>`
    - `X-Rustybee-Event`
    - `X-Rustybee-Delivery: <public_id>` (the idempotency key)
    - `X-Rustybee-Signature: t=<unix seconds>,v1=<hex HMAC-SHA256(secret, "<t>.<raw body>")>`
  - The signature is computed again on each attempt.
- **Outcome:**
  - **Success:** any 2xx.
  - **Retryable:** a network error, a timeout, 408, 429 or any 5xx.
  - **Other 4xx:** retried at most twice (three attempts in total), then aborted.
  - **Backoff after attempt n:** 30 s, 2 min, 10 min, 1 h, 6 h, 24 h. The delivery is aborted after the 7th failed attempt.
  - `last_status` and `last_error` (the first 500 characters of the body or the error) are recorded. At most 2 KiB of the response body is read, then the connection is dropped; bytes cut mid-character become U+FFFD.
- **Redeliver** (UI): resets an aborted or delivered row to `attempts=0`, `next_attempt_at=now`, `aborted_at=NULL`, `delivered_at=NULL`.
- **Test** (UI): sends a synthetic `message.received` payload marked `"test":true` directly, without writing to the outbox.
- **Verification recipe** for receivers, documented in the README: split the header, recompute the HMAC over `t.body`, compare in constant time, and reject if `|now − t| > 300 s`.

## Acceptance criteria
1. AC-008-01 A status transition produces exactly one delivery for each subscribed active webhook, and none for unsubscribed or inactive ones.
2. AC-008-02 The signature verifies with the recipe. A different secret fails.
3. AC-008-03 A 500 response retries with the backoff schedule. A 400 response aborts after the 3rd attempt. A 2xx marks the delivery delivered.
4. AC-008-04 Two workers, or a restart in the middle of a delivery, never deliver the same row concurrently: the lease prevents it, and an expired lease is picked up again.
5. AC-008-05 Redeliver sends the delivery again.
6. AC-008-06 A receiver that streams an endless body still gets its status recorded, with `last_error` holding the first 500 characters.

## Test plan
| AC | Test |
|---|---|
| 01–06 | `server/tests/webhooks.rs` with a local axum receiver and an injected clock and backoff scale |

## Assumptions
- No automatic disabling of failing webhooks.

## Open questions
