# 000 — Architecture

Status: implemented
Updated: 2026-10-16

## Context
rustybee replaces a NestJS/Mongo/Redis/Mosquitto/Next.js stack and a legacy Android app. The reasons are in `docs/rewrite/01-audit.md`. It is for personal, single-user use.

## Goals / Non-goals
- Goals:
  - one server process and one SQLite file;
  - a phone app that is idle unless it is sending, receiving or reconnecting;
  - correct message state;
  - commands from server to phone delivered within seconds.
- Non-goals:
  - multi-user or multi-tenant use;
  - billing, registration, email, analytics;
  - horizontal scaling;
  - MMS or RCS;
  - acting as the default SMS app.

## Design
```
 API client ──HTTPS──▶ ┌──────────── rustybee (Rust) ────────────┐
 Browser ───HTTPS────▶ │ http/api  http/ui(askama+htmx)  http/device│
                       │ hub (WS registry, dispatcher, SSE bus)     │
                       │ webhooks worker   fcm (optional)  sweeper  │
                       │ store (SQLite WAL, 1 writer)  domain (pure)│
                       └───────────────▲───────────────────────────┘
                                       │ WebSocket (JSON envelope, acks)
                              ┌────────┴────────┐  FCM wake (gms, optional)
                              │ Android app     │◀──────────────────────
                              │ GatewayService  │
                              │ (FGS remoteMsg) │── SmsManager / SMS_RECEIVED
                              └─────────────────┘
```
- **Transport:** one WebSocket per device (spec 002). SQLite is the durable queue on the server, and a local SQLite outbox does the same on the phone. There is no broker.
- **Server modules:** `domain` (pure), `store`, `hub`, `webhooks`, `fcm`, `http/{api,device,ui}`, `config`, and `main` (CLI).
- **Android:** a single `GatewayService` foreground service of type `remoteMessaging` owns the socket, sending and the uplink. Receivers only hand work to it.
- **Tech:**
  - Rust (stable) with axum, tokio, rusqlite (bundled), askama, rust-embed, reqwest (rustls), argon2, hmac and sha2.
  - Kotlin with OkHttp 5, kotlinx.serialization and coroutines, on platform Views.

## Footprint budgets
| Item | Budget |
|---|---|
| Server idle RSS | < 20 MB |
| Container image | < 15 MB |
| App PSS while connected | < 40 MB |
| Release APK (foss) | < 3 MB |
| Wakelocks held outside sends | none |

## Acceptance criteria
1. AC-000-01 The server is one static binary that needs nothing at runtime except a writable data directory.
2. AC-000-02 No source file, resource or package id contains `textbee` or `vernu`. The credit note in the root `README.md` is the one exception.
3. AC-000-03 The footprint budgets above are met (measured in M7).

## Test plan
| AC | Test |
|---|---|
| 01 | `deploy/Dockerfile` builds a `FROM scratch` image; the container starts |
| 02 | Verified once at cutover with `git grep -nIiE 'textbee\|vernu' -- . ':!docs' ':!CLAUDE.md' ':!README.md'` |
| 03 | Manual measurement, recorded in `docs/specs/README.md` |

## Assumptions
- One crate on the server (not a workspace) until build times say otherwise.

## Open questions
