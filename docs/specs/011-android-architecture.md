# 011 — Android architecture

Status: implemented
Updated: 2026-10-17

## Design

### Build
- **Project:** a single `:app` module with `applicationId dev.joowdx.rustybee`, `namespace dev.joowdx.rustybee`, `compileSdk` and `targetSdk` 36, and `minSdk` 35 (Android 15).
  - Nothing in the app needs API 36. The floor is API 35 for `ApplicationStartInfo` (below); before that it would be API 34 for the `remoteMessaging` FGS type.
  - An API 36 call added later needs an `SDK_INT` check, which lint enforces.
  - Android 15 does not count jobs that run while an FGS is active against the job quota the way Android 16 does. Nothing here relies on either behaviour, but both versions are tested (below).
- **Toolchain:** Kotlin 2.x with JVM target 17, the current stable AGP, and a version catalog in `gradle/libs.versions.toml`.
- **Flavors:** dimension `dist` with `foss` (the default) and `gms`. Only `gms` applies the `com.google.gms.google-services` plugin, and only if `src/gms/google-services.json` exists; otherwise the gms build fails with a clear message.
- **Release build:**
  - `isMinifyEnabled = true`, `isShrinkResources = true`.
  - Signing comes from `keystore.properties` (`storeFile`, `storePassword`, `keyAlias`, `keyPassword`) or the environment variables `RUSTYBEE_STORE_FILE`, `RUSTYBEE_STORE_PASSWORD`, `RUSTYBEE_KEY_ALIAS`, `RUSTYBEE_KEY_PASSWORD`. If neither exists, the build is unsigned.
- **Dependencies:**
  - `com.squareup.okhttp3:okhttp` (5.x, pinned to 5.4.0: 5.5.0 requires compileSdk 37). It brings `androidx.annotation` and `androidx.startup` transitively, so `android.useAndroidX=true` is set;
  - `org.jetbrains.kotlinx:kotlinx-serialization-json`;
  - `org.jetbrains.kotlinx:kotlinx-coroutines-android`;
  - `com.google.zxing:core` for the in-app QR scanner (spec 013);
  - `gmsImplementation` `com.google.firebase:firebase-messaging` (without the BoM extras);
  - tests: junit and kotlinx-coroutines-test.

  Nothing else. In particular no AppCompat, Material, Compose, Room, WorkManager or AndroidX lifecycle.
- **No baseline profile** in the first release.

### Packages
| Package | Contents |
|---|---|
| `protocol` | `@Serializable` envelope and data classes for spec 002, plus a `Json` instance with `ignoreUnknownKeys` and `explicitNulls=false` |
| `data` | `Prefs` (SharedPreferences: server URL, device id, encrypted token, gateway enabled, config cache, fcm token). `Crypto` (Android Keystore AES-256-GCM key `rustybee_token`). `Db : SQLiteOpenHelper` (below). |
| `gateway` | `GatewayService`, `Connection`, `Uplink`, `Sender`, `SmsInboundReceiver`, `SmsStatusReceiver`, `BootReceiver`, `Watchdog`, `Telemetry` |
| `ui` | `SetupActivity`, `MainActivity`, small view helpers |
| `gms` (flavor source set) | `push.Push` (FCM token) and `push.WakeService` |
| `foss` (flavor source set) | `push.Push` stub returning a null token |

### Local database (`rustybee.db`, version 1)
```sql
CREATE TABLE outbox (seq INTEGER PRIMARY KEY AUTOINCREMENT, frame_id TEXT NOT NULL UNIQUE,
  type TEXT NOT NULL, json TEXT NOT NULL, created_at INTEGER NOT NULL);
CREATE TABLE sent (msg_id TEXT PRIMARY KEY, to_addr TEXT NOT NULL, state TEXT NOT NULL,
  parts INTEGER NOT NULL, parts_sent INTEGER NOT NULL DEFAULT 0, parts_failed INTEGER NOT NULL DEFAULT 0,
  parts_delivered INTEGER NOT NULL DEFAULT 0, error_code TEXT, error_message TEXT, updated_at INTEGER NOT NULL);
CREATE TABLE seen (frame_id TEXT PRIMARY KEY, at INTEGER NOT NULL);
CREATE TABLE log (id INTEGER PRIMARY KEY AUTOINCREMENT, at INTEGER NOT NULL, level TEXT NOT NULL, msg TEXT NOT NULL);
```
- `outbox` holds every device→server frame except `hello`, `health` and acks, until the server acks it.
- `seen` dedupes server frames by id and is pruned after 7 days.
- `sent` rows in a final state (`sent`, `delivered`, `failed`) are pruned once `updated_at` is more than 7 days old, at service start next to the `seen` prune. `accepted` rows are kept for `failLost`.
- `log` is capped at 1000 rows. Phone numbers are redacted to their last 4 digits, whether written with spaces, dashes, dots or parentheses (`555.123.4567`, `(555) 123-4567`, `+1 (555) 123-4567`).

### GatewayService
- A foreground service with `foregroundServiceType="remoteMessaging"` and the `FOREGROUND_SERVICE_REMOTE_MESSAGING` permission.
- **Notification:** channel `gateway`, `IMPORTANCE_LOW`, ongoing. It shows "Connected" or "Reconnecting…" and the count of sends today. Tapping it opens `MainActivity`.
- **Lifecycle:**
  - Started by the UI toggle, `BootReceiver`, `Watchdog`, `WakeService` and the receivers when work arrives.
  - `onStartCommand` calls `startForeground` before anything else and returns `START_STICKY`. Without the battery exemption a sticky restart may be refused (spec 009); it is kept because it costs nothing when it works.
  - It stops itself (`stopForeground` + `stopSelf`) when the gateway is disabled or unpaired. When the user switches the gateway off, it first closes the socket with 4010 `gateway_off` (spec 002), if open.
  - Android 14+ lets the user dismiss the notification. The service keeps running and re-posts the notification on the next state change.
- **Scope:** one `CoroutineScope(SupervisorJob() + Dispatchers.Default)`. The parts below share one `Mutex`-free design: frames are processed in a single coroutine fed by a `Channel`.
  - **`Connection`:**
    - OkHttp `WebSocket` with `pingInterval(config.ping_s)` (270 s until the first `welcome`) and a connect timeout of 15 s.
    - Backoff and resets follow spec 002. A generation counter discards callbacks from old sockets.
    - Reconnects when the `ConnectivityManager.registerDefaultNetworkCallback` reports onAvailable or onLost.
  - **On open:** send `hello` (built by `Telemetry`), then, after `welcome`, resend the outbox in `seq` order.
  - **`Uplink`:** inserts a frame into `outbox`, then sends it if the socket is open. An ack deletes the row by `frame_id`.
  - **`Sender`:** spec 012.
- **Wake locks:** a partial wake lock held only while parts are being handed to `SmsManager` (5 s timeout). None otherwise.
- **State exposed to the UI:** a `StateFlow<GatewayState>` (connection, last error, outbox size, sends and receives today) through a local `Binder`.

### Boot, watchdog and battery
- `BootReceiver`: on `BOOT_COMPLETED` and `MY_PACKAGE_REPLACED`, if paired and enabled, calls `startForegroundService`. `remoteMessaging` is not in the list of types banned from boot.
- `Watchdog`: a platform `JobScheduler` job, id 1, periodic every 15 min, with `NETWORK_TYPE_ANY` and persisted.
  - If the gateway is enabled and the service isn't running: when the app is battery-unrestricted, start the FGS; otherwise post a high-importance "Gateway stopped — tap to restart" notification (the alerts channel).
  - `WakeService` (gms) posts the same notification when its own start attempt throws (spec 009). `GatewayService.start` returns whether the start succeeded so callers can fall back.
  - The job finishes immediately.
- The UI asks for the battery-optimization exemption with `ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS`, and explains why.

### FCM token (gms)
- `Push.refresh` runs at service start and at pairing. Pairing waits up to 5 s for it, then sends whatever token it has (spec 009).
- `WakeService.onNewToken` stores the token and, if the socket is open, sends a `health` with `fcm_token` at once.

### Process lifecycle report
- `Telemetry` adds `lifecycle` (spec 002) to every `hello`, in both flavors:
  - `start`: the first entry of `ActivityManager.getHistoricalProcessStartReasons(1)`, if it belongs to the current process (its `pid` matches); otherwise `start` is omitted. Uses `getReason()`, `getStartupTimestamps()[START_TIMESTAMP_LAUNCH]` and `wasForceStopped()`.
  - `exit`: the first entry of `ActivityManager.getHistoricalProcessExitReasons(packageName, 0, 1)`, or `null`. Uses `getReason()`, `getTimestamp()` and `getDescription()`.
  - Both are read once per process, when `GatewayService` is created, and reused for every reconnect in that process.
- The reason ints are mapped to the strings of spec 002 in a pure function, so the mapping is unit-tested on the JVM.

### Manifest
- **Permissions:** `INTERNET`, `ACCESS_NETWORK_STATE`, `SEND_SMS`, `RECEIVE_SMS`, `READ_PHONE_STATE` (SIM list), `POST_NOTIFICATIONS`, `FOREGROUND_SERVICE`, `FOREGROUND_SERVICE_REMOTE_MESSAGING`, `RECEIVE_BOOT_COMPLETED`, `REQUEST_IGNORE_BATTERY_OPTIMIZATIONS`, `WAKE_LOCK`.
- `uses-feature android.hardware.telephony required=true`.
- **Network security:** one `network_security_config.xml` with a domain-agnostic `cleartextTrafficPermitted="true"` base config that is gated at runtime: release builds require https/wss unless the user ticked "Allow insecure http (LAN)" while pairing; debug builds accept http/ws without it. A `debug-overrides` block (debug builds only) also trusts user-installed CAs for local development servers.

## Acceptance criteria
1. AC-011-01 `assembleFossRelease` succeeds with R8. The APK is under 3 MB and contains no Firebase, AppCompat or Compose classes.
2. AC-011-02 Every fixture in `protocol/fixtures` parses into the Kotlin types and re-serialises to equivalent JSON.
3. AC-011-03 Outbox: frames inserted while offline are sent in order after reconnect, and an ack removes exactly that row.
4. AC-011-04 The backoff sequence is 1, 2, 4 … up to 120 s within ±20 %, and resets after a connection has been up for 60 s.
5. AC-011-05 After a reboot with the gateway enabled, the FGS runs and connects. Manual check on the emulator.
6. AC-011-06 While connected and idle, `dumpsys power` shows no wake lock held by the app.
7. AC-011-07 Log redaction keeps only the last 4 digits of numbers written with spaces, dashes, dots or parentheses, and leaves short numbers and ids alone.
8. AC-011-08 Service start deletes `sent` rows in a final state older than 7 days and keeps `accepted` rows.
9. AC-011-09 Every `ApplicationStartInfo` and `ApplicationExitInfo` reason constant maps to its spec 002 string, and an unknown int maps to `unknown_<int>`.
10. AC-011-10 Killing the app process (`adb shell am kill`) with the gateway enabled and default battery settings: on gms the gateway reconnects with `hello.lifecycle.exit.reason` set and `start.reason="push"`; on foss the watchdog posts "Gateway stopped — tap to restart", and tapping it reconnects with `start.reason="start_activity"`. Manual check on an API 35 and an API 36+ emulator.

## Test plan
| AC | Test |
|---|---|
| 01 | Gradle build plus `apkanalyzer` |
| 02 | `ProtocolFixturesTest` (JVM; reads `../protocol/fixtures` via a test resource srcDir) |
| 03 | `OutboxTest` (Robolectric-free: the SQL lives in a small interface faked in JVM) |
| 04 | `BackoffTest` |
| 05, 06 | Manual check in M7 |
| 07 | `RedactTest` |
| 08 | `RedactTest` (final-state list); the `Db.pruneSent` SQL is checked by review, since Robolectric is not used |
| 09 | `LifecycleReasonTest` |
| 10 | Manual check; the gms half also covers AC-009-05 |

## Assumptions
- Robolectric is not used, to keep the test toolchain light. Code that needs the Android framework is kept thin.
- The token is encrypted with the Keystore. Other settings are stored in plain SharedPreferences.
- The socket counts as "open" for `Uplink` only after `welcome`, so the outbox is resent once per connection (after `welcome`, as above, rather than right after `hello`).
- If `welcome.config.ping_s` differs from the interval the socket was opened with, the app reconnects once with the new interval.
- Pure logic (`Backoff`, `Uplink`, `Sender`, `PartAggregator`, `InboundFilter`, `InboundUid`, `FrameHandler`) lives in `gateway` behind small store/radio interfaces; `Graph` holds the process-wide `Prefs`, `Db`, `Uplink` and `Sender` shared by the service and receivers.
- The `sends today` / `receives today` counters are kept in prefs and reset at local midnight.

## Open questions
