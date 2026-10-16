# Specs

Spec-driven development with a soft gate: see `CLAUDE.md`. The template is `_template.md`.

| Spec | Scope | Status |
|---|---|---|
| [000](000-architecture.md) | Architecture, principles, footprint budgets | implemented |
| [001](001-repo-and-ci.md) | Repo layout, CI, licence | implemented |
| [002](002-device-protocol.md) | Device protocol, pairing, WebSocket | implemented |
| [003](003-data-model.md) | SQLite schema, writer actor, backup | implemented |
| [004](004-message-lifecycle.md) | State machine, expiry, segments | implemented |
| [005](005-auth.md) | Admin login (texted code, password fallback), sessions, API keys | implemented |
| [006](006-dispatch.md) | Dispatcher, pacing, replay | implemented |
| [007](007-public-api.md) | `/api/v1` | implemented |
| [008](008-webhooks.md) | Webhooks | implemented |
| [009](009-fcm-wake.md) | Optional FCM wake | implemented |
| [010](010-web-ui.md) | Web UI | implemented |
| [011](011-android-architecture.md) | Android architecture | implemented |
| [012](012-android-sms.md) | Android SMS send and receive | implemented |
| [013](013-android-ui.md) | Android UI | implemented |
| [014](014-deploy.md) | Deploy | implemented |

## Assumptions to review

Every choice made without asking during the one-shot run. Each one is also listed in its own spec.

- **000** One crate on the server (not a workspace) until build times say otherwise.
- **001** The worktree lives at `../rustybee`, a sibling of the legacy checkout.
- **001** The `gms` flavor is built in CI only when a `GOOGLE_SERVICES_JSON_RUSTYBEE` secret exists.
- **002** Ping frames come from the app only. The server never sends pings, which saves server-initiated wakeups on the phone.
- **002** The default `ping_s` is 270, set with `RUSTYBEE_PING_SECONDS`. Use 60 behind Cloudflare.
- **003** Message bodies are stored in plain text. The database file is protected by filesystem permissions, with no at-rest encryption.
- **003** No automatic retention. Messages are kept until the user deletes them; a prune setting may come later.
- **004** No user-facing cancel in the first release. The `cancel` transition exists only for deleting a device that still has queued messages.
- **005** Single admin with no username field.
- **006** A window of 1 per device is enough throughput for personal use. Carriers throttle anyway, and AOSP prompts after 30 SMS in 30 minutes.
- **007** No scheduled sends in the first release (non-goal).
- **007** `body` is limited to 1600 characters, about 10 GSM segments.
- **008** No automatic disabling of failing webhooks.
- **009** The OAuth and FCM base URLs can be configured, but only for tests.
- **010** No command palette and no charts.
- **011** Robolectric is not used, to keep the test toolchain light. Code that needs the Android framework is kept thin.
- **011** The token is encrypted with the Keystore. Other settings are stored in plain SharedPreferences.
- **012** Delivery reports are always requested. They cost nothing in battery, and carriers that don't support them simply never report.
- **013** No in-app QR scanner. The phone's camera app opens the `rustybee://` link from the QR code, and pasting the link is the fallback.
- **014** The server terminates no TLS itself. A reverse proxy or tunnel is expected for anything outside the LAN.

## M7 verification (2026-10-16)

Emulator `Pixel_10` (API 37), local server binary, foss build.

| Check | Result |
|---|---|
| Server tests | 97 pass; fmt and clippy clean |
| Android unit tests | 37 pass; lint clean (warnings only) |
| Pair through the `rustybee://` deep link (http, insecure box ticked) | connected |
| API send → `sent` → `delivered` | ok |
| `adb emu sms send` → history + `message.received` webhook | ok; HMAC signature valid |
| 80 s offline: 2 sends queued on server, 1 inbound queued on phone | all replayed after reconnect |
| Reboot | FGS restarted on `BOOT_COMPLETED` (about 2 min after boot on the emulator), next send delivered |
| Web UI at 375 px | no horizontal scroll on any page |
| Docker image | 7.9 MB on disk, healthy, uid 10001 |
| Server idle RSS | 9–12 MB in Docker (no device); 20 MB on macOS with one device |
| App PSS, release build, connected, in background | 23 MB |
| App PSS, debug build | 53 MB (not the budget build) |
| Foss release APK | 633 KB, no Firebase, AppCompat or Compose classes |
| App wake locks while idle | none held |

Not run: FCM wake end to end (no `google-services.json` or service account yet), Doze/app-standby policy checks, a real phone, release signing (no keystore yet).
