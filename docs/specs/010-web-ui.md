# 010 — Web UI

Status: implemented
Updated: 2026-10-17

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
| `/` | Device cards: name, online dot, battery and charging, network, last seen, outbox depth, permission warnings. Today's counts: sent, delivered, failed, received, queued. Recent 10 messages. Updated live over SSE. |
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

## Acceptance criteria
1. AC-010-01 Every listed route renders 200 with a session. Without one, `/login` renders and the others redirect.
2. AC-010-02 Sending from `/send` creates queued messages that are visible in `/messages`.
3. AC-010-03 The bulk flow with a 3-row CSV and the template `Hi {{name}}` creates 3 messages with substituted bodies.
4. AC-010-04 Pairing from `/devices` produces a link that the pairing API accepts.
5. AC-010-05 The total compressed weight of the embedded static assets is under 40 KB.
6. AC-010-06 The layout has no horizontal scroll at 375 px width.
7. AC-010-07 UI responses carry the four security headers above; the login page has no inline `<script>` and its htmx config sets `allowEval`, `allowScriptTags` and `selfRequestsOnly` as above.

## Test plan
| AC | Test |
|---|---|
| 01–04 | `server/tests/ui.rs`, HTTP-level checks on status, redirects and key markup |
| 05 | Test that sums the gzip sizes of the embedded files |
| 06 | Manual check in the browser pane during M7 |
| 07 | `server/tests/ui.rs` |

## Assumptions
- No command palette and no charts.
- Webhook events are picked with one checkbox per event; at least one is required.
- The SMS filter editor is a mode select plus one row per rule: field (sender, message, either), match (equals, contains, starts with, ends with), value and a case toggle. Rows post as repeated `rule_field`/`rule_op`/`rule_value`/`rule_case` keys; blank-value rows are dropped. Without JS one spare blank row is rendered; with JS rows are added and removed in place. "Forward only matching" with no rules shows a warning that nothing is forwarded.
- The forwarding switch can turn forwarding off but not on (spec 002); while it is off the switch is disabled with the hint "Only the phone can turn forwarding back on", and the filter editor is disabled and keeps its stored values. Unticking the switch disables the filter editor in place with JS.
- "Polls until a device pairs" is `GET /devices/paired?n=<count>` every 3 s via htmx; it answers `204`, or `HX-Refresh: true` once the device count grows.

## Open questions
