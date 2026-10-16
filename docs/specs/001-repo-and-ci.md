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
  - secrets: `android/keystore.properties`, `*.jks`, `android/app/src/gms/google-services.json`, `*.db*`, `.env`, `/data/` (anchored, so source packages named `data` are not ignored).
- CI:
  - `master` takes changes through pull requests only, squash-merged, so each PR title becomes one commit subject.
  - `pr.yml` runs on every PR and fails unless the title is a Conventional Commit (`feat(android): ...`, `fix: ...`, `feat!: ...`). It is the check to require in branch protection; the path-filtered workflows below skip when their files are untouched.
  - `server.yml`, triggered by `server/**`, `protocol/**`, `deploy/**`:
    - on a PR: runs `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test`, then builds the image natively on amd64 and arm64 runners without pushing;
    - on a push to `master` (the package): also publishes the multi-arch image `ghcr.io/joowdx/rustybee` tagged `latest`, `sha-<short>` and the major version of `server/Cargo.toml`.
  - `android.yml`, triggered by `android/**`, `protocol/**`, `VERSION`, on JDK 21:
    - on a PR: runs `./gradlew testFossDebugUnitTest assembleFossRelease`, signed when the `ANDROID_KEYSTORE_*` secrets are present, and uploads the APK as a run artifact;
    - on a push to `master` (the release): computes the next version, builds and signs the foss APK (the `ANDROID_KEYSTORE_*` secrets are required) and the gms APK when the `RUSTYBEE_GOOGLE_SERVICES_JSON` secret exists, and creates the GitHub release and tag `v<version>` with the APKs and `SHA256SUMS`.
- Versioning:
  - The project starts at **3.0.0**, after the 2.x releases of the old fork, and follows semver from here with no upstream suffix.
  - Releases are the Android app only. `.github/scripts/next-version.sh` bumps the last `vX.Y.Z` tag by the commits since it that touch `android/` or `protocol/`: a breaking change (`type!:` or a `BREAKING CHANGE:` footer) bumps major, a `feat` bumps minor, anything else bumps patch. Nothing is committed back; the tag is the record.
  - The repository-root `VERSION` file is a floor: the release is the higher of it and the bumped tag, so raising it in a PR forces a jump, and with no tag yet `VERSION` itself is released. Local builds read it; CI passes the release version in `RUSTYBEE_VERSION`. The app sets `versionName` to the version and `versionCode = major×10000 + minor×100 + patch`.
  - `server/Cargo.toml` versions the server on its own; only its major goes into the image tag.
- Branding: the legacy bee icons are not carried over. The rusty bee in `branding/` is the source for every icon: the Android adaptive and notification icons, the web UI favicon and the header and login logo. `branding/generate-icons.sh` regenerates them.

## Acceptance criteria
1. AC-001-01 `git log master` has no commit shared with `main`.
2. AC-001-02 Secrets matching the `.gitignore` patterns can never be committed.
3. AC-001-03 Both CI workflows pass on a clean checkout.
4. AC-001-04 Merging a PR that touches `android/` publishes the signed APK in a GitHub release `v<version>`, versioned by its Conventional Commit title.
5. AC-001-05 Merging a PR that touches `server/` publishes the multi-arch image to ghcr.
6. AC-001-06 A PR whose title is not a Conventional Commit fails `pr.yml`.

## Test plan
| AC | Test |
|---|---|
| 01 | `git merge-base main master` exits non-zero |
| 02 | `git check-ignore` on the sample paths |
| 03 | CI run |
| 04 | First release run; dry-run of `next-version.sh` on fake history |
| 05 | First package run |
| 06 | PR run |

## Assumptions
- The worktree lives at `../rustybee`, a sibling of the legacy checkout.
- The `gms` flavor is built in CI only when a `RUSTYBEE_GOOGLE_SERVICES_JSON` secret exists.

## Open questions
- Resolved: the `upstream` remote was removed at cutover.
