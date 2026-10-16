# rustybee rewrite plan

## Context

The current fork (NestJS + Mongo + Redis + Mosquitto + Next.js, and a Java/Kotlin Android app on minSdk 24) takes 6 containers and several hundred MB of RAM to run. Most of its weight is SaaS features a personal gateway doesn't need. The audit in `docs/rewrite/01-audit.md` also found correctness and security bugs:
- spoofable SMS receivers;
- multipart and multi-recipient status loss;
- no status state machine;
- webhook replay.

The rewrite is **rustybee**, a single-user SMS gateway rebuilt from scratch in the same repo on an orphan branch `rewrite`, which shares no history with `main`:
- **Server:** one Rust binary with SQLite, serving the device WebSocket, the public API and an askama + htmx web UI.
- **Android app:** native Kotlin, `dev.joowdx.rustybee`, minSdk = targetSdk = 36.

Settled decisions:
- WebSocket replaces MQTT.
- FCM is kept only as an optional wake fallback, in a separate `gms` build flavor.
- targetSdk stays at 36 so OTP texts are still received right away.
- Every upstream link, name and SaaS feature is removed.

This plan is copied to `docs/rewrite/02-plan.md` as the first step, so it persists in the repo.

## Working method: spec-driven development (soft gate)

Every piece of work is described in a spec before code is written. The spec is the source of truth, and the tests come from its acceptance criteria.

- **Where:** `docs/specs/NNN-<slug>.md`, one per scope. `docs/specs/README.md` is the index, showing each spec's status.
- **Template:** each spec carries a header (`Status: draft | approved | implemented | superseded`, `Updated: <date>`) and these sections:
  - Context
  - Goals / Non-goals
  - Design (interfaces, data, flows)
  - Acceptance criteria (numbered, each one testable)
  - Test plan (maps tests to criteria)
  - Open questions
- **Lifecycle:**
  1. Draft the spec.
  2. Discuss it with you.
  3. You approve it, and I set `Status: approved`.
  4. Implement it, with tests named after the criterion they cover (e.g. `ac_002_04_replay_after_reconnect`).
  5. Set `Status: implemented`.
  6. Any change of behavior later goes back into the spec first, in the same commit as the code.
- **Soft gate:** I don't refuse to write code without an approved spec. But before coding against a spec that is still `draft`, or against behavior no spec covers, I stop and say so in one line, then ask you to approve the spec as it stands, discuss it, or go ahead anyway. A "go ahead" is recorded as a note in the spec's open questions.
- **Persisting the rule:** in M0, a project `CLAUDE.md` is added with this rule and the spec location, so later sessions follow it too.
- **Bundling:** specs stay small enough to approve in one sitting. Each milestone below lists the specs it needs. The technical sections that follow are **seed content for those specs**: proposals, not approved designs.

## Repo layout (branch `rewrite`)

```
server/        Rust crate `rustybee` (bin)
android/       new Gradle project, single :app module, flavors foss|gms
protocol/      PROTOCOL.md + fixtures/*.json (golden messages, shared by both test suites)
deploy/        Dockerfile (static musl → scratch), compose.yaml, rustybee.service, Caddy/nginx snippets
branding/      keep the icon pipeline; re-export icons for rustybee
docs/rewrite/  01-audit.md, 02-plan.md
.github/workflows/  server.yml (fmt, clippy, test, musl build), android.yml (lint, unit tests, assembleRelease)
```
The branch is an orphan, so none of the old tree is carried over (`api/`, `web/`, `android/`, `mqtt/`, compose, coderabbit, fork workflows, upstream README/SECURITY). The old code stays untouched on `main`, later renamed `legacy`.

## Protocol (M1: written first; both sides code against it)

**Pairing**
1. The web UI creates a single-use pairing code, valid for 10 minutes.
2. It shows a QR code containing `rustybee://pair?u=<server-url>&c=<code>`. Pasting the link into the app also works.
3. The app calls `POST /api/device/pair` with device info and the code. The server returns `{device_id, device_token}`. Only the SHA-256 of the token is stored on the server.

**WebSocket connection**
- The app connects with `GET /api/device/ws` and `Authorization: Bearer <device_token>`.
- Every message uses the envelope `{"v":1,"id":"<ulid>","type":"…", …}`. Each side acks a message by its `id`, and handling is idempotent on `id`.
- Server → device:
  - `welcome`: config, i.e. send_delay_s, ping_s, receive_enabled, filters.
  - `send`: `{msg_id, to, body, sim?}`.
  - `config`.
- Device → server:
  - `hello`: app version, SIMs, battery, charging, network.
  - `status`: `{msg_id, state: sent|delivered|failed, parts, error_code?, at}`.
  - `inbound`: `{uid, from, body, sim, received_at}`.
  - `health`: the same fields as `hello`, sent periodically.
- **Durability:**
  - Server side: rows stay in SQLite until the device acks them, and `dispatched` rows that were never acked are replayed on reconnect.
  - Phone side: a local SQLite outbox holds messages until the server acks them.
  - The phone's dedupe table on `msg_id` means a replayed send is never sent twice; the phone re-reports the stored status instead.
- **Keepalive:** OkHttp ping every `ping_s` seconds. The server sets it per deployment: 270 for a direct or LAN connection, 60 behind Cloudflare.

**Pacing and recovery**
- The server owns pacing. Each device has at most one `send` in flight. The next one goes out after a terminal status (or a 2-minute timeout) and once `send_delay_s` has passed.
- **FCM wake (gms only, optional):** if the server has queued work for a device that has been offline for more than 3 minutes, it sends a high-priority data message `{"t":"wake"}`. It does this at most once every 15 minutes per device.

## Server (M2–M4)

**Crates:**
- Web: axum 0.8, tokio, tower-http (trace, compression, body limit).
- Database: rusqlite (bundled).
- Web UI: askama, rust-embed (htmx + css + a small js file).
- Formats and ids: serde/serde_json, ulid, time.
- Auth and signing: argon2, hmac, sha2, rand, jsonwebtoken (FCM only).
- Outbound HTTP: reqwest (rustls).
- Uploads and pairing: csv, qrcode (SVG).
- Runtime: tracing, mimalloc.
- Release profile: LTO, `opt-level="s"`, strip. Built as a musl static binary.

**Module layout (hexagonal-lite, one crate):**
- `domain/`: pure code with no IO and is the most heavily unit-tested.
  - the message state machine;
  - GSM-7/UCS-2 segment math;
  - the pacing policy;
  - webhook signing.
- `store/`:
  - SQLite with PRAGMAs: WAL, synchronous=NORMAL, busy_timeout, foreign_keys.
  - Writes go through one writer connection owned by a `spawn_blocking` actor (mpsc commands); reads use a small pool.
  - Migrations are embedded SQL tracked with `user_version`.
  - A periodic WAL checkpoint.
- `hub/`:
  - a registry of connected devices, with one tokio task per WebSocket;
  - a per-device dispatcher woken by `Notify` when messages are queued or a status arrives;
  - a `broadcast` channel that feeds the web UI's SSE stream.
- `webhooks/`: an outbox worker. It claims rows with a lease (`UPDATE … RETURNING`) and retries with backoff (30s, 2m, 10m, 1h, 6h, 24h), then aborts. 4xx responses are retried twice before giving up.
- `fcm/`: about 80 lines. It signs a service-account JWT, gets an access token, caches it for 55 minutes and calls the HTTP v1 API. It only switches on when `RUSTYBEE_FCM_CREDENTIALS` is set.
- `http/api.rs`: the public API (`Authorization: Bearer <api key>`; query-string keys are rejected).
  - `POST /api/v1/messages {to[], body, device?, sim?}` creates a batch.
  - `POST /api/v1/batches {items:[{to,body}], device?}`.
  - `GET /api/v1/messages` (filters, keyset paging), `GET /api/v1/messages/:id`, `GET /api/v1/devices`.
- `http/device.rs`: pairing and the WebSocket endpoint.
- `http/ui/`: the web UI (below).
- `config.rs`: settings from environment variables:
  - `RUSTYBEE_DATA_DIR`, `RUSTYBEE_BIND`, `RUSTYBEE_PUBLIC_URL`;
  - `RUSTYBEE_PING_SECONDS`, `RUSTYBEE_MESSAGE_TTL_HOURS` (default 24);
  - `RUSTYBEE_FCM_CREDENTIALS`.
- `main.rs`: the subcommands `serve`, `set-password` (prompts and stores the argon2 hash) and `backup` (`VACUUM INTO`).

**Schema:**
- `settings(key, value)`: holds the admin password hash and session key.
- `sessions(id_hash, created_at, expires_at)`.
- `api_keys(id, name, prefix, sha256, created_at, last_used_at, revoked_at)`.
- `devices(id, name, token_sha256, enabled, is_default, fcm_token, model, app_version, send_delay_s, receive_enabled, filters_json, telemetry_json, sims_json, last_seen_at, connected)`.
- `pairing_codes(code_sha256, expires_at, used_at)`.
- `batches(id, device_id, total, sent, delivered, failed, created_at)`.
- `messages(id, public_id, batch_id, device_id, direction, peer, body, sim, state, parts, error_code, error_message, attempts, created_at, dispatched_at, sent_at, delivered_at, failed_at, received_at)`, with indexes on `(device_id, state)`, `(created_at)` and `(peer)`.
- `inbound_dedupe(device_id, uid)` with a unique constraint.
- `webhooks(id, name, url, events_json, secret, active)`.
- `webhook_deliveries(id, webhook_id, event, payload, idempotency_key, attempts, next_attempt_at, lease_until, last_status, last_error, delivered_at, aborted_at)`.

**Message state machine** (enforced in `domain`; any other transition is ignored):
- `queued → dispatched → sent → delivered`
- `queued|dispatched → failed|expired`
- `sent → failed` (a delivery failure report)

Each transition writes its webhook rows in the same transaction (a transactional outbox), so every event fires exactly once.

Batch counters are recomputed from message states, so there is only one writer.

**Webhooks:**
- Events: `message.sent`, `message.delivered`, `message.failed`, `message.expired`, `message.received`.
- The signature header is `X-Rustybee-Signature: t=<unix>,v1=<hex HMAC-SHA256(secret, "t.body")>`.
- LAN URLs are allowed.

**Web UI** (askama + htmx, embedded; login sets an HttpOnly SameSite=Strict session cookie):
- Login.
- Dashboard: devices with live status over SSE, today's counts, and warnings (device offline, permission missing).
- Send: recipient chips, device/SIM select, a live segment counter (a small vanilla JS file).
- Bulk: CSV upload, server-side parse, a column-mapping form via htmx, a `{{column}}` template with preview, then send.
- History: unified sent and received, filters held in the URL, keyset paging, detail view and reply.
- Devices: pair (QR as SVG), rename, default, delay, receive toggle, SMS filters, remove.
- API keys: create (shown once), rename, revoke.
- Webhooks: CRUD, a delivery log, and redeliver.
- Settings: change password, sessions.
- Light and dark themes via `prefers-color-scheme`. Mobile-friendly layout with no CSS framework, hand-written CSS of about 10 KB.

## Android (M5)

**Build setup:**
- Kotlin 2.x and the current AGP. compileSdk, minSdk and targetSdk are all 36.
- R8 and resource shrinking are on. Release builds use a real keystore read from `keystore.properties`, which is gitignored.
- Flavors: `foss` (no Google dependencies) and `gms` (adds firebase-messaging only).
- Dependencies: OkHttp 5 and kotlinx.serialization (plus coroutines). No AppCompat, Material, Compose, Room, WorkManager or DI.

**Packages:**
- `protocol/`: envelope and message types, matching `protocol/fixtures`.
- `data/`:
  - `Prefs` (config; device token encrypted with an Android Keystore AES-GCM key);
  - `Db` (a raw `SQLiteOpenHelper` with tables `outbox`, `sent_dedupe`, `send_parts`, `log` capped at 1000 rows).
- `gateway/GatewayService`: the FGS, `foregroundServiceType="remoteMessaging"`, on a LOW-importance channel. It owns one `CoroutineScope` that runs:
  - `Connection`: OkHttp WebSocket, jittered backoff from 1 s to 2 min, reconnect on default-network change via `ConnectivityManager.registerDefaultNetworkCallback`, and a stale-callback generation counter;
  - `Sender`: `getSystemService(SmsManager)` then `.createForSubscriptionId()`, `divideMessage`, and one sent PendingIntent and one delivered PendingIntent per part with unique request codes from `send_parts`. The parts are combined into one status per message: failed if any part failed, delivered only when every part was delivered;
  - `Uplink`: drains the outbox over the WebSocket and deletes each row once it is acked.
- `gateway/SmsInboundReceiver`:
  - declared with `android:permission="android.permission.BROADCAST_SMS"`;
  - uses `goAsync()`, joins the parts, applies the filters, reads the SIM from the intent's subscription extra, writes a row to the outbox with a deterministic uid, then wakes the service.
- `gateway/SmsStatusReceiver`: `exported=false`, explicit intents only.
- `gateway/BootReceiver`: starts the FGS on `BOOT_COMPLETED`, which `remoteMessaging` allows.
- `gateway/Watchdog`: a platform `JobScheduler` periodic job (15 minutes, needs network). If the FGS is down and the battery exemption is granted, it restarts the FGS; otherwise it posts a "gateway stopped" notification.
- `gms/WakeService` (gms flavor only): a `FirebaseMessagingService` that starts the FGS on `wake`. It does not use `directBootAware`.
- `ui/` (platform Views; edge-to-edge insets handled; `OnBackInvokedCallback`):
  - `SetupActivity`: handles the `rustybee://pair` deep link or a pasted link, then pairs and requests permissions.
  - `MainActivity`:
    - status: connection, last send and receive, outbox depth;
    - a gateway on/off toggle;
    - a health checklist: SMS permissions, notifications, battery exemption via `ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS`, and a hint about the restricted-settings step;
    - preferred SIM;
    - a log viewer (redacted, exportable);
    - unpair.
  - State reaches the UI through a `StateFlow` exposed by the service through a local binder.
- **Not in the app:** history, compose, update checks, analytics, crash reporting.
- Write down the AOSP 30 SMS / 30 minute prompt and the `settings put global sms_outgoing_check_max_count` override in `android/README.md`.

## Run mode: one-shot

You chose a one-shot run. I write every spec, mark it `approved (self, one-shot)`, and build all milestones straight through. The server and Android work run in parallel in separate worktrees, and nothing stops for spec review. Wherever I had to choose something on my own, it is noted under **Assumptions** in that spec, and `docs/specs/README.md` gathers all of them into one review list for you at the end. The soft gate still applies to later work in other sessions.

Only these still stop the run:
- a missing prerequisite (below);
- the GitHub cutover (push and default branch).

### Prerequisites (provide before starting)

1. **Toolchains: all present, nothing to install.**
   - Rust 1.99 from Homebrew (with clippy and rustfmt) handles local dev and tests.
   - The musl static release binary is built inside the multi-stage Docker build (Docker 29 + buildx), so neither rustup nor cross targets are needed locally.
   - Gradle runs on Android Studio's JBR 25 (`JAVA_HOME="/Applications/Android Studio.app/Contents/jbr/Contents/Home"`), not the system JDK 27.
   - The Android SDK already has the android-36 platform, the emulator, and the existing AVDs `Pixel_10` (API 37, with Play) and `Pixel10_API37_Root`. A minSdk/targetSdk 36 app runs on API 37, which also tests the Android 17 OTP behaviour. `Pixel_10` is used for the gms flavor because it has Play services.
2. **Firebase (reuses your existing project).**
   - (a) In the Firebase console, add an Android app with package `dev.joowdx.rustybee` to the project you already use. Download its `google-services.json` and put it at `android/app/src/gms/google-services.json`. It is gitignored. The existing GitHub secret is for `com.vernu.sms` and won't work.
   - (b) Project settings → Service accounts → Generate new private key. Save the JSON outside the repo and give me the path; the server gets it through `RUSTYBEE_FCM_CREDENTIALS`.
   - If you skip this, the `foss` flavor and the server still build and test fully. Only the FCM wake end-to-end check gets skipped.
3. **Release keystore.** Reuse the one behind the `ANDROID_KEYSTORE_*` GitHub secrets. I can't read secret values, so put the `.jks` file somewhere local and fill in `android/keystore.properties` (gitignored). If you'd rather not, I'll generate a new local keystore, which works because this is a new package with no upgrade path to keep.
4. **Optional: a real phone** on adb (USB or wireless debugging) for the final end-to-end run. The emulator covers everything else.

## Spec index (seeded from the sections above)

| Spec | Scope |
|---|---|
| 000-architecture | System overview, principles, repo layout, tech choices, footprint budgets |
| 001-repo-and-ci | Branch, deletions, skeletons, CI workflows, licence, README |
| 002-device-protocol | Pairing, WS envelope, message types, acks, replay, keepalive (with `protocol/fixtures`) |
| 003-data-model | SQLite schema, migrations, PRAGMAs, writer actor, backup |
| 004-message-lifecycle | State machine, batches, expiry, inbound dedupe, segment math |
| 005-auth | Admin password CLI, sessions, API keys, device tokens, pairing codes |
| 006-dispatch | Per-device dispatcher, pacing, in-flight window, timeouts, reconnect replay |
| 007-public-api | `/api/v1` endpoints, errors, paging |
| 008-webhooks | Events, payloads, signing, outbox, retry, redeliver |
| 009-fcm-wake | Optional wake path, server and `gms` flavor |
| 010-web-ui | Pages, SSE live updates, bulk CSV flow, styling |
| 011-android-architecture | Flavors, deps, GatewayService, Connection, Uplink, storage, Watchdog, boot |
| 012-android-sms | Send (per-part intents, aggregation, SIM), receive (protected receiver, filters, uid) |
| 013-android-ui | Setup and pairing, main screen, health checklist, log |
| 014-deploy | Docker scratch image, compose, systemd, reverse proxy, backups |

## Milestones and order

In one-shot mode each milestone drafts its specs, self-approves them, and implements them without stopping. In normal mode it waits for your approval (soft gate).

1. **M0, scaffold and specs 000, 001:**
   - Create `rewrite` as an **orphan branch** (`git switch --orphan rewrite`): no parent commit and no shared history with `main`, starting from an empty tree.
   - First commit: this plan as `docs/rewrite/02-plan.md`, plus `docs/rewrite/01-audit.md` brought over as a new file (copied from the working tree, not merged).
   - Add `docs/specs/` (template, index, 000, 001) and the project `CLAUDE.md` with the SDD rule.
   - After 001 is approved: add the skeletons, CI, README and a LICENSE (MIT under your name). Nothing needs deleting because the branch starts empty.
   - The old code is only a read-only reference, through `git show main:<path>` or a separate `git worktree` of `main`. Nothing gets copied from it except the branding pipeline (`branding/*.sh|*.py`), and only if 001 says so.
   - Ask before removing the `upstream` git remote.
2. **M1, specs 002–004:** protocol, data model, lifecycle. These are the contract that both sides build against. Golden fixtures are written as part of 002.
3. **M2, server core, specs 005–007:** auth, dispatch, public API.
4. **M3, server extras, specs 008, 009 (server half):** webhooks, FCM wake, SSE.
5. **M4, web UI, spec 010.**
6. **M5, Android app, specs 011–013 and 009 (app half).** This can run in parallel with M2–M4 once M1 is approved.
7. **M6, deploy, spec 014.**
8. **M7, verification:**
   - Check every acceptance criterion against a passing test or a recorded manual check.
   - Flip the specs to `implemented`.
   - Cut over without a merge, since the histories are unrelated: rename `main` to `legacy`, rename `rewrite` to `main`, and make it the default branch on GitHub. Pushing and changing the default branch happen only after you confirm.

Delegation: work that is mechanical and fully specified goes to the Cursor runner. That covers fixtures, Askama templates, CSS, CRUD handlers and Gradle boilerplate. Design-heavy parts stay here: the protocol, dispatcher, state machine, service lifecycle and multipart aggregation.

## Verification

Each spec's acceptance criteria and test plan are the real gate. The checks below are the cross-cutting baseline those specs build on.

- **Server:**
  - Run `cargo test`.
  - Unit tests cover the state machine (every allowed and refused transition), segment math, pacing and signing.
  - Integration tests start axum on an in-memory SQLite and connect a fake device client (tokio-tungstenite) that replays `protocol/fixtures`. They cover: pairing, send, ack, status, then webhook rows; replaying a send after a reconnect; dedupe of inbound messages; and that a stuck queued message expires.
- **Android:**
  - Run `./gradlew testFossDebugUnitTest`. These tests parse the protocol fixtures and cover outbox semantics and multipart aggregation.
  - Smoke-test both flavors' release builds on an API 36 emulator.
- **End to end, local:**
  1. `rustybee set-password`, then `rustybee serve`.
  2. Pair the emulator through the deep link.
  3. Call `curl POST /api/v1/messages` and check that the status reaches `sent`. The emulator reports delivery; check with `adb emu`.
  4. Run `adb emu sms send 5551234 hello`, then check that the message shows up in history and at a local webhook catcher with a valid signature.
  5. Kill the network for 5 minutes and confirm the replay after reconnect.
  6. Reboot the emulator and confirm the FGS comes back.
- **Footprint:**
  - Server idle RSS under 20 MB, with `ps -o rss`.
  - The scratch image under 15 MB.
  - App total PSS under 40 MB while connected, with `adb shell dumpsys meminfo dev.joowdx.rustybee`.
  - Release APK under 3 MB (foss).
  - `dumpsys batterystats` shows no wakelocks held outside sends.
- **Policy:** use `adb shell cmd appops` and Doze (`dumpsys deviceidle force-idle`) to confirm that the socket survives with the exemption and that FCM wake works without it.
