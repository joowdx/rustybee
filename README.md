<p align="center">
  <img src="branding/icon-squircle.png" alt="rustybee" width="128" height="128">
</p>

<h1 align="center">rustybee</h1>

<p align="center">A tiny, self-hosted SMS gateway: your Android phone, your server, a simple API.</p>

<p align="center">
  <a href="https://github.com/joowdx/rustybee/releases/latest"><img alt="Latest release" src="https://img.shields.io/github/v/release/joowdx/rustybee"></a>
  <a href="LICENSE"><img alt="MIT licence" src="https://img.shields.io/badge/licence-MIT-blue"></a>
</p>

Turn an Android phone into your own SMS gateway. You run a small server, the phone connects to it, and from then on you can send and receive text messages with a simple HTTP API, a web dashboard, or webhooks. You host all of it yourself.

- **Self-hosted.** One server binary and one SQLite file on your machine. You need no accounts, no cloud services and no third-party SMS provider. Your messages stay on your phone and your server.
- **Tiny.** The server image is under 10 MB and the server idles at about 10–30 MB of RAM with near-zero CPU. It runs on a Raspberry Pi, a NUC, a NAS or a $5 VPS.
- **Uses your own SIM.** Messages go out through your phone's normal SIM plan, so they cost what your plan charges.
- **FOSS by default.** The recommended app build has no Google code at all. An optional build adds Firebase push wake-ups for phones that kill background apps.
- **For one person.** It is built for personal use: one admin, one or a few phones. It is not a bulk-SMS platform for many customers.

> **Where it comes from.** rustybee started as a fork of [textbee](https://github.com/vernu/textbee), the open-source Android SMS gateway. It has since been rewritten from scratch: the NestJS and MongoDB backend and the Next.js dashboard became one Rust binary with SQLite, and the Android app became a new native Kotlin app. It shares no code with textbee and doesn't track it, but the idea came from there. Thanks to the textbee authors.

## Contents

- [How it works](#how-it-works)
- [What you need](#what-you-need)
- [Deploy the server](#deploy-the-server)
- [Set up the phone](#set-up-the-phone)
- [Use the API](#use-the-api)
- [Webhooks](#webhooks)
- [Exposing it to the internet](#exposing-it-to-the-internet)
- [Cloudflare Turnstile (optional)](#cloudflare-turnstile-optional)
- [Configuration](#configuration)
- [Footprint](#footprint)
- [FOSS and GMS app builds](#foss-and-gms-app-builds)
- [Why Rust?](#why-rust)
- [Backups and upgrades](#backups-and-upgrades)
- [Building from source](#building-from-source)
- [Repository layout](#repository-layout)

## How it works

```
 your app / script ──HTTPS──▶ ┌─────────── rustybee server ───────────┐
 browser (dashboard) ───────▶ │ REST API · web UI · webhooks · SQLite  │
                              └──────────────────▲────────────────────┘
                                                 │ one WebSocket, kept open by the phone
                                       ┌─────────┴─────────┐
                                       │ Android phone     │ ──▶ SMS out through your SIM
                                       │ rustybee app      │ ◀── SMS in from anyone
                                       └───────────────────┘
```

1. **The phone dials out to the server.** The app keeps one WebSocket open to your server from a foreground service. The phone needs no open ports and no public IP; it only has to reach the server's URL.
2. **You send a message.** Your code calls `POST /api/v1/messages`. The server saves it in SQLite as `queued` and hands it to the phone over the socket.
3. **The phone sends the SMS** through Android's own SMS system and reports back: `sent`, then `delivered` when the carrier confirms it, or `failed`.
4. **Incoming SMS** go the other way: the phone forwards each one to the server, which stores it and can call your webhook.
5. **Nothing gets lost when the connection drops.** Both sides keep a durable queue. If the phone is offline, outgoing messages wait on the server (for 24 hours by default) and incoming ones wait on the phone. Everything is replayed when they reconnect.

There is no message broker and no separate database server. The server is a single process with an SQLite file.

## What you need

| | Requirement |
|---|---|
| **Server** | Any machine that runs Docker, or Linux with systemd. `linux/amd64` and `linux/arm64` images are published. A few MB of disk and about 30 MB of RAM. |
| **Phone** | Android 16 or newer with an active SIM. A spare phone that stays plugged in works best. |
| **Network** | The phone must reach the server. On your home Wi-Fi a LAN address is enough. To send while the phone is on mobile data, the server needs a public HTTPS URL (see [Exposing it to the internet](#exposing-it-to-the-internet)). |
| **Optional** | A domain and a reverse proxy or Cloudflare Tunnel for HTTPS. Cloudflare Turnstile keys if you want bot protection on the login page. A Firebase project if you want the GMS build's push wake-ups. |

## Deploy the server

### Docker Compose (recommended)

```sh
git clone https://github.com/joowdx/rustybee.git
cd rustybee/deploy
cp .env.example .env
```

Edit `.env`. Only two values matter for a first start:

```sh
RUSTYBEE_PUBLIC_URL=https://sms.example.com     # the URL your phone will use to reach the server
RUSTYBEE_ADMIN_PASSWORD=pick-a-long-password    # used once, on first start
```

Then start it:

```sh
docker compose up -d
docker compose ps        # wait until it says "healthy"
```

Open the public URL and log in with the password. Compose publishes port 8080 on `127.0.0.1` only, for a reverse proxy or tunnel on the same host; to reach it as `http://<server-ip>:8080` on your LAN with no proxy, change the mapping in `compose.yaml` to `"8080:8080"` and set `RUSTYBEE_PUBLIC_URL=http://<server-ip>:8080` in `.env`, since the phone pairs and connects through that URL (trusted LANs only; public access needs a TLS-terminating proxy or tunnel). After the first login, remove `RUSTYBEE_ADMIN_PASSWORD` from `.env`: it is only read when no password is set yet. To change the password later:

```sh
docker exec -it rustybee /rustybee set-password
```

### Plain Docker

```sh
docker run -d --name rustybee --restart unless-stopped \
  -p 127.0.0.1:8080:8080 -v rustybee-data:/data \
  -e RUSTYBEE_PUBLIC_URL=https://sms.example.com \
  -e RUSTYBEE_ADMIN_PASSWORD='pick-a-long-password' \
  ghcr.io/joowdx/rustybee:latest
```

This binds to loopback for a proxy or tunnel on the same host; use `-p 8080:8080` only for direct access on a trusted LAN.

Image tags: `latest`, the major version (e.g. `3`), and `sha-<commit>`. Pin the major version if you want to avoid surprise breaking changes.

### systemd (no Docker)

Build the binary (see [Building from source](#building-from-source)), then:

```sh
sudo install -m 755 server/target/release/rustybee /usr/local/bin/rustybee
sudo install -m 644 deploy/rustybee.service /etc/systemd/system/
sudoedit /etc/rustybee.env     # RUSTYBEE_PUBLIC_URL=…, RUSTYBEE_ADMIN_PASSWORD=…
sudo systemctl daemon-reload && sudo systemctl enable --now rustybee
```

Data lives in `/var/lib/rustybee`. The unit runs with a dynamic user and systemd sandboxing.

More detail, including nginx and Caddy configs, is in [deploy/README.md](deploy/README.md).

## Set up the phone

1. **Install the app.** Download `rustybee-<version>-foss.apk` from the [latest release](https://github.com/joowdx/rustybee/releases/latest) and open it on the phone. Android will ask you to allow installs from your browser or file manager.
2. **Pair it.** In the web UI, go to **Devices → Pair a device**. A QR code appears. In the app, tap **Scan QR code** (or scan it with the camera app, or paste the `rustybee://pair?…` link).
3. **Grant permissions.** The app asks for SMS, phone state and notifications. Allow them.
   - If the SMS permissions are greyed out, that is Android protecting sideloaded apps: go to **App info → ⋮ → Allow restricted settings**, then try again.
4. **Allow unrestricted battery use** when asked. Without it, Android may put the app to sleep and stop the gateway.
5. The device shows as **connected** in the dashboard. Send a test message from the web UI.

Tips for a reliable gateway phone:

- Keep it plugged in and on Wi-Fi, or on mobile data with a public server URL.
- Turn off any vendor "battery saver" or "app sleeping" lists for rustybee (Samsung, Xiaomi and others have their own).
- **Android limits SMS sending to 30 messages in 30 minutes per app** and then shows a confirmation popup on the phone. If you send more than that, raise the limit once over adb:

  ```sh
  adb shell settings put global sms_outgoing_check_max_count 500
  ```

- An `http://` (not https) server URL works on a LAN, but the app makes you tick an "insecure connection" box first.

## Use the API

Create an API key in the web UI under **API keys**. It starts with `rbk_` and is shown only once, so copy it. Send it as a bearer token on every request:

```
Authorization: Bearer rbk_…
```

The base path is `/api/v1`. Requests and responses are JSON. Keys in the query string (`?api_key=`) are rejected on purpose, so they don't end up in logs.

### Send a message

```sh
curl https://sms.example.com/api/v1/messages \
  -H "Authorization: Bearer $RUSTYBEE_KEY" \
  -H 'Content-Type: application/json' \
  -d '{"to":["+15551234567"],"body":"Hello from rustybee"}'
```

- `to`: 1 to 1000 phone numbers. Use international format (`+` and country code).
- `body`: 1 to 1600 characters. Long texts are split into parts automatically.
- `device_id` (optional): which phone sends it. Defaults to the device marked as default.
- `sim` (optional): which SIM slot to use on a dual-SIM phone.

It returns `202 Accepted` right away, with one message per recipient, all `queued`:

```json
{
  "batch_id": "01JAB…",
  "messages": [
    {"id": "01JAC…", "direction": "out", "peer": "+15551234567", "body": "Hello from rustybee",
     "state": "queued", "created_at": "2026-10-16T08:00:00.000Z", "...": "..."}
  ]
}
```

The request is checked as a whole before anything is saved. If one number is invalid, nothing is sent and the error says which one (`"to[3]: …"`).

### Send different texts to different people

```sh
curl https://sms.example.com/api/v1/batches \
  -H "Authorization: Bearer $RUSTYBEE_KEY" \
  -H 'Content-Type: application/json' \
  -d '{"items":[
        {"to":"+15551234567","body":"Hi Ana, your order shipped."},
        {"to":"+15557654321","body":"Hi Ben, your order shipped."}
      ]}'
```

### Check what happened

```sh
# one message
curl -H "Authorization: Bearer $RUSTYBEE_KEY" https://sms.example.com/api/v1/messages/01JAC…

# a whole batch: counts per state
curl -H "Authorization: Bearer $RUSTYBEE_KEY" https://sms.example.com/api/v1/batches/01JAB…

# received messages from one number since a date
curl -G -H "Authorization: Bearer $RUSTYBEE_KEY" https://sms.example.com/api/v1/messages \
  --data-urlencode direction=in \
  --data-urlencode peer=+15551234567 \
  --data-urlencode since=2026-10-01T00:00:00Z
```

A message moves through these states:

```
queued ──▶ dispatched ──▶ sent ──▶ delivered
   │            │           └────▶ failed
   └────────────┴──▶ expired   (phone offline past the TTL)
                     failed
received   (incoming messages)
```

`delivered` depends on your carrier sending delivery reports. Some never do, so `sent` may be the last state you see.

### Endpoints

| Method | Path | What it does |
|---|---|---|
| `GET` | `/api/v1/health` | Liveness check, no key needed. Returns `{"ok":true,"version":"…"}`. |
| `POST` | `/api/v1/messages` | Send one text to one or more numbers. |
| `POST` | `/api/v1/batches` | Send a different text to each number. |
| `GET` | `/api/v1/batches/{id}` | Batch progress: totals and counts per state. |
| `GET` | `/api/v1/messages` | List and search messages. |
| `GET` | `/api/v1/messages/{id}` | One message. |
| `GET` | `/api/v1/devices` | Paired phones: connected, battery, network, SIMs. |

`GET /api/v1/messages` filters: `direction` (`out`/`in`), `state`, `device_id`, `peer`, `batch_id`, `q` (text search in body or number), `since` and `until` (RFC 3339), `limit` (1–200, default 50). Results are newest first. If the response has a `next_cursor`, pass it back as `cursor` to get the next page.

### Errors

Errors always look like this:

```json
{"error": {"code": "invalid_recipient", "message": "to[0]: …"}}
```

| Status | Codes |
|---|---|
| 400 | `bad_request`, `invalid_recipient`, `body_too_long`, `device_required`, `key_in_query` |
| 401 | `unauthorized` (missing, wrong or revoked key) |
| 404 | `not_found` |
| 409 | `device_disabled` |
| 429 | `queue_full` (the phone already has `RUSTYBEE_MAX_QUEUED` messages waiting) |

### From code

Python:

```python
import os, requests

r = requests.post(
    "https://sms.example.com/api/v1/messages",
    headers={"Authorization": f"Bearer {os.environ['RUSTYBEE_KEY']}"},
    json={"to": ["+15551234567"], "body": "Server is down!"},
    timeout=10,
)
r.raise_for_status()
print(r.json()["messages"][0]["id"])
```

JavaScript:

```js
const res = await fetch("https://sms.example.com/api/v1/messages", {
  method: "POST",
  headers: {
    Authorization: `Bearer ${process.env.RUSTYBEE_KEY}`,
    "Content-Type": "application/json",
  },
  body: JSON.stringify({ to: ["+15551234567"], body: "Server is down!" }),
});
if (!res.ok) throw new Error((await res.json()).error.message);
```

## Webhooks

Instead of polling, let rustybee call you. In the web UI under **Webhooks**, add a URL and pick the events:

`message.sent`, `message.delivered`, `message.failed`, `message.expired`, `message.received`

Each event is a `POST` with this body:

```json
{"id": "<delivery id>", "event": "message.received", "created_at": "2026-10-16T08:00:00.000Z",
 "data": { "...the message object, same as the API returns..." }}
```

- **Retries.** Any 2xx counts as success. Network errors, timeouts, 408, 429 and 5xx are retried after 30 s, 2 min, 10 min, 1 h, 6 h and 24 h. Other 4xx responses are tried 3 times in total. Failed deliveries can be resent from the UI.
- **Idempotency.** The `X-Rustybee-Delivery` header is the same on every retry of one delivery, so you can ignore duplicates.
- **LAN targets are allowed**, so a webhook can point at Home Assistant, n8n or anything else on your network.

### Verify the signature

Every request has an `X-Rustybee-Signature: t=<unix seconds>,v1=<hex>` header. `v1` is HMAC-SHA256 of `"<t>.<raw body>"` with the webhook's secret, which is shown in the UI. Check it before trusting the payload:

```python
import hmac, hashlib, time

def verify(secret: str, header: str, raw_body: bytes) -> bool:
    parts = dict(p.split("=", 1) for p in header.split(","))
    t, sig = parts["t"], parts["v1"]
    if abs(time.time() - int(t)) > 300:          # reject replays older than 5 minutes
        return False
    expected = hmac.new(secret.encode(), f"{t}.".encode() + raw_body, hashlib.sha256).hexdigest()
    return hmac.compare_digest(expected, sig)    # constant-time compare
```

Use the raw request body exactly as received, not re-encoded JSON.

## Exposing it to the internet

The server speaks plain HTTP on port 8080 and does no TLS itself. On a trusted LAN that is fine. For anything reachable from the internet, put HTTPS in front of it. Pick one:

- **Cloudflare Tunnel.** No open ports and no public IP needed. Point the tunnel at `http://127.0.0.1:8080` and set:
  ```sh
  RUSTYBEE_TRUST_PROXY=1
  RUSTYBEE_PING_SECONDS=60   # Cloudflare drops idle WebSockets after ~100 s
  ```
- **Caddy.** `reverse_proxy 127.0.0.1:8080`. It gets certificates automatically and handles WebSockets.
- **nginx or Traefik.** Allow WebSocket upgrades on `/api/device/ws`, turn off buffering on `/events`, and set the read timeout above the ping interval. A full nginx example is in [deploy/README.md](deploy/README.md).

Behind any proxy, set `RUSTYBEE_TRUST_PROXY=1` so the server sees real client IPs (used by the login rate limit), and set `RUSTYBEE_PUBLIC_URL` to the `https://` address.

Logging in is protected even without extra setup. By default the dashboard login uses a **one-time code texted to your own phone** through the gateway, with the password as a fallback. Failed attempts are rate-limited per IP. Sessions use `HttpOnly`, `SameSite=Strict` cookies.

## Cloudflare Turnstile (optional)

[Turnstile](https://www.cloudflare.com/products/turnstile/) is Cloudflare's free, privacy-friendly CAPTCHA replacement. rustybee can use it to stop bots from hammering the login page.

**Should you enable it?**

| Your setup | Recommendation |
|---|---|
| LAN only, or behind a VPN like Tailscale/WireGuard | Not needed. |
| Public, behind Cloudflare (proxied DNS or Tunnel) | **Yes, recommended.** It costs nothing and is invisible to you. |
| Public, behind another proxy (Caddy, nginx) without Cloudflare | Has no effect. Turnstile only applies to requests that came through Cloudflare. |

**How it behaves.** It only applies to logins that arrive through Cloudflare (when `RUSTYBEE_TRUST_PROXY=1` and Cloudflare's `CF-Connecting-IP` header is present). Logins from your LAN skip it, so you can still get in when the tunnel or the internet is down. If Cloudflare's check fails or can't be reached, the login is refused (fail closed). It never touches the API or the phone connection.

**Set it up:**

1. In the Cloudflare dashboard, go to **Turnstile → Add widget**.
2. Hostname: your public host, e.g. `sms.example.com`. Widget mode: **Invisible**.
3. Copy the site key and secret key into `.env`:
   ```sh
   RUSTYBEE_TURNSTILE_SITE_KEY=0x4AAAA…
   RUSTYBEE_TURNSTILE_SECRET=0x4AAAA…
   RUSTYBEE_TRUST_PROXY=1
   ```
4. `docker compose up -d` to apply. Both keys must be set together, or the server refuses to start.

## Configuration

Everything is set with environment variables. Only `RUSTYBEE_PUBLIC_URL` is really needed.

| Variable | Default | Meaning |
|---|---|---|
| `RUSTYBEE_PUBLIC_URL` | `http://<bind>` | The URL phones and browsers use. It goes into pairing QR codes. |
| `RUSTYBEE_ADMIN_PASSWORD` | — | Sets the admin password on first start, if none exists yet. |
| `RUSTYBEE_BIND` | `127.0.0.1:8080` (`0.0.0.0:8080` in Docker) | Listen address. |
| `RUSTYBEE_DATA_DIR` | `./data` (`/data` in Docker) | Where the SQLite database lives. |
| `RUSTYBEE_TRUST_PROXY` | `0` | `1` to trust `X-Forwarded-*` and `CF-Connecting-IP` from your proxy. |
| `RUSTYBEE_PING_SECONDS` | `270` | How often the phone pings. Use `60` behind Cloudflare. |
| `RUSTYBEE_MESSAGE_TTL_HOURS` | `24` | How long a message waits for an offline phone before it expires. |
| `RUSTYBEE_OTP_MAX_FAILURES` | `3` | Failed login-code texts in a row before password login unlocks. |
| `RUSTYBEE_MAX_QUEUED` | `1000` | Most messages waiting for one phone. Sends beyond it get `429 queue_full`. |
| `RUSTYBEE_ALLOW_SHORT_CODES` | `0` | `1` to allow recipients with 3–6 digits (short codes). Otherwise 7 digits are required. |
| `RUSTYBEE_RETENTION_DAYS` | — | Delete finished messages and webhook deliveries older than this many days. Unset keeps everything. |
| `RUSTYBEE_TURNSTILE_SITE_KEY` | — | Turnstile site key. Needs the secret too. |
| `RUSTYBEE_TURNSTILE_SECRET` | — | Turnstile secret key. |
| `RUSTYBEE_FCM_CREDENTIALS` | — | Path to a Firebase service-account JSON, for GMS wake-ups. |
| `RUST_LOG` | `info` | Log level (`warn`, `info`, `debug`, …). |

The binary also has a few commands: `rustybee serve`, `set-password`, `backup <path>` and `healthcheck`.

## Footprint

Rough figures for one phone and personal traffic. They depend on your CPU architecture, OS and how many messages you keep.

| | Estimate |
|---|---|
| Container image | about 8 MB (`FROM scratch`, a single static binary) |
| Server binary | about 5–8 MB |
| RAM | about 10–20 MB idle, 25–35 MB under load |
| CPU | near 0% idle; a send or receive takes milliseconds |
| Threads | about 5 (2 async workers, the database writer, and a few helpers) |
| Disk | under 1 MB to start, plus roughly 1 KB per stored message |
| Android app | under 1 MB APK; about 25 MB of memory while connected; no wake locks held while idle |

For comparison, a Node.js backend with MongoDB and a Next.js dashboard typically needs hundreds of MB of RAM before sending a single message.

## FOSS and GMS app builds

Each release has two APKs:

| | `foss` (recommended) | `gms` |
|---|---|---|
| Google / Firebase code | none | Firebase Cloud Messaging only |
| Works on de-Googled phones (GrapheneOS, LineageOS without GApps) | yes | needs Play services |
| Keeps the gateway running | foreground service + battery exemption | the same, plus push wake-ups |
| Extra server setup | none | a Firebase project and `RUSTYBEE_FCM_CREDENTIALS` |

On most phones the foreground service and battery exemption are enough to stay connected around the clock, so start with `foss`. Choose `gms` only if your phone's vendor keeps killing the app anyway. With it, the server sends a small "wake up" push (never message content) when a phone has gone quiet and messages are waiting. FCM is never required: with no credentials set, the server makes no calls to Google.

## Why Rust?

- **Small and cheap to run.** One static binary of a few MB, no runtime, no interpreter. The Docker image is just that binary on an empty `scratch` base. It fits on a Raspberry Pi next to everything else you host.
- **Predictable memory.** No garbage collector and no warm-up. Memory stays flat at a few tens of MB for months.
- **Correctness where it matters.** Losing or double-sending an SMS is a real bug. Rust's type system and the strict message state machine (`queued → dispatched → sent → delivered`) make those mistakes hard to write, and the core logic is plain functions with no IO, tested on their own.
- **Simple to operate.** The server, web UI, templates and static files are compiled into one file. Deploying means copying one binary or pulling one image. Upgrading means replacing it.
- **Safe by default.** Memory safety rules out a whole class of security bugs in a service that is exposed to the internet and holds your messages.

The server uses axum and tokio for HTTP and WebSockets, SQLite (bundled, WAL mode) for storage, askama and htmx for the web UI, and rustls for outbound HTTPS. The Android app is native Kotlin on plain platform Views, with no AppCompat, Compose or other heavy libraries, which keeps the APK under 1 MB.

## Backups and upgrades

Back up while the server is running:

```sh
docker exec rustybee /rustybee backup /data/backup-$(date +%F).db
docker cp rustybee:/data/backup-$(date +%F).db .
```

To restore, stop the server, replace `rustybee.db` in the data volume with the backup (delete any `rustybee.db-wal` and `rustybee.db-shm` next to it), and start it again.

To upgrade:

```sh
docker compose pull && docker compose up -d
```

Database migrations run automatically on start. Install new APKs over the old app; pairing is kept.

## Building from source

Server (Rust stable):

```sh
cd server
cargo test
cargo build --release          # binary at target/release/rustybee
cargo run -- set-password
cargo run -- serve
```

Docker image, from the repository root:

```sh
docker build -f deploy/Dockerfile -t rustybee .
```

Android app (needs Android Studio's bundled JDK, since very new system JDKs are too new for Gradle):

```sh
cd android
./gradlew testFossDebugUnitTest assembleFossDebug
```

The `gms` flavor needs your own `google-services.json` in `android/app/src/gms/`. See [android/README.md](android/README.md) for signing and details.

## Repository layout

- `server/`: the Rust server: REST API, device WebSocket, web UI, webhooks, SQLite.
- `android/`: the Kotlin app, with `foss` and `gms` flavors.
- `protocol/`: the phone–server protocol and shared test fixtures used by both test suites.
- `deploy/`: Dockerfile, Compose file, systemd unit and reverse-proxy notes.
- `docs/specs/`: the specs the code is built from.

## Licence

MIT. See [LICENSE](LICENSE).
