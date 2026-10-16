# Rewrite audit and research

Date: 2026-10-16. Input for the rewrite plan (`02-plan.md`, not written yet).

Scope of the rewrite: personal, single-user SMS gateway. Monorepo with three parts: a Rust server, a native Android app (minSdk 36), and a web UI. Everything is written from scratch. Dropped entirely: subscriptions/billing, public registration, email flows, support, analytics/marketing, onboarding funnels, community/discord/donation pages, update checks against GitHub, and every upstream textbee/vernu link, name and package id.

---

## 1. Current system at a glance

- Today it is NestJS + MongoDB + Redis/Bull + Mosquitto (dynamic-security) + firebase-admin, plus a Next.js 16 standalone server (NextAuth, SSR) and a mixed Java/Kotlin Android app (minSdk 24, targetSdk 34, AGP 7.4, R8 off).
- Running it takes 6 containers: mongo, mongo-express, api, web, redis, mosquitto. Idle RAM is several hundred MB: Node ×2 at roughly 80–150 MB each, plus Mongo. Measured by the user on 2026-10-16 with `docker stats`: mongo 357 MB and ~1.1 % CPU, api 192 MB and ~0.2 % CPU; the whole stack totals ~640 MB and ~1.8 % CPU at idle. Images take ~4.8 GB on disk (~1 GB compressed).
- Only a small core does real work: device registry, an outbound send pipeline with pacing and status, inbound SMS, API keys, webhooks, and a dashboard with about 7 screens.

---

## 2. API audit (NestJS)

### Module verdicts

| Module | What it does | Verdict |
|---|---|---|
| auth | JWT login, Google login, register/reset/verify, API-key CRUD, footprints | SIMPLIFY: one admin password + API keys (SHA-256 hash only) |
| users | user doc with attribution, rollups, email prefs | DROP (singleton settings row at most) |
| gateway | devices, send/bulk, receive, status, heartbeat, recovery poll, stats, listing | KEEP the core, cut ~60% |
| mqtt | Mosquitto transport, dynsec credential rotation, presence, uplinks | KEEP the ideas, replace the transport (see §5) |
| webhook | subscriptions, HMAC delivery, retry cron, auto-disable | KEEP/SIMPLIFY: drop auto-disable and caps |
| notifications | dashboard feed rule engine | DROP |
| mail | SES, templates, bounces, unsubscribe | DROP |
| billing | Polar, plans, quotas | DROP |
| support | support tickets, deletion requests | DROP |
| analytics | Meta CAPI | DROP |
| openapi | public spec builder | SIMPLIFY: generate from Rust, or one hand-written doc |
| common | client IP, Turnstile, UA parsing | DROP (keep real-IP handling only if behind a proxy) |

### Domain worth keeping

- **Device:** id, name, enabled, is_default, fcm_token, model/brand/os/app version, send_delay_seconds (0–3600, default 5), receive_enabled, last_seen_at, connected, and telemetry as one JSON column (battery, charging, network, SIMs).
- **Message:** one table for both directions.
  - Common fields: direction (out/in), device, batch, peer number, body, sim_subscription_id, status, error_code, error_message.
  - Timestamps: created/dispatched/sent/delivered/failed/received.
  - Retry fields: dispatch_attempts, dispatch_due_at.
- **Batch:** device, body template, recipient_count, success/failure counts, status.
- **ApiKey:** prefix, name, sha256, last_used_at, revoked_at.
- **Webhook:** name, url, events (JSON), secret, active. **WebhookDelivery** (the outbox): webhook, event, payload, idempotency_key, attempt_count, next_attempt_at, last_status, last_error, delivered_at, aborted_at.
- **Mongo to SQLite mapping:**
  - ObjectIds become integer rowids internally, plus a short random public id (ULID) where ids leave the server.
  - `findOneAndUpdate` claims become `UPDATE … RETURNING`.
  - Keyset pagination runs on rowid.

### Device protocol (current)

- **Device endpoints** (`x-api-key`):
  - `POST /devices` registers. It is not idempotent and creates duplicates.
  - `PATCH /devices/:id` updates.
  - `POST /devices/:id/heartbeat` sends heavy telemetry. The reply carries pendingCount and server-pushed config flags.
  - `PATCH /devices/:id/sms-status`, `POST /devices/:id/receive-sms`.
  - `GET /devices/:id/messages/pending` is the recovery claim: up to 25 messages, 30 min–72 h old, fewer than 3 attempts.
  - `POST /devices/:id/mqtt-credentials`.
- **Downlink:** MQTT `down/send` or `down/heartbeat-check`, otherwise FCM data (`smsData` JSON string, high priority, TTL 72 h).
- **Uplink:** MQTT `up/sms-status` and `up/received`, with HTTP fallback. Presence uses a retained `status` topic with an LWT.

### Send pipeline (current)

1. `send-sms` / `send-bulk-sms` resolves the device (default, then online, then most recent).
2. It creates one batch and N messages (`pending`).
3. In queue mode, Bull sends waves paced by the device's send delay. In direct mode it calls FCM synchronously.
4. MQTT is used if the device is connected, FCM otherwise. If the MQTT publish fails it falls back to FCM, and the device dedupes.
5. The device sends and reports sent, delivered or failed. Each report overwrites the status and fires a webhook.
6. A 5-minute cron marks messages stuck more than 20 min as `unknown`. The recovery poll re-dispatches them, up to 3 attempts within 72 h.

### Bugs and smells not to repeat

- **No validation pipe.** DTO decorators are never enforced, and `sms-status` accepts any string.
- **No status state machine.** A late `sent` overwrites `delivered`, and every report fires a webhook.
- **The direct path never marks dispatched or failed.** Bulk marks the batch completed after the first push.
- **Error writes go to a nonexistent field** (`error`), so failures are silently lost.
- **Two batch-status writers disagree.**
- **Webhook rows created without `next_attempt_at`** are never retried if their Redis job is lost. The queue and the cron have no lease, so double delivery is possible.
- **Webhook signature** is HMAC over the body only, with no timestamp, so it can be replayed. 4xx responses abort immediately.
- **The stale threshold (20 min)** is shorter than a slow wave takes to drain on the device, which produces false `unknown`s.
- **API keys are accepted in the query string**, so they leak into logs.
- **Redis grows forever** (`removeOnComplete: false`).
- **Legacy aliases** (`sendSMS`, `receiveSMS`, `smsBody`, `receivers`) and version-gating sprawl.

### Good ideas to carry over

- Wave pacing tied to the device's send delay.
- Atomic recovery claim with an attempt cap.
- Received-SMS dedupe window (same device+sender+body within ±5 s).
- Presence with a wake push when a device drops.
- Bounded push TTLs, and invalidating a token on a permanent FCM error.
- Server-pushed device config via the heartbeat reply.
- Webhook idempotency key and URL snapshot.
- API keys stored as a SHA-256 hash with a visible prefix.

---

## 3. Android audit

### Feature verdicts

| Feature | Verdict |
|---|---|
| Splash router, legacy `MainActivity` (800+ lines Java + XML), `SMSFilterActivity` | DROP |
| Onboarding (5 screens, ZXing QR) | SIMPLIFY: one setup screen (server URL + key) + permissions; QR via system scanner or none |
| Dashboard (subscription, profile, docs) | SIMPLIFY: gateway toggle, connection state, last activity |
| Messages screen / compose | DROP (web UI's job); maybe a small local recent list |
| Activity log (`DeviceLog`, redacted ring buffer) | KEEP |
| Settings: name, gateway on/off, receive on/off, send delay, SIM | KEEP |
| SMS filter (allow/block, match modes) | KEEP |
| Device health (permissions, notifications, battery exemption) | KEEP, fold into settings |
| Release checker / update notifier / version tracker | DROP |
| Server-driven feature flags | DROP (hardcode the new behaviour) |
| Recovery poll | KEEP |
| Heartbeat | KEEP, slim payload |
| Crashlytics, Analytics | DROP |
| Room (already commented out) | DROP |

### Background model today and its problems

- **FGS** with `remoteMessaging` type: the correct choice, keep it. Its channel is IMPORTANCE_HIGH when it should be LOW or MIN. On 14+ the user can dismiss the notification.
- **Heartbeat worker starts the FGS from the background.** That throws on 12+ unless the app is exempt, and the error is swallowed.
- **Boot receiver** does async network calls without `goAsync()`.
- **FCM service** is `directBootAware` but reads credential-encrypted prefs.
- **Expedited workers `Thread.sleep`** up to 120 s, and the legacy chain sleeps up to an hour, which breaks the worker limit.
- **Heartbeat rescheduling** uses `REPLACE` on every boot and every check, which resets the period.
- **The MQTT client is HiveMQ** (Netty, RxJava, Dagger, JCTools): several MB of code and the largest runtime-memory cost. It uses a 30 s keepalive.

### Correctness and security bugs

- **`SMSBroadcastReceiver` is exported without `BROADCAST_SMS`**, so any app can inject fake inbound SMS.
- **`SMSStatusReceiver` is exported** with plain actions, so statuses can be spoofed.
- **Multipart:** one sent/delivered PendingIntent is shared by all parts. SENT can be reported after part 1 even if a later part fails.
- **Multi-recipient:** the PendingIntent request code and the unique status work key leave out the recipient, so only the first recipient's status is reported.
- **Received dedupe** is a 5 s in-memory cache, which is lost on process death. The receiving SIM isn't recorded.
- **The API key is copied into every WorkManager job's input data** (plaintext in the WM database). MQTT password is plaintext in prefs.
- **Smaller issues:**
  - Deprecated `SmsManager.getDefault()` and `NetworkInfo`.
  - `Gson()` constructed per call.
  - Reflection for result-code names.
  - The heartbeat reports app heap as device memory.

### Dependencies

Firebase BoM 29 (Analytics, Crashlytics, Messaging), two Retrofit stacks + Gson, HiveMQ + Netty, ZXing, AppCompat/Material/RecyclerView/ConstraintLayout, Compose BOM 2023.08 + material-icons-extended (with R8 off), WorkManager 2.7. Release is signed with the debug keystore. Nearly all of it goes.

### Good ideas to carry over

- **Reconnect logic:** jittered exponential backoff (1 s to 2 min), immediate reconnect on a default-network change, and a generation counter that discards stale callbacks.
- **Uplink** with ack timeout, then HTTP fallback, relying on server-side idempotency.
- **Dedupe semantics:** "seen" vs "sent", and only "sent" blocks a resend.
- **`SendSlotScheduler` pacing math**, with clock-skew guards.
- **Retries only on transient errors:** 408, 429, 5xx, network.
- **Testable code:** `DeviceHealth` probe abstraction, redacted `DeviceLog`.

---

## 4. Web audit

- **Footprint today:** Next 16 standalone + NextAuth + SSR session reads on every request. It needs a Node server, with a ~150–250 MB image and ~80–150 MB idle RSS, even though 118 files are `'use client'`. Nothing in it needs SSR.
- **Keep:**
  - Devices: list, status, default, delete, and the heartbeat/health data the API stores but never shows.
  - Add device: create a key and show it as a QR code.
  - Send: multi-recipient, device/SIM picker, GSM-7/UCS-2 segment counter.
  - Bulk CSV wizard: upload, map columns, `{{column}}` template with preview, send.
  - History: unified sent and received, filters in the URL, details, reply.
  - Stats, SMS-permission alert, API keys (list/rename/revoke/delete), webhooks + delivery log, change password, dark mode, mobile layout.
- **Drop:** register/reset/verify, checkout/billing, contribute, discord/community, support, profile/delete-account, onboarding, notification feed, banners, surveys, analytics, Google sign-in, robots, the multi-language API guide (replace with one curl example).
- **Auth today:** NextAuth wraps the API's JWT. Replace it with one HttpOnly session cookie set by the server.

---

## 5. Research: transport

| Option | Verdict |
|---|---|
| MQTT + Mosquitto (today) | Extra daemon to run and secure. QoS and retained messages duplicate state SQLite already holds. Heavy client on the phone. |
| Embedded broker (rumqttd) | Maintained, but persistence is still experimental. It adds MQTT framing with no benefit at 1–3 devices. |
| **WebSocket (device → server, persistent)** | **Recommended.** One bidirectional socket carries commands down and acks, status and inbound SMS up. Native in axum and OkHttp. Works through a reverse proxy or Cloudflare Tunnel. |
| SSE / long-poll | One-way; still needs POSTs for uplink. No gain. |
| WebTransport / HTTP/3 | No mature Android client; poor fit with tunnels. Skip. |
| FCM only (no socket) | Lowest battery. But delivery depends on Google's priority rules: FCM downgrades high-priority messages that don't produce a visible notification. Too weak for "within seconds". |
| UnifiedPush | Needs a distributor app holding its own socket. For one dedicated phone, our own socket already plays that role. |

**Recommendation: replace MQTT with a WebSocket**, using a small typed JSON message protocol with ids, acks and replay from the server-side outbox.

- **Durability lives in SQLite**, not in the transport:
  - The server keeps each command until the device acks it.
  - The phone keeps unsent uplinks in a small local outbox until the server acks them.
- **Optional FCM high-priority data message, used only as a wake signal** when the socket has been down longer than N minutes. It brings back the FGS (a high-priority FCM message may start one from the background).
  - With FCM: Firebase stays on the phone (Play Services required) and the server needs a service account.
  - Without FCM: the app is fully Google-free and relies on the FGS, the battery exemption and a boot restart.
- **Keepalive must be shorter than the shortest idle timeout on the path:**
  - Cloudflare: ~100 s, so ping every ~60–90 s.
  - nginx default: 60 s, so raise `proxy_read_timeout`.
  - Direct or LAN: 4–5 min.
  - Expect a gateway phone to be on Wi-Fi and a charger, so the cost is small. On cellular it matters.

## 6. Research: Android 16 (API 36) platform constraints

- **FGS type: `remoteMessaging`.** It is defined as moving text messages between devices, has no time limit, and can be started from `BOOT_COMPLETED`.
  - Avoid `dataSync` (6 h/24 h limit on 15+, banned from boot), `shortService` (~3 min) and `connectedDevice`.
  - `specialUse` is the fallback.
- **Job quotas (Android 16):** jobs that run while an FGS is active now count against normal quota. So send and ack work runs in the FGS's coroutines, not WorkManager. WorkManager is used only when no FGS is running (e.g. the periodic health check).
- **Battery:**
  - Request the ignore-battery-optimizations exemption. It's allowed for sideloaded apps, and the docs only guarantee network access in Doze for exempt apps.
  - FGS channel at LOW importance.
  - Handle the user dismissing the FGS notification.
- **UI rules at target 36:**
  - Edge-to-edge is enforced.
  - Predictive back is on: no `onBackPressed`, use `OnBackInvokedCallback`.
  - Orientation and resizability locks are ignored at ≥600 dp.
- **SMS:**
  - `SEND_SMS` and `RECEIVE_SMS` are enough. The default-SMS role is not required.
  - Use `context.getSystemService(SmsManager::class.java).createForSubscriptionId(id)`.
  - AOSP shows a confirmation prompt after 30 SMS in 30 min. It can be raised with `settings put global sms_outgoing_check_max_count`. Document this for bulk sends.
- **Restricted settings (15+):** an APK installed from a browser or file manager can't get SMS permissions until the user taps App info → ⋮ → Allow restricted settings. `adb install`, Obtainium and F-Droid-style session installers avoid it.
- **Android 17 (API 37) trap:**
  - Apps that target 37 stop receiving SMS containing OTPs for 3 hours.
  - On Android 17 devices, SMS Retriever and WebOTP-format OTPs are held back for every app, whatever its target.
  - **Keep targetSdk 36** if OTP forwarding matters. Sideloading has no Play target requirement.
- **Developer verification:** enforced in 4 countries since 2026-09-30, global from 2027. A free limited-distribution account covers up to 20 devices. ADB installs are reported to remain exempt. Register before 2027.
- **16 KB page size:** irrelevant unless native `.so` files are added.
- **Distribution:** Google's last official figure (Dec 2025) was Android 16 at 7.5%. Third-party figures for mid-2026 put it at ~24%. Fine for a personal device fleet.

## 7. Research: stack choices

### Android (lean)

| Concern | Choice | Why |
|---|---|---|
| Language | Kotlin 2.x only | — |
| UI | Platform Views, no AppCompat/Material (alternative: minimal Compose) | 2–3 screens. Views mean zero UI dependencies and the lowest heap. Compose adds a few MB of APK and heap for no gain here. |
| Network | OkHttp 5 (HTTP + WebSocket) | One library for both. Ktor would only wrap it. |
| JSON | kotlinx.serialization | No reflection, R8-friendly. |
| Storage | SharedPreferences (config) + raw `SQLiteOpenHelper` (outbox, dedupe, log) | No Room/KSP, no DataStore. |
| DI | none, manual wiring | — |
| Build | AGP 8.x/9, R8 full mode, resource shrinking, real release keystore | Optional: baseline profile. |
| Push | Firebase Messaging only if the FCM wake is kept | — |

### Server (Rust)

| Concern | Choice | Why |
|---|---|---|
| HTTP + WS | axum 0.8 on tokio | Built-in WebSocket support, tower middleware. |
| SQLite | rusqlite (bundled) with one writer via `spawn_blocking` / `tokio-rusqlite`; WAL, `synchronous=NORMAL`, `busy_timeout`, `foreign_keys`, periodic checkpoint | Lighter and faster to build than sqlx. Skip sea-orm. |
| Migrations | embedded SQL files (`rusqlite_migration` or hand-rolled `user_version`) | — |
| Jobs | outbox tables + a tokio task woken by `Notify` + a periodic sweep | apalis is overkill. |
| Auth | argon2 (one admin password), signed/private cookie session, API keys = random secret with SHA-256 stored | — |
| Webhooks | HMAC-SHA256 over `timestamp.body`, timestamp header, exponential retry from the outbox | Fixes replay. |
| FCM (optional) | ~80 lines: RS256 service-account JWT → token (cached ~55 min) → HTTP v1 via reqwest + rustls | Avoids `yup-oauth2`. |
| Assets | rust-embed | Single binary. |
| Build | musl static, rustls, mimalloc, LTO, `opt-level="s"`, strip | Expected idle RSS ~5–15 MB (estimate, not measured). |
| API docs | utoipa (optional) | — |

### Web UI

**Recommendation: server-rendered HTML** using askama templates and htmx (~16 KB), embedded in the server binary. It needs no JS build toolchain or Node runtime. The page handlers sit in the same binary as the JSON API, and the htmx SSE extension can push live status updates.

- SPA (Svelte/Preact/Solid built to static files): small too, but it adds an npm build and a duplicate JSON client layer, with nothing in return for a single-user admin page.
- Leptos/Dioxus: heavy WASM and slow builds. Skip.
- Small client JS that can't be avoided: CSV parsing and column mapping in the bulk wizard, the segment counter, QR rendering. Write these as small vanilla modules, or do the CSV parsing server-side.

---

## 8. Design patterns per scope (proposed)

| Scope | Pattern |
|---|---|
| Server layout | Hexagonal-lite: `domain` (pure types, state machine, pacing math), `store` (SQLite repositories), `transport` (WS hub, optional FCM), `http` (axum routers: device API, public API, web UI). One crate with modules; split into a workspace only if build times demand it. |
| Message lifecycle | Explicit state machine with monotonic transitions (`queued → dispatched → sent → delivered`, `failed`, `expired`). A transition fires its webhook exactly once. Status reports are idempotent. |
| Reliable delivery (both directions) | Transactional outbox + acknowledged delivery + idempotent consumers (dedupe on message id + recipient + part). |
| Device commands | Command/ack protocol over the WebSocket, with a versioned JSON envelope `{v, id, type, payload}`, server-side replay of unacked commands on reconnect, and pacing owned by the server (one in-flight window per device). |
| Webhooks | Outbox + scheduler with exponential backoff, a lease column to stop double delivery, signed payloads. |
| Concurrency (server) | One writer task for SQLite (actor-style), readers on a small pool. One tokio task per connected device. |
| Android service | One FGS owning a single coroutine scope that holds the socket, send queue, uplink outbox and pacing. Receivers only hand work to the service, or enqueue a short job when the service isn't running. Unidirectional state (`StateFlow`) to the UI. |
| Android SMS | Per-part, per-recipient PendingIntents with unique request codes. Aggregate the parts into one outcome per recipient. |
| Config | Server is the source of truth for per-device settings, pushed over the socket. The phone caches the last config. |

---

## 9. Open questions for the plan

1. Transport: WebSocket replaces MQTT (recommended), or keep MQTT?
2. FCM wake: keep it as an optional fallback (needs Play Services and a Firebase project), or go fully Google-free?
3. targetSdk 36 (recommended, keeps OTP forwarding) vs 37.
4. Web UI: askama + htmx in the server binary (recommended), or a static SPA?
5. ~~Project name and git history~~ Decided: same repo, rewrite on a new branch. Name and `applicationId` still pending (proposed: `rustybee` / `dev.joowdx.rustybee`).
