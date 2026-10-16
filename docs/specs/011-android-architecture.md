# 011 — Android architecture

Status: approved (self, one-shot)
Updated: 2026-10-16

## Design

### Build
- **Project:** a single `:app` module with `applicationId dev.joowdx.rustybee`, `namespace dev.joowdx.rustybee`, and `compileSdk`, `minSdk` and `targetSdk` all set to 36.
- **Toolchain:** Kotlin 2.x with JVM target 17, the current stable AGP, and a version catalog in `gradle/libs.versions.toml`.
- **Flavors:** dimension `dist` with `foss` (the default) and `gms`. Only `gms` applies the `com.google.gms.google-services` plugin, and only if `src/gms/google-services.json` exists; otherwise the gms build fails with a clear message.
- **Release build:**
  - `isMinifyEnabled = true`, `isShrinkResources = true`.
  - Signing comes from `keystore.properties` (`storeFile`, `storePassword`, `keyAlias`, `keyPassword`) or the matching environment variables. If neither exists, the build is unsigned.
- **Dependencies:**
  - `com.squareup.okhttp3:okhttp` (5.x);
  - `org.jetbrains.kotlinx:kotlinx-serialization-json`;
  - `org.jetbrains.kotlinx:kotlinx-coroutines-android`;
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
| `gms` (flavor source set) | `WakeService` |
| `foss` (flavor source set) | empty `Push` stub returning a null token |

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
- `log` is capped at 1000 rows. Phone numbers are redacted to their last 4 digits.

### GatewayService
- A foreground service with `foregroundServiceType="remoteMessaging"` and the `FOREGROUND_SERVICE_REMOTE_MESSAGING` permission.
- **Notification:** channel `gateway`, `IMPORTANCE_LOW`, ongoing. It shows "Connected" or "Reconnecting…" and the count of sends today. Tapping it opens `MainActivity`.
- **Lifecycle:**
  - Started by the UI toggle, `BootReceiver`, `Watchdog`, `WakeService` and the receivers when work arrives.
  - `onStartCommand` returns `START_STICKY`.
  - It stops itself (`stopForeground` + `stopSelf`) when the gateway is disabled or unpaired.
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
  - If the gateway is enabled and the service isn't running: when the app is battery-unrestricted, start the FGS; otherwise post a high-importance "Gateway stopped — tap to restart" notification.
  - The job finishes immediately.
- The UI asks for the battery-optimization exemption with `ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS`, and explains why.

### Manifest
- **Permissions:** `INTERNET`, `ACCESS_NETWORK_STATE`, `SEND_SMS`, `RECEIVE_SMS`, `READ_PHONE_STATE` (SIM list), `POST_NOTIFICATIONS`, `FOREGROUND_SERVICE`, `FOREGROUND_SERVICE_REMOTE_MESSAGING`, `RECEIVE_BOOT_COMPLETED`, `REQUEST_IGNORE_BATTERY_OPTIMIZATIONS`, `WAKE_LOCK`.
- `uses-feature android.hardware.telephony required=true`.
- **Network security:** cleartext is allowed only for local development, through a debug-only network security config. Release builds require https/wss, except for user-entered `http://` URLs when the user ticks "Allow insecure (LAN)". That is allowed with a domain-agnostic `cleartextTrafficPermitted` that is checked at runtime.

## Acceptance criteria
1. AC-011-01 `assembleFossRelease` succeeds with R8. The APK is under 3 MB and contains no Firebase, AppCompat or Compose classes.
2. AC-011-02 Every fixture in `protocol/fixtures` parses into the Kotlin types and re-serialises to equivalent JSON.
3. AC-011-03 Outbox: frames inserted while offline are sent in order after reconnect, and an ack removes exactly that row.
4. AC-011-04 The backoff sequence is 1, 2, 4 … up to 120 s within ±20 %, and resets after a connection has been up for 60 s.
5. AC-011-05 After a reboot with the gateway enabled, the FGS runs and connects. Manual check on the emulator.
6. AC-011-06 While connected and idle, `dumpsys power` shows no wake lock held by the app.

## Test plan
| AC | Test |
|---|---|
| 01 | Gradle build plus `apkanalyzer` |
| 02 | `ProtocolFixturesTest` (JVM; reads `../protocol/fixtures` via a test resource srcDir) |
| 03 | `OutboxTest` (Robolectric-free: the SQL lives in a small interface faked in JVM) |
| 04 | `BackoffTest` |
| 05, 06 | Manual check in M7 |

## Assumptions
- Robolectric is not used, to keep the test toolchain light. Code that needs the Android framework is kept thin.
- The token is encrypted with the Keystore. Other settings are stored in plain SharedPreferences.

## Open questions
