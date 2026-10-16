# 010 — Web UI

Status: approved (self, one-shot)
Updated: 2026-10-16

## Design

### Stack
- **Rendering:** askama templates, rendered on the server.
- **Interactivity:** htmx 2.x, vendored into `server/static/` and embedded with rust-embed. Live updates use htmx's SSE extension.
- **Front-end code:** one `app.css` (hand-written, under about 12 KB) and one `app.js` (segment counter, recipient chips, copy buttons; under about 6 KB).
- **No external requests.** No CDN, no web fonts; the system font stack only.
- **Themes:** light and dark through `prefers-color-scheme`.
- **Layout:** a top bar on desktop, collapsing to a bottom tab bar under 640 px.

### Pages
All pages except `/login` require a session (spec 005).

| Route | Content |
|---|---|
| `/login` | Password form. Shows the "set password" hint when none is set. |
| `/` | Device cards: name, online dot, battery and charging, network, last seen, outbox depth, permission warnings. Today's counts: sent, delivered, failed, received, queued. Recent 10 messages. Updated live over SSE. |
| `/send` | Recipient chip input. The body textarea shows encoding, characters, parts and remaining. Device and SIM selects. Submits via htmx and shows the result inline. |
| `/bulk` | Step 1: upload a CSV (≤ 1 MB, ≤ 1000 rows), parsed on the server with the `csv` crate. Step 2: pick the phone column; the rest are variables. Step 3: write a `{{column}}` template with a preview of the first 5 rows and the segment count. Step 4: confirm, which calls the batch endpoint internally. The parsed CSV is kept in memory under a random token for 30 min, so there are no temp files. |
| `/messages` | Unified history. Filters (direction, state, device, peer, q, date range) live in the query string. Keyset "Load more" via htmx. |
| `/messages/{id}` | Detail view with the timeline (timestamps), error, webhook deliveries for the message, and a Reply form for the peer. |
| `/devices` | List. "Pair a device" shows the QR code (inline SVG) plus the copyable link, with a countdown of the code's expiry; the page polls until a device pairs. Per device: rename, set default, enable/disable, send delay, receive toggle, preferred SIM, SMS filters editor, remove (with confirmation; closes the socket with 4003). |
| `/keys` | Create (secret shown once with a copy button), rename, revoke, delete. |
| `/webhooks` | Create, edit, delete, rotate secret, test. `/webhooks/{id}` shows the deliveries log, paged, with the payload, status and error, and a redeliver button. |
| `/settings` | Change password; list sessions and revoke them. |
| `/events` | SSE stream. Event types: `device` (re-render device card) and `message` (re-render a row or counts). |
| `/static/*` | Embedded assets, `Cache-Control: public, max-age=31536000, immutable`, with content-hashed names. |

### Rules
- Every form also works without JavaScript through full-page POSTs. htmx is a progressive enhancement.
- The page includes a short curl example using the public API, under `/keys`.

## Acceptance criteria
1. AC-010-01 Every listed route renders 200 with a session. Without one, `/login` renders and the others redirect.
2. AC-010-02 Sending from `/send` creates queued messages that are visible in `/messages`.
3. AC-010-03 The bulk flow with a 3-row CSV and the template `Hi {{name}}` creates 3 messages with substituted bodies.
4. AC-010-04 Pairing from `/devices` produces a link that the pairing API accepts.
5. AC-010-05 The total compressed weight of the embedded static assets is under 40 KB.
6. AC-010-06 The layout has no horizontal scroll at 375 px width.

## Test plan
| AC | Test |
|---|---|
| 01–04 | `server/tests/ui.rs`, HTTP-level checks on status, redirects and key markup |
| 05 | Test that sums the gzip sizes of the embedded files |
| 06 | Manual check in the browser pane during M7 |

## Assumptions
- No command palette and no charts.
- Webhook events are picked with one checkbox per event; at least one is required.
- The SMS filter editor is a textarea with one rule per line: `<field> <op> <value>` (for example `sender equals +15550000009`), plus mode and ignore-case controls.
- "Polls until a device pairs" is `GET /devices/paired?n=<count>` every 3 s via htmx; it answers `204`, or `HX-Refresh: true` once the device count grows.

## Open questions
