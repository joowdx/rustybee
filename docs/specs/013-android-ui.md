# 013 — Android UI

Status: implemented
Updated: 2026-10-16

## Design

### General
- Platform `Activity` with Views built in code or in small XML layouts, using platform `android:Theme.DeviceDefault.DayNight` or a Material-free `Theme.Material` derivative.
- Edge-to-edge: handle `WindowInsets` on the root view.
- Back: `onBackInvokedDispatcher` where custom behaviour is needed; otherwise the default.
- The app supports adaptive layouts and doesn't lock the orientation.

### `SetupActivity`
- Shown when the app isn't paired.
- **Entry:**
  - the deep link `rustybee://pair?u=…&c=…` (intent filter: scheme `rustybee`, host `pair`, `BROWSABLE`);
  - or a text field to paste the link;
  - an "Allow insecure http (LAN)" checkbox, shown only for `http://` URLs.
- **Pairing:** calls the pairing API from spec 002 and shows errors inline. On success it stores the credentials and opens `MainActivity`.

### `MainActivity`
A single scrolling screen with these sections:
1. **Status:**
   - connection state with a dot;
   - server host;
   - device name;
   - last connected time;
   - outbox size;
   - sends and receives today;
   - a gateway on/off `Switch`, which starts or stops the service and sets prefs.
2. **Health checklist:** one row per item, each with a ✓/✗ and an action button.
   - SMS permissions (send, receive, phone state);
   - notifications;
   - battery unrestricted (`isIgnoringBatteryOptimizations`);
   - "If permissions are greyed out" help, which explains App info → ⋮ → Allow restricted settings and opens app details;
   - (gms) FCM token present.
3. **SIMs:** a list of active subscriptions (read-only). The preferred SIM is set in the web UI.
4. **Log:** the last 100 entries, monospace, with Copy and Clear buttons.
5. **Unpair:** asks for confirmation, wipes the credentials and the db, stops the service and opens Setup.

Live updates come from binding to `GatewayService` and collecting its `StateFlow` while the activity is started.

### Permissions
Requested in one runtime request, with a short rationale, when the gateway is first enabled: `SEND_SMS`, `RECEIVE_SMS`, `READ_PHONE_STATE`, `POST_NOTIFICATIONS`.

### Copy and branding
- App label `rustybee`.
- The icon is a raster adaptive icon of the rusty bee: the bee as the foreground, a rust honeycomb as the background, plus a monochrome layer for themed icons. The notification icon is the same white silhouette. `branding/generate-icons.sh` builds every density from `branding/bee.png`.

## Acceptance criteria
1. AC-013-01 Opening the deep link on the emulator pairs the app, shows the main screen and connects. Manual check.
2. AC-013-02 The checklist reflects the real state after each permission is granted or revoked. Manual check.
3. AC-013-03 There is no layout overlap with the system bars in portrait and landscape. Manual check, screenshots at 0.5 scale.
4. AC-013-04 Unpairing returns to Setup and the service stops.

## Test plan
Manual checks in M7, with screenshots attached to the M7 notes.

## Assumptions
- No in-app QR scanner. The phone's camera app opens the `rustybee://` link from the QR code, and pasting the link is the fallback.
- A deep link with an `https://` server pairs immediately; an `http://` one waits for the insecure checkbox and the Pair button.
- A successful pairing enables the gateway and opens `MainActivity`, which shows the permission rationale and request at once, so the deep link alone leads to a connected gateway (AC-013-01).

## Open questions
