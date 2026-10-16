# 001 — Repository and CI

Status: implemented
Updated: 2026-10-16

## Context
The rewrite lives on the orphan branch `master`, which shares no history with the legacy code. The legacy code stays on `main`, unchanged, as a read-only reference.

## Design
- Layout:
  - `server/`
  - `android/`
  - `protocol/`
  - `deploy/`
  - `docs/rewrite/` (audit, plan)
  - `docs/specs/`
  - `.github/workflows/`
  - `CLAUDE.md`, `README.md`, `LICENSE` (MIT, © joowdx)
- `.gitignore` covers:
  - Rust `target/`;
  - Gradle `.gradle/`, `build/`, `local.properties`, `.idea/`;
  - secrets: `android/keystore.properties`, `*.jks`, `android/app/src/gms/google-services.json`, `*.db*`, `.env`, `data/`.
- CI:
  - `server.yml`, triggered by `server/**`, `protocol/**`: runs `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test`, then `docker build -f deploy/Dockerfile .`.
  - `android.yml`, triggered by `android/**`, `protocol/**`, on JDK 21: runs `./gradlew testFossDebugUnitTest assembleFossRelease`. Release signing comes from the secrets `ANDROID_KEYSTORE_BASE64`, `ANDROID_KEYSTORE_PASSWORD`, `ANDROID_KEY_ALIAS` and `ANDROID_KEY_PASSWORD` when they are present; without them the release is unsigned.
  - `hygiene.yml`: the `textbee|vernu` grep from AC-000-02, and a check that `VERSION` matches `server/Cargo.toml`.
  - `release.yml`, triggered by a pushed tag `v<VERSION>`:
    - fails unless the tag, `VERSION` and `server/Cargo.toml` agree;
    - builds the image natively on amd64 and arm64 runners and publishes the multi-arch package `ghcr.io/joowdx/rustybee` tagged `<version>`, `<major>.<minor>`, `<major>` and `latest`;
    - exports the static server binary for both architectures from the same build;
    - builds and signs the foss APK (the `ANDROID_KEYSTORE_*` secrets are required), and the gms APK when the `RUSTYBEE_GOOGLE_SERVICES_JSON` secret exists;
    - creates the GitHub release `v<version>` with the APKs, the binaries and `SHA256SUMS`.
- Versioning:
  - The project starts at **3.0.0**, after the 2.x releases of the old fork, and follows semver from here with no upstream suffix.
  - The repository-root `VERSION` file is the single source. The app reads it at build time: `versionName = VERSION`, `versionCode = major×10000 + minor×100 + patch`. `server/Cargo.toml` repeats it because Cargo can't read files, and CI checks the two match.
- Branding: the legacy bee icons are not carried over. The app icon is a new vector adaptive icon.

## Acceptance criteria
1. AC-001-01 `git log master` has no commit shared with `main`.
2. AC-001-02 Secrets matching the `.gitignore` patterns can never be committed.
3. AC-001-03 Both CI workflows pass on a clean checkout.
4. AC-001-04 Pushing tag `v3.0.0` publishes the multi-arch image, the signed APK and the binaries in one GitHub release.

## Test plan
| AC | Test |
|---|---|
| 01 | `git merge-base main master` exits non-zero |
| 02 | `git check-ignore` on the sample paths |
| 03 | CI run |
| 04 | First release run |

## Assumptions
- The worktree lives at `../rustybee`, a sibling of the legacy checkout.
- The `gms` flavor is built in CI only when a `RUSTYBEE_GOOGLE_SERVICES_JSON` secret exists.

## Open questions
- Resolved: the `upstream` remote was removed at cutover.
