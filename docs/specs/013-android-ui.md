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
  - "Scan QR code", the primary button, which opens `ScanActivity`;
  - the deep link `rustybee://pair?u=…&c=…` (intent filter: scheme `rustybee`, host `pair`, `BROWSABLE`);
  - or a text field to paste the link;
  - an "Allow insecure http (LAN)" checkbox, shown only for `http://` URLs.
- **Pairing:** calls the pairing API from spec 002 and shows errors inline. On success it stores the credentials and opens `MainActivity`.

### `ScanActivity`
- A full-screen camera preview (platform Camera2 on a `TextureView`, back camera) with a hint and Cancel.
- Frames go through ZXing core's QR reader on a background thread. The first `rustybee://pair?…` text is returned to `SetupActivity`, which handles it like the deep link: an `https://` server pairs at once, an `http://` one waits for the insecure checkbox. Any other QR code shows "That QR code isn't a rustybee pairing code."
- `CAMERA` is requested when the screen opens. If it is refused, Setup says so and the paste field stays as the fallback.
- No camera, or a camera error, shows a message pointing to the paste field.

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

6. **Forwarding** button, which opens `ForwardingActivity`. The Device row reads Forwarding, Logs, Unpair.

Live updates come from binding to `GatewayService` and collecting its `StateFlow` while the activity is started.

### `ForwardingActivity`
- Edits whether this phone forwards received SMS, and its inbound filter (spec 002), on one screen:
  - a "Forward received SMS" switch at the top. While it is off, the mode picker and the rules are locked and the screen says nothing is forwarded;
  - a mode picker: off, forward only matching, forward all except matching;
  - rule rows, each with a field (sender, message, sender or message), a match (contains, equals, starts with, ends with), the text, an "Ignore case" box and Remove; plus "Add rule".
- Loads from the config the server last sent. Save sends `set_forwarding` with that config's `filters_rev` and shows Saving…, Saved, or the error.
- On `filters_conflict` the editor reloads from the new config and says "Changed on the web, reloaded."
- Turning the switch off saves at once, with no Save press: it applies on the phone immediately, even offline, and reaches the server now or when the phone reconnects (spec 002). Turning it on and editing the filter need Save, which is disabled while the gateway isn't connected. The phone keeps using its last config either way.
- The switch shows the phone's effective state: off when the server's config is off or the phone's local "off" is set.
- Warns when "forward only matching" has no rules (nothing is forwarded) and when rules exist while the filter is off.

### Permissions
Requested in one runtime request, with a short rationale, when the gateway is first enabled: `SEND_SMS`, `RECEIVE_SMS`, `READ_PHONE_STATE`, `POST_NOTIFICATIONS`. `CAMERA` is requested separately, only when scanning; the manifest declares `android.hardware.camera.any` as not required.

### Copy and branding
- App label `rustybee`.
- The icon is a raster adaptive icon of the rusty bee: the bee as the foreground, a rust honeycomb as the background, plus a monochrome layer for themed icons. The foreground is the sticker bee with its white border, flap lines and stinger swoosh. The monochrome layer and the notification icon are the plain white silhouette, without the sticker. `branding/generate-icons.sh` builds every density: the colour layers from `branding/bee.svg`, the monochrome ones from `branding/bee.png`.

## Acceptance criteria
1. AC-013-01 Opening the deep link on the emulator pairs the app, shows the main screen and connects. Manual check.
2. AC-013-02 The checklist reflects the real state after each permission is granted or revoked. Manual check.
3. AC-013-03 There is no layout overlap with the system bars in portrait and landscape. Manual check, screenshots at 0.5 scale.
4. AC-013-04 Unpairing returns to Setup and the service stops.
5. AC-013-05 Scanning the Devices-page QR from inside the app pairs it, in portrait and landscape. Refusing the camera leaves Setup usable with a message. Manual check.
6. AC-013-06 The Forwarding screen locks the filter while the switch is off, and turning the switch off while offline stops forwarding at once. Manual check.

## Test plan
Manual checks in M7, with screenshots attached to the M7 notes.

## Assumptions
- The phone's camera app can also open the `rustybee://` link from the QR code, and pasting the link is the fallback.
- The scanner uses Camera2 rather than CameraX, because CameraX needs AndroidX lifecycle (spec 011).
- A deep link with an `https://` server pairs immediately; an `http://` one waits for the insecure checkbox and the Pair button.
- A successful pairing enables the gateway and opens `MainActivity`, which shows the permission rationale and request at once, so the deep link alone leads to a connected gateway (AC-013-01).

## Open questions
