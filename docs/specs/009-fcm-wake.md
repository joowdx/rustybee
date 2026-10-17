# 009 — FCM wake (optional)

Status: implemented
Updated: 2026-10-17

## Context
The gateway must keep working after the OS kills the app, with the phone's battery setting left at the default ("Optimized"), without the battery-optimization exemption.

Without the exemption, neither a `START_STICKY` restart nor the `JobScheduler` watchdog (spec 011) may start a foreground service from the background. Boot is still an exemption, and so is a high-priority FCM message. So on the gms flavor, FCM is the only remote way back after a kill. It is used only to wake the app and never carries message content.

The first version woke a phone only when messages were waiting for it, and never checked whether the wake worked. This revision:
- also wakes idle phones;
- backs off when wakes go unanswered;
- protects the high-priority status FCM can take away;
- flags a phone that does not answer as unreachable.

## Goals / Non-goals
- Goal: with default battery settings, a phone the OS killed comes back within minutes, whether or not work is waiting.
- Goal: every wake FCM delivers ends in a visible notification, so FCM keeps treating the app's wakes as high priority.
- Goal: the admin can see when a phone cannot be revived remotely.
- Non-goal: reviving a force-stopped app, including one an OEM task killer stopped. Android delivers no FCM message to an app in the stopped state until the user opens it, so nothing remote can fix this. The unreachable flag exists to tell the admin.
- Non-goal: the foss flavor. Without FCM or the exemption, it comes back only on boot, on an update, or when the user taps the watchdog's "Gateway stopped" notification.

## Platform facts this design relies on
Facts here were checked against the Android and Firebase docs as far as they go. Anything marked unverified needs the real-phone check (AC-009-12).
- A high-priority FCM message lets the app start a foreground service from the background for a short window, about 10 s. The service must call `startForeground` within it.
- **High priority can be taken away.** FCM may downgrade an app's high-priority messages to normal priority when they do not lead to a user-visible notification, or when the app's notifications are turned off. A normal-priority message cannot start a foreground service. Whether an ongoing `IMPORTANCE_LOW` service notification counts as user-visible is not documented (unverified).
- **High-priority wakes are capped in low App Standby buckets.** In `restricted` the cap is about 5 per day; the exact caps for `frequent` and `rare` are not published. A running foreground service keeps the app active, but a dead gateway the user never opens can sink to `rare` within days.
- **Doze may cut the socket (unverified).** Without the exemption, Doze may cut network access even while the service runs. If it does, a dozing phone disconnects, is woken, and disconnects again. AC-009-12 settles this before implementation.

## Design

### Server
- Enabled only when `RUSTYBEE_FCM_CREDENTIALS` points to a service-account JSON file with `project_id`, `client_email` and `private_key`.
- **Access token:**
  - Sign an RS256 JWT with `iss=client_email`, `scope=https://www.googleapis.com/auth/firebase.messaging`, `aud=https://oauth2.googleapis.com/token`, `iat` and `exp = iat + 3600`.
  - Exchange it at the token endpoint and cache the access token until 5 min before it expires.
- **Wake rule**, evaluated every 60 s and whenever a message is queued for an offline device. A device is due a wake when all of these hold:
  - it is not removed, `enabled=1`, `connected=0`, `gateway_off_at` is NULL, and it has an `fcm_token`;
  - it has been quiet long enough: `last_seen_at` (or `created_at` if NULL) is more than **3 min** ago when it has a `queued` message or an unacked `dispatched` one, and more than **5 min** ago otherwise;
  - the backoff has passed: `last_wake_at` is NULL, or more than `backoff(wake_attempts)` ago.
- **Backoff:** `backoff(n)` doubles from 15 min and stops at 8 h: 15 min for n ≤ 1, then 30 min, 1 h, 2 h, 4 h, and 8 h for n ≥ 6.
  - Unanswered wakes go out at +0, +15, +45, +105, +225 and +465 min, then every 8 h.
  - There is no give-up. Three wakes a day stays under the `restricted` bucket's cap, so a wake still lands at high priority once the cause clears, for example after the user opens the app.
- **Request:** `POST https://fcm.googleapis.com/v1/projects/{project_id}/messages:send` with:
  ```json
  {"message":{"token":"…","data":{"t":"wake"},
   "android":{"priority":"HIGH","ttl":"3600s","collapse_key":"wake"}}}
  ```
  - `collapse_key` keeps at most one wake waiting at FCM per phone.
  - The 1 h TTL lets a wake sent while the phone had no network still arrive when it comes back.
- **Result:**
  - Any attempt sets `last_wake_at=now`, so a failing FCM call is retried no more often than the current backoff.
  - A wake FCM accepts (2xx) also increments `wake_attempts`.
  - On a 404 or an `UNREGISTERED` error, set `fcm_token=NULL`.
  - Other errors are logged and nothing else happens.
- **Gateway switched off on the phone** (`gateway_off_at`):
  - When the user switches the gateway off, the app closes the socket with close code **4010 `gateway_off`** (spec 002). The server sets `gateway_off_at=now` and never wakes the device while it is set.
  - The next `hello` clears it.
  - Without this, every wake to a switched-off phone would end with no notification, which is the pattern that gets high priority taken away.
- **Unreachable flag** (`unreachable_at`):
  - Set by the same 60 s check, to `last_wake_at`, when all of these hold:
    - `wake_attempts ≥ 1` and `unreachable_at` is NULL;
    - `connected=0`;
    - `last_wake_at` is more than **2 min** ago, with no `hello` since (`last_seen_at` NULL or older than `last_wake_at`).
  - Setting it emits a `device` SSE event so the card re-renders (spec 010).
  - Cleared, together with `wake_attempts=0`, by the `hello` that reconnects the device (spec 002).
  - A device without an `fcm_token` (the foss flavor, or a token FCM unregistered) is never flagged. It shows as offline only.
- **Token updates:** the server stores `fcm_token` from `hello` and from `health` (spec 002), so a rotated token takes effect without waiting for a reconnect.
- **Constants:** these live in `fcm.rs`, not configuration: quiet 3/5 min, backoff 15 min doubling to 8 h, answer window 2 min, TTL 1 h.

### App (`gms` flavor only)
- **`WakeService : FirebaseMessagingService`**, on a `t=wake` message:
  - **Paired and gateway enabled:** call `startForegroundService(GatewayService)`, which calls `startForeground` at once in `onStartCommand`.
    - If the start or the `startForeground` call throws (for example `ForegroundServiceStartNotAllowedException`, when the message arrived downgraded), post the watchdog's high-importance "Gateway stopped — tap to restart" notification instead (spec 011).
    - Either way, a delivered wake always ends in a visible notification.
  - **Not paired, or the gateway is off:** do nothing. The server should not have sent the wake (`gateway_off_at`), and this case is logged.
- **Token handling:**
  - The app opts in to FCM's `register()` flow (`firebase_messaging_installation_id_enabled` meta-data; Play services older than 26.12 fall back to the legacy token RPC inside the SDK). `onRegistered` stores the token. If the connection is welcomed, it also sends a `health` carrying `fcm_token` at once. A token that rotates after `hello` but before `welcome` is sent in a `health` right after `welcome`. Otherwise the token goes in the next `hello`.
  - Pairing calls `FirebaseMessaging.register()` and waits up to 5 s for the `onRegistered` token before sending the pair request, so a fresh pairing usually starts with a token.
- **Notifications are required on gms.**
  - FCM downgrades wakes for an app whose notifications are off. A wake also needs the gateway notification channel to be able to show its notification.
  - So `permissions.notifications` (spec 002) is false when either `POST_NOTIFICATIONS` is denied or the gateway notification channel is blocked.
  - The phone's checklist (spec 013) and the web card (spec 010) both warn that "Wake-ups will not work while notifications are off".
- The `foss` flavor has no Firebase code or dependency. In that flavor `fcm_token` is always null.
- Why the app last died and how it was started are reported in `hello.lifecycle` by both flavors (specs 002 and 011). A wake that works shows up as `start.reason="push"`.

## Acceptance criteria
1. AC-009-01 With no credentials, nothing FCM-related runs and the server makes no outbound calls.
2. AC-009-02 A device with a queued message and quiet for over 3 min gets one wake, and no second one within 15 min. Tested against a mock FCM endpoint.
3. AC-009-03 An `UNREGISTERED` response clears the token.
4. AC-009-04 The foss APK contains no `com.google.firebase` classes.
5. AC-009-05 On the gms flavor, an FCM wake starts the gateway while the app is in the background. This is a manual check that needs Firebase credentials and is skipped if they are absent.
6. AC-009-06 An idle device (nothing queued) gets no wake while quiet for under 5 min, and one wake once quiet for over 5 min.
7. AC-009-07 Unanswered wakes back off: with the clock advanced, wakes go out at +0, +15, +45, +105, +225 and +465 min, then every 8 h.
8. AC-009-08 A device that has not sent `hello` 2 min after an accepted wake gets `unreachable_at` set and a `device` event. A failed FCM call never sets it.
9. AC-009-09 A `hello` clears `unreachable_at` and `gateway_off_at` and resets `wake_attempts` to 0, so the next wake after a later disconnect is not backed off.
10. AC-009-10 Disabled, removed, token-less and switched-off (`gateway_off_at` set) devices are never woken or flagged.
11. AC-009-11 The wake request carries `priority=HIGH`, `ttl=3600s` and `collapse_key=wake`. A `health` carrying `fcm_token` replaces the stored token.
12. AC-009-12 Real-phone check, gms flavor, default battery setting, notifications allowed. It runs before implementation on the current build, to settle Doze, and again before release.
    - Screen off, then `adb shell dumpsys deviceidle force-idle`.
    - Idle socket in Doze: does the socket stay up for 30 min? If not, record how the disconnect and wake cycle behaves.
    - `adb shell am kill dev.joowdx.rustybee`, once with nothing queued and once with a message queued. The gateway reconnects within the quiet time plus 2 min, and its `hello` reports `start.reason="push"`.
    - With the gateway switched off on the phone, no wake is sent.
13. AC-009-13 A wake whose start throws posts the "Gateway stopped — tap to restart" notification. Manual check: send a normal-priority wake to a non-exempt app in Doze.

## Test plan
| AC | Test |
|---|---|
| 01–03, 06–11 | `server/tests/fcm.rs` with the base URLs overridden to a local mock and an injected clock |
| 04 | CI: `apkanalyzer dex packages` grep |
| 05, 12, 13 | Manual check on a real phone |

## Assumptions
- The OAuth and FCM base URLs can be configured, but only for tests.
- Waking idle phones costs one high-priority FCM message per offline spell, plus backoff. FCM is free, and the phone does little more than reconnect.
- If the user switches the gateway off while the phone is offline, the server never sees close code 4010 and keeps waking the phone. The backoff brings this down to three wakes a day, and the next `hello` puts things right. This is accepted rather than adding an offline "off" queue.
- When the phone loses its network without closing the socket, the server notices only after `idle_timeout` (2 × `ping_s` + 30 s, about 9.5 min at the default). A process the OS kills closes its socket at once, so this only delays wakes after network loss.
- The OEM "autostart" or app-killer settings on some brands put the app in the stopped state. The unreachable flag reports it. Guiding users through those settings is a separate change (spec 013).

## Open questions
- If AC-009-12 shows that Doze cuts the socket without the exemption, the default-battery goal needs a rethink. One option is to accept the wake cycle during Doze. Another is to ask for the exemption after all, as a recommendation rather than a requirement.
- Should `unreachable_at` also fire a webhook (for example `device.unreachable`) so the admin is told without opening the web UI? Webhook payloads are Message objects today (spec 008), so this needs a new payload shape.
- Should the gms flavor stop asking for the battery-optimization exemption, since this design assumes it is not granted? Granting it only makes revival stronger: the watchdog and sticky restarts work again.
- 2026-10-17: moved from the deprecated `getToken()`/`onNewToken` to `register()`/`onRegistered` (firebase-messaging 26). Not yet checked on a gms device.
