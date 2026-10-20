# 010 — Web UI

Status: implemented
Updated: 2026-10-20

## Design

### Stack
- **Rendering:** askama templates, rendered on the server.
- **Interactivity:** htmx 2.x, vendored into `server/static/` and embedded with rust-embed. Live updates use htmx's SSE extension.
- **Front-end code:** one `app.css` (hand-written, under about 24 KB) and one `app.js` (segment counter, recipient chips, copy buttons; under about 6 KB).
- **No external requests.** No CDN, no web fonts; the system font stack only.
- **Themes:** light and dark through `prefers-color-scheme`.
- **Layout:** one centred column (max 1120 px) shared by the top bar and the page. Under 640 px the tabs move to a bottom bar with Home, Send, Messages and Devices, plus a "More" menu (a `<details>`, so it works without JavaScript) for Bulk, Keys, Webhooks and Settings.

### Pages
All pages except `/login` require a session (spec 005).

| Route | Content |
|---|---|
| `/login` | "Text me a code" when a code can be sent, then the code form. The password form appears only when spec 005 allows it; otherwise a hint says when it opens. Shows the "set password" hint when none is set. |
| `/` | Device cards: name, status dot (online, offline, or unreachable, below), battery and charging, network, last seen, outbox depth, permission warnings, and a "Last restart" line (below). Today's counts: sent, delivered, failed, received, queued. Recent 10 messages. Updated live over SSE: at most one refresh a second, and always one after the last event of a burst. |
| `/send` | Recipient chip input. The body textarea shows encoding, characters, parts and remaining. Device and SIM selects. Submits via htmx and shows the result inline. |
| `/bulk` | Step 1: upload a CSV (≤ 1 MB, ≤ 1000 rows), parsed on the server with the `csv` crate. Step 2: pick the phone column; the rest are variables. Step 3: write a `{{column}}` template with a preview of the first 5 rows and the segment count. Step 4: confirm, which calls the batch endpoint internally. The parsed CSV is kept in memory under a random token for 30 min, so there are no temp files. |
| `/messages` | Unified history. Filters (direction, state, device, peer, q, date range) live in the query string. Keyset "Load more" via htmx. |
| `/messages/{id}` | Detail view with the timeline (timestamps), error, webhook deliveries for the message, and a Reply form for the peer. |
| `/devices` | List. "Pair a phone" shows the QR code (inline SVG) plus the copyable link, with a countdown of the code's expiry; the page polls until a device pairs. Per device: rename, set default, enable/disable, send delay, preferred SIM, a forwarding switch with the SMS filter editor (mode plus rule rows) under it, remove (with confirmation; closes the socket with 4003). |
| `/keys` | Create (secret shown once with a copy button), rename, revoke, delete. |
| `/webhooks` | Create, edit, delete, rotate secret, test. `/webhooks/{id}` shows the deliveries log, paged, with the payload, status and error, and a redeliver button. |
| `/settings` | Login phone (set with a texted code, or remove); change password; list sessions and revoke them. |
| `/events` | SSE stream. Event types: `device` (re-render device card) and `message` (re-render a row or counts). |
| `/static/*` | Embedded assets, `Cache-Control: public, max-age=31536000, immutable`, with content-hashed names. |

### Rules
- Every form also works without JavaScript through full-page POSTs. htmx is a progressive enhancement.
- The page includes a short curl example using the public API, under `/keys`.
- **Security headers** on every response, unless the handler set them: `X-Frame-Options: DENY`, `X-Content-Type-Options: nosniff`, `Referrer-Policy: same-origin`, and `Content-Security-Policy: script-src 'self' https://challenges.cloudflare.com; frame-src https://challenges.cloudflare.com; frame-ancestors 'none'; object-src 'none'; base-uri 'none'`. There is no `default-src` or `style-src`, so inline styles and `data:` images keep working.
- **No inline script.** All page code lives in `app.js`, including the Turnstile `data-callback` (`rbTurnstileOk`, spec 005). The Turnstile loader is `defer`, so it runs after `app.js`.
- **htmx config:** `allowEval: false`, `allowScriptTags: false`, `selfRequestsOnly: true`. Templates use no `hx-on`, `js:` values or trigger filters, and swapped partials carry no `<script>`.

- **Unreachable devices** (spec 009): when `unreachable_at` is set, the dot is red and the card says "Not answering wake-ups since <time>. It was probably force-stopped or blocked by the phone's battery manager. Open the app on the phone." The same notice shows on the device's row on `/devices`.
- **Switched off on the phone**: when `gateway_off_at` is set, the dot is grey and the card says "Gateway switched off on the phone since <time>" instead of offline. No wake-ups are sent (spec 009).
- **Notifications off** (gms devices, those with an `fcm_token`): when `permissions.notifications` is false, the card's permission warnings include "Notifications are off on the phone, so wake-ups will not work".
- **Last restart** (from `lifecycle_json`, spec 002): "Last restart <relative time>: <start reason>", plus ", after <exit reason>" when an exit is known and ", was force-stopped" when `was_force_stopped` is true. Reasons show in plain words (`push` → "wake-up push", `job` → "watchdog", `boot_complete` → "phone boot", `low_memory` → "low memory", `user_requested` → "stopped from settings", other codes as written). The exit's `description`, when present, is the line's `title` tooltip. No line when `lifecycle_json` is NULL.

## Acceptance criteria
1. AC-010-01 Every listed route renders 200 with a session. Without one, `/login` renders and the others redirect.
2. AC-010-02 Sending from `/send` creates queued messages that are visible in `/messages`.
3. AC-010-03 The bulk flow with a 3-row CSV and the template `Hi {{name}}` creates 3 messages with substituted bodies.
4. AC-010-04 Pairing from `/devices` produces a link that the pairing API accepts.
5. AC-010-05 The total compressed weight of the embedded static assets is under 40 KB.
6. AC-010-06 The layout has no horizontal scroll at 375 px width.
7. AC-010-07 UI responses carry the four security headers above; the login page has no inline `<script>` and its htmx config sets `allowEval`, `allowScriptTags` and `selfRequestsOnly` as above.
8. AC-010-08 A device with `unreachable_at` set renders the unreachable notice on `/` and `/devices`; clearing it removes the notice. One with `gateway_off_at` set renders the switched-off state, and a gms device with notifications off renders the wake-up warning. A device with `lifecycle_json` renders the "Last restart" line with the mapped reasons.
9. AC-010-09 A device whose telemetry has `battery.charging` true shows a labelled charging bolt inside its battery icon on `/` and `/devices`; one that is not charging shows none.
10. AC-010-10 The dashboard refreshes on every burst of `device` and `message` events, ending with a refresh after the burst's last event. Events inside the one-second window are coalesced, never dropped, so a reconnect (close, `hello`, `health`) never leaves a card on an earlier state.

11. AC-010-11 A phone whose SIM list was read with `read_phone_state` granted (a `hello` sent with it, or a `health` carrying `sims`) and is empty shows a red "No SIM" in its card's status bar in place of the cellular indicator, and a warning "No SIM: this phone cannot send or receive SMS", on `/` and `/devices`. A phone with a SIM, or one that can't read its SIMs, shows neither.

## Test plan
| AC | Test |
|---|---|
| 01–04 | `server/tests/ui.rs`, HTTP-level checks on status, redirects and key markup |
| 05 | Test that sums the gzip sizes of the embedded files |
| 06 | Manual check in the browser pane during M7 |
| 07 | `server/tests/ui.rs` |
| 08 | `server/tests/ui.rs::ac_010_08_unreachable_and_last_restart` |
| 09 | `server/tests/ui.rs::ac_010_09_charging_bolt` |
| 10 | `server/tests/ui.rs::ac_010_10_live_refresh_is_trailing` checks the markup; the coalescing in `app.js` was checked in the browser pane |
| 11 | `server/tests/ui.rs::ac_010_11_no_sim_indicator` |

## Assumptions
- No command palette and no charts.
- Webhook events are picked with one checkbox per event; at least one is required.
- The SMS filter editor is a mode select plus one row per rule: field (sender, message, either), match (equals, contains, starts with, ends with), value and a case toggle. Rows post as repeated `rule_field`/`rule_op`/`rule_value`/`rule_case` keys; blank-value rows are dropped. Without JS one spare blank row is rendered; with JS rows are added and removed in place. "Forward only matching" with no rules shows a warning that nothing is forwarded.
- The forwarding switch can turn forwarding off but not on (spec 002); while it is off the switch is disabled with the hint "Only the phone can turn forwarding back on", and the filter editor is disabled and keeps its stored values. Unticking the switch disables the filter editor in place with JS.
- "Polls until a device pairs" is `GET /devices/paired?n=<count>` every 3 s via htmx; it answers `204`, or `HX-Refresh: true` once the device count grows.

## Open questions
- AC-010-11 (no-SIM indicator) was built on a direct request on 2026-10-20 without a separate spec review.
