# rustybee Android app

Native Kotlin SMS gateway client for the rustybee server. Platform Views, no AppCompat, Material,
Compose, Room, WorkManager or DI. Android 11 (API 30) or newer; targets API 36. Specs: `docs/specs/011`, `012`, `013`.

## Build

```sh
export JAVA_HOME="/Applications/Android Studio.app/Contents/jbr/Contents/Home"   # system JDK may be too new
echo "sdk.dir=$HOME/Library/Android/sdk" > local.properties
./gradlew testFossDebugUnitTest assembleFossDebug
./gradlew assembleFossRelease lintFossDebug
```

- `foss` (default): no Google code; `fcm_token` is always null.
- `gms`: adds `firebase-messaging` for the optional FCM wake (spec 009). Put your Firebase
  `google-services.json` in `app/src/gms/` first; without it any gms build stops with a clear message.

### Release signing

Create `keystore.properties` next to `settings.gradle.kts` (it is gitignored):

```properties
storeFile=/path/to/release.jks
storePassword=…
keyAlias=…
keyPassword=…
```

or set `RUSTYBEE_STORE_FILE`, `RUSTYBEE_STORE_PASSWORD`, `RUSTYBEE_KEY_ALIAS`, `RUSTYBEE_KEY_PASSWORD`.
With neither, `assembleFossRelease` produces an unsigned APK.

## Install and pair

1. `adb install app/build/outputs/apk/foss/debug/app-foss-debug.apk`
2. In the web UI, create a pairing code and open the QR code with the phone's camera
   (or paste the `rustybee://pair?u=…&c=…` link into the app).
3. Grant the SMS, phone state and notification permissions, and allow unrestricted battery use.
   If the SMS permissions are greyed out (sideloaded app): App info → ⋮ → Allow restricted settings.

## Sending limits (AOSP 30 SMS / 30 min prompt)

AOSP rate-limits apps that send SMS: by default, more than **30 messages within 30 minutes** from one
app triggers a system confirmation dialog on the phone, and the send waits (or fails with
`limit_exceeded`) until someone taps Allow. For an unattended gateway, raise the limit over adb:

```sh
adb shell settings put global sms_outgoing_check_max_count <n>
# optional: the window in ms (default 1800000 = 30 min)
adb shell settings put global sms_outgoing_check_interval_ms 1800000
```

The setting survives reboots but may be reset by a factory reset or some OEM updates. Check it with
`adb shell settings get global sms_outgoing_check_max_count`.

## Manual checks

- Inbound: `adb emu sms send +15551234567 "hello"` on an emulator.
- Wake locks while idle: `adb shell dumpsys power | grep -i rustybee` should show nothing.
- Stored state: `adb shell run-as dev.joowdx.rustybee ls databases` (debug builds).
