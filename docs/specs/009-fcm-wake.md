# 009 — FCM wake (optional)

Status: implemented
Updated: 2026-10-16

## Context
A foreground service with a battery-optimization exemption normally keeps the socket alive. FCM is an optional extra that can revive the app after the OS or the user has stopped it. It is used only to wake the app and never carries message content.

## Design

### Server
- Enabled only when `RUSTYBEE_FCM_CREDENTIALS` points to a service-account JSON file with `project_id`, `client_email` and `private_key`.
- **Access token:**
  - Sign an RS256 JWT with `iss=client_email`, `scope=https://www.googleapis.com/auth/firebase.messaging`, `aud=https://oauth2.googleapis.com/token`, `iat` and `exp = iat + 3600`.
  - Exchange it at the token endpoint and cache the access token until 5 min before it expires.
- **Wake rule**, evaluated every 60 s and whenever a message is queued for an offline device. A device gets a wake when all of these hold:
  - it is `connected=0`;
  - it has an `fcm_token`;
  - it has at least one `queued` message, or an unacked `dispatched` one;
  - `last_seen_at` is more than 3 min ago (or NULL);
  - `last_wake_at` is more than 15 min ago (or NULL).
- **Request:** `POST https://fcm.googleapis.com/v1/projects/{project_id}/messages:send` with:
  ```json
  {"message":{"token":"…","data":{"t":"wake"},"android":{"priority":"HIGH","ttl":"600s"}}}
  ```
- **Result:**
  - Set `last_wake_at=now` on any attempt.
  - On a 404 or a `UNREGISTERED` error, set `fcm_token=NULL`.
  - Other errors are logged and nothing else happens.

### App (`gms` flavor only)
- `WakeService : FirebaseMessagingService`:
  - `onMessageReceived` with `t=wake` calls `startForegroundService(GatewayService)`. A high-priority FCM message grants a temporary exemption to start an FGS from the background.
  - `onNewToken` stores the token and sends a `health`/`hello` with it on the next connect.
- The `foss` flavor has no Firebase code or dependency. In that flavor `fcm_token` is always null.

## Acceptance criteria
1. AC-009-01 With no credentials, nothing FCM-related runs and the server makes no outbound calls.
2. AC-009-02 The wake rule fires once for an eligible device and does not fire again within 15 min. Tested against a mock FCM endpoint.
3. AC-009-03 An `UNREGISTERED` response clears the token.
4. AC-009-04 The foss APK contains no `com.google.firebase` classes.
5. AC-009-05 On the gms flavor, an FCM wake starts the gateway while the app is in the background. This is a manual check that needs Firebase credentials and is skipped if they are absent.

## Test plan
| AC | Test |
|---|---|
| 01–03 | `server/tests/fcm.rs` with the base URLs overridden to a local mock |
| 04 | CI: `apkanalyzer dex packages` grep |
| 05 | Manual end-to-end check |

## Assumptions
- The OAuth and FCM base URLs can be configured, but only for tests.

## Open questions
