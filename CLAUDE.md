# rustybee

A personal, single-user SMS gateway made of three parts:
- `server/`: one Rust binary with axum, SQLite (rusqlite), and askama + htmx for the web UI.
- `android/`: a native Kotlin app on minSdk 30 and targetSdk 36, with flavors `foss` and `gms`.
- `protocol/`: the contract between the two, with golden fixtures that both test suites read.

## Spec-driven development (soft gate)

- Specs live in `docs/specs/NNN-<slug>.md`, and `docs/specs/README.md` is the index. Each spec has a `Status:` of draft, approved, implemented or superseded.
- Before writing code against a `draft` spec, or behaviour no spec covers, say so in one line and ask: approve the spec, discuss it, or go ahead anyway. If the answer is "go ahead", add a note under the spec's Open questions.
- A change in behaviour updates the spec in the same commit as the code.
- Name each test after the acceptance criterion it covers, e.g. `ac_002_04_replay_after_reconnect`.

## Build and run

- Server:
  - `cd server && cargo test`
  - `cargo run -- serve`; set the admin password with `cargo run -- set-password`.
- Android:
  - Set `JAVA_HOME="/Applications/Android Studio.app/Contents/jbr/Contents/Home"`. The system JDK 27 is too new for Gradle.
  - `cd android && ./gradlew testFossDebugUnitTest assembleFossDebug`
- Container: `docker build -f deploy/Dockerfile -t rustybee .`, run from the repo root.

## Conventions

- Keep the footprint small. Every new dependency needs a reason.
- Server: domain logic in `domain/` stays pure, with no IO. All SQLite writes go through the store's writer.
- Android: no AppCompat, Material, Compose, Room, WorkManager or DI framework. Send and receive work runs inside `GatewayService`.
- No upstream textbee/vernu names, links or package ids, except the credit note in the root README.
