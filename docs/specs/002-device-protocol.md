# 002 — Device protocol

Status: implemented
Updated: 2026-10-16

## Context
This is the contract between the server and the Android app. It replaces MQTT and FCM-as-transport. Golden examples live in `protocol/fixtures/`, and both test suites parse every fixture.

## Goals / Non-goals
- Goals:
  - one bidirectional socket per device;
  - at-least-once delivery in both directions;
  - idempotent handling, so in practice every message takes effect exactly once;
  - a battery-friendly keepalive.
- Non-goals:
  - multiplexing several devices over one socket;
  - binary encoding.

## Design

### Identifiers and encoding
- Ids (`device_id`, `msg_id`, envelope `id`) are ULIDs: 26 characters, Crockford base32, uppercase.
- Timestamps are integer Unix **milliseconds** in fields ending `_at` or named `at`.
- JSON is UTF-8. Unknown fields are ignored and missing optional fields are `null`, so a new optional field is not a breaking change.
- `sim` is an Android subscription id (int), or `null` for the device default.

### Pairing (HTTP)
1. The web UI creates a pairing code and shows the link `rustybee://pair?u=<urlencoded base URL>&c=<code>` as a QR code. The code is 20 random base32 characters, single-use, and valid for 10 minutes.
2. The app sends `POST {u}/api/device/pair` with `Content-Type: application/json`:
   ```json
   {"code":"…","name":"Pixel 10","manufacturer":"Google","model":"Pixel 10","os_version":"16",
    "app_version":"0.1.0","app_build":1,"fcm_token":null}
   ```
3. The server replies `200` with:
   ```json
   {"device_id":"01J…","device_token":"rbd_…","ws_url":"wss://host/api/device/ws"}
   ```
   - `ws_url` is the base URL with `http→ws` / `https→wss` substituted and `/api/device/ws` appended.
   - `device_token` is `rbd_` followed by 43 base64url characters (32 random bytes). The server stores only its SHA-256.
4. Errors: `400 {"error":{"code":"bad_request",…}}`, or `401 {"error":{"code":"pairing_invalid",…}}` when the code is unknown, expired or already used.

### Connection
- The app opens `GET /api/device/ws` with the header `Authorization: Bearer <device_token>`. A missing or bad token gets HTTP `401` before the upgrade.
- If the device already has an open socket, the server closes the old one with close code **4000 `replaced`**.
- Close code **4003 `revoked`** means the device was removed. The app wipes its credentials, stops the gateway and shows setup again.
- Liveness:
  - The app sends WebSocket ping frames every `config.ping_s` seconds (OkHttp `pingInterval`).
  - The server closes the socket (code 1001) if it receives no frame for `2 × ping_s + 30` seconds.
  - The app reconnects after any close except 4003. It uses jittered exponential backoff (1 s doubling to 120 s, ±20 %), resets to 1 s after a connection has stayed up for 60 s, and also reconnects immediately when the default network changes.

### Envelope
Every text frame is one JSON object:
```json
{"v":1,"id":"<ulid>","type":"<type>","data":{…}}
```
- `v` is always 1. A frame with a different `v`, malformed JSON, or an unknown `type` gets a `nack` (when it has an `id`) and is otherwise ignored.
- **ack:** `{"v":1,"id":"<new ulid>","type":"ack","data":{"ref":"<id>"}}`. The receiver sends it **after** it has durably handled the frame. Acks are never acked.
- **nack:** `{"type":"nack","data":{"ref":"<id>","code":"bad_frame|unsupported|invalid","message":"…"}}`. The sender must not retransmit a nacked frame.
- **Retransmit rule:** neither side retransmits during a healthy connection. After every (re)connect, each side resends every frame it has not had acked, with the **same `id`**. Both sides resend after the `welcome` exchange: the server right after sending `welcome`, the device right after receiving it.
- **Idempotency:** a receiver that sees an `id` again, or a duplicate business key (`msg_id` + `state` for status, `uid` for inbound), acks it and does nothing else.

### Device → server
| type | data | when |
|---|---|---|
| `hello` | `{app_version, app_build, os_version, manufacturer, model, sims:[{subscription_id, slot, carrier, number}], battery:{level, charging}, network, fcm_token, permissions:{send_sms, receive_sms, notifications, battery_unrestricted}, outbox}` | first frame after every connect |
| `status` | `{msg_id, state:"sent"\|"delivered"\|"failed", at, parts, error:{code,message}\|null}` | each change of a message's outcome |
| `inbound` | `{uid, from, body, sim, received_at}` | each received SMS, while forwarding is on, that passes the filters |
| `health` | `{battery, network, permissions, outbox}` | every 15 min while connected, and on a change of charging state or permissions |
| `set_forwarding` | `{base_rev, receive_enabled, filters}` | the user saves the Forwarding screen on the phone, or the phone restores its own "off" (below) |

- `network` is one of `wifi`, `cellular`, `ethernet`, `other`, `none`. `battery.level` is 0–100.
- `inbound.uid` is the lowercase hex of the first 16 bytes of `SHA-256(from + "\n" + received_at + "\n" + body + "\n" + (sim ?: ""))`.
- `status.error.code` is one of:
  - `generic_failure`, `no_service`, `null_pdu`, `radio_off`, `limit_exceeded`, `short_code_not_allowed`, `short_code_never_allowed`;
  - `permission_denied`, `invalid_destination`, `sim_unavailable`, `delivery_failed`;
  - `unknown_<int>`.

  `message` is a human-readable text.

### Server → device
| type | data | when |
|---|---|---|
| `welcome` | `{device_id, server_time, config}` | in reply to every `hello` (after acking it) |
| `config` | `config` | whenever the device settings change |
| `send` | `{msg_id, to, body, sim}` | dispatch of one outbound message to one recipient |

- `config` is:
  ```json
  {"name":"…","ping_s":270,"receive_enabled":true,"sim":null,"filters_rev":3,
   "filters":{"mode":"off"|"allow"|"block","rules":[{"field":"sender"|"body"|"any","op":"equals"|"contains"|"starts_with"|"ends_with","value":"…","ignore_case":true}]}}
  ```
- Forwarding: `receive_enabled=false` forwards nothing, whatever the filters say. The filters apply only while it is `true`.
- Filter semantics:
  - `mode=off`: forward everything.
  - `allow`: forward only messages that match at least one rule.
  - `block`: drop messages that match any rule.
  - `allow` with no rules forwards nothing. Both editors warn about this.
- Forwarding sync: `receive_enabled` and the filters are one setting, edited from the web or the phone. `filters_rev` goes up by one whenever either changes.
  - The device sends `set_forwarding` with the `filters_rev` its edit started from as `base_rev`.
  - If `base_rev` is current and the filters are valid (at most 100 rules; each value non-empty after trimming and at most 200 characters), the server stores both, acks, and sends `config` with the new rev.
  - If `base_rev` is stale, the server nacks with `filters_conflict` and sends the current `config`. The phone reloads its editor.
  - If the filters are invalid, the server nacks with `invalid`.
  - Editing on the phone needs a live connection, so offline edits never have to be merged. The one exception is turning forwarding off, below.
- The phone has the final say on forwarding, because the phone's owner may not be the server's admin:
  - The web can turn forwarding off but never on. Only `set_forwarding` from the phone turns it on.
  - The phone also keeps its own "off" locally. Turning forwarding off on the phone takes effect at once, even offline. While that local "off" is set, the phone forwards nothing even if a `config` says `receive_enabled=true`, and once connected it sends `set_forwarding` with `receive_enabled=false` so the server agrees again. The phone clears its local "off" only when the server acks a `set_forwarding` with `receive_enabled=true`.
  - The web edits the filters only while forwarding is on. A web save with forwarding off keeps the stored filters. Both rules are checked against the stored row in the same write, so a phone change made after the web form was loaded is never undone.
- A replayed `set_forwarding` (same `id`, its ack lost) that was already stored is acked again and not reapplied. One that was refused is judged again.

### Device handling of `send`
1. Look up `msg_id` in the local `sent` table.
   - Found with a terminal outcome: queue that `status` again and ack.
   - Found and still in progress: ack only.
   - Not found: insert the row (`accepted`), ack, then hand the message to the radio.
2. Never send the same `msg_id` twice. An ack means "accepted, and will report an outcome".
3. After `sent`, a `delivered` or `failed` status may follow. The device reports `failed` instead of `sent` if any part fails to send.

### Server handling
- Pacing and replay are covered in spec 006.
- On `hello`, the server stores the telemetry, sets `connected=1`, acks, sends `welcome`, then wakes the dispatcher.

## Acceptance criteria
1. AC-002-01 Every fixture in `protocol/fixtures/` parses and round-trips in both the server and the app test suites.
2. AC-002-02 Pairing with a valid code returns a token that opens the socket. Reusing a code, or using an expired one, returns 401 `pairing_invalid`.
3. AC-002-03 A socket with a bad token is refused with 401 before the upgrade.
4. AC-002-04 Frames left unacked when a connection dropped are resent with the same `id` after reconnect, on both sides.
5. AC-002-05 A duplicate `inbound` (same uid) or a duplicate `status` (same msg_id and state) is acked and has no further effect.
6. AC-002-06 A second connection for the same device closes the first with 4000.
7. AC-002-07 Removing a device closes its socket with 4003, and the app wipes its credentials.
8. AC-002-08 A malformed frame with an `id` gets a `nack`, and the connection stays open.
9. AC-002-09 The server closes a socket that is silent for `2 × ping_s + 30` s.
10. AC-002-10 `set_forwarding` with the current `base_rev` stores `receive_enabled` and the filters and is acked, and a `config` with the next `filters_rev` follows. With a stale `base_rev` it is nacked `filters_conflict`, a `config` with the current settings follows, and nothing is stored.
11. AC-002-11 A web save can turn forwarding off but not on, bumps `filters_rev` when it turns it off, and leaves the filters unchanged while forwarding is off.
12. AC-002-12 With its local "off" set, the phone forwards nothing when a `config` says `receive_enabled=true`, and sends `set_forwarding` with `receive_enabled=false` once connected.

## Test plan
| AC | Test |
|---|---|
| 01 | `server/tests/fixtures.rs`; app `ProtocolFixturesTest` |
| 02, 03, 06, 07, 08 | `server/tests/device_ws.rs` with a tokio-tungstenite client |
| 04 | server: `device_ws.rs::ac_002_04_*`; app: `OutboxTest` |
| 05 | `server/tests/device_ws.rs::ac_002_05_*` |
| 09 | server test with a short `ping_s` |
| 10 | `server/tests/device_ws.rs::ac_002_10_set_forwarding_checks_rev` |
| 11 | `server/tests/ui.rs::ac_002_11_web_turns_forwarding_off_not_on` |
| 12 | app `ForwardingTest` |

## Assumptions
- Ping frames come from the app only. The server never sends pings, which saves server-initiated wakeups on the phone.
- The default `ping_s` is 270, set with `RUSTYBEE_PING_SECONDS`. Use 60 behind Cloudflare.
- The idle-close timeout is derived from `ping_s` but can be injected directly, for tests only.

## Open questions
