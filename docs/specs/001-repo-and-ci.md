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
  - `hygiene.yml`: the `textbee|vernu` grep from AC-000-02.
- Branding: the legacy bee icons are not carried over. The app icon is a new vector adaptive icon.

## Acceptance criteria
1. AC-001-01 `git log rewrite` has no commit shared with `main`.
2. AC-001-02 Secrets matching the `.gitignore` patterns can never be committed.
3. AC-001-03 Both CI workflows pass on a clean checkout.

## Test plan
| AC | Test |
|---|---|
| 01 | `git merge-base main rewrite` exits non-zero |
| 02 | `git check-ignore` on the sample paths |
| 03 | CI run |

## Assumptions
- The worktree lives at `../rustybee`, a sibling of the legacy checkout.
- The `gms` flavor is built in CI only when a `GOOGLE_SERVICES_JSON_RUSTYBEE` secret exists.

## Open questions
- Remove the `upstream` remote: ask the user at cutover.
