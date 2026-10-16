# 014 — Deploy

Status: implemented
Updated: 2026-10-16

## Design

### `deploy/Dockerfile` (built from the repo root)
- **Build stage:** `rust:1-alpine` with `apk add musl-dev`.
  - `cargo build --release --locked` in `server/`.
  - BuildKit cache mounts cover the cargo registry and `target`.
  - It builds natively for the buildx target platform, so `docker buildx build --platform linux/amd64,linux/arm64` works without cross-compilers.
- **Final stage:** `FROM scratch`. Copy in the binary as `/rustybee`, create `/data` owned by `10001:10001` (prepared in the build stage), and set `USER 10001:10001`.
  - `ENV RUSTYBEE_DATA_DIR=/data RUSTYBEE_BIND=0.0.0.0:8080`
  - `EXPOSE 8080`, `VOLUME /data`
  - `HEALTHCHECK CMD ["/rustybee","healthcheck"]`. The `healthcheck` subcommand does a GET on `http://127.0.0.1:<port>/api/v1/health` with a plain TCP client and exits 0 or 1.
  - `ENTRYPOINT ["/rustybee"]`, `CMD ["serve"]`
- **TLS roots:** reqwest uses `rustls` with bundled webpki roots, so no CA files are needed in the image.

### `deploy/compose.yaml`
- One service `rustybee`, using the published image `ghcr.io/joowdx/rustybee:3` (or a local build from the Dockerfile with `docker compose build`), a named volume `rustybee-data:/data`, `restart: unless-stopped`, and port `127.0.0.1:8080:8080`: loopback only, for a reverse proxy or tunnel on the same host. Widen it to `8080:8080` only when there is no proxy.
- Environment from `.env`: `RUSTYBEE_PUBLIC_URL`, `RUSTYBEE_ADMIN_PASSWORD` (first run only), and the optional `RUSTYBEE_PING_SECONDS`, `RUSTYBEE_TRUST_PROXY`, `RUSTYBEE_TURNSTILE_SITE_KEY`, `RUSTYBEE_TURNSTILE_SECRET` and `RUSTYBEE_FCM_CREDENTIALS` (the file mounted read-only).
- `deploy/.env.example` documents every variable.

### `deploy/rustybee.service`
A systemd unit for running without Docker:
- `DynamicUser=yes`, `StateDirectory=rustybee`, `Environment=RUSTYBEE_DATA_DIR=/var/lib/rustybee`;
- hardening: `ProtectSystem=strict`, `NoNewPrivileges`, `PrivateTmp`, `MemoryMax=64M`.

### Reverse proxy snippets (`deploy/README.md`)
- **Caddy:** `reverse_proxy 127.0.0.1:8080`. WebSockets work by default; set `RUSTYBEE_TRUST_PROXY=1`.
- **nginx:** `proxy_http_version 1.1`, the `Upgrade`/`Connection` headers, `proxy_read_timeout 600s`, and `proxy_buffering off` for `/events`.
- **Cloudflare Tunnel:** set `RUSTYBEE_PING_SECONDS=60`, because idle WebSockets are cut at about 100 s. Set `RUSTYBEE_TRUST_PROXY=1`, and optionally the Turnstile keys (widget hostname = the public host).
- **Backups:** `docker exec rustybee /rustybee backup /data/backup-$(date +%F).db`.

### Configuration (all optional except the public URL)
| Var | Default | Meaning |
|---|---|---|
| `RUSTYBEE_BIND` | `127.0.0.1:8080` | listen address |
| `RUSTYBEE_DATA_DIR` | `./data` | SQLite location |
| `RUSTYBEE_PUBLIC_URL` | `http://<bind>` | base URL used in pairing links and `ws_url` |
| `RUSTYBEE_ADMIN_PASSWORD` | — | sets the password at first start if none is set |
| `RUSTYBEE_PING_SECONDS` | `270` | device ping interval (spec 002) |
| `RUSTYBEE_MESSAGE_TTL_HOURS` | `24` | spec 004 |
| `RUSTYBEE_TRUST_PROXY` | `0` | trust `CF-Connecting-IP` / `X-Forwarded-For` / `X-Forwarded-Proto` |
| `RUSTYBEE_TURNSTILE_SITE_KEY` | — | Turnstile on Cloudflare-proxied logins (spec 005); needs the secret too |
| `RUSTYBEE_TURNSTILE_SECRET` | — | Turnstile Siteverify secret |
| `RUSTYBEE_FCM_CREDENTIALS` | — | path to the service-account JSON (spec 009) |
| `RUST_LOG` | `info` | tracing filter |

## Acceptance criteria
1. AC-014-01 The image builds, is under 15 MB, and runs as a non-root user.
2. AC-014-02 `docker compose up` gives a healthy container, and `/api/v1/health` returns 200.
3. AC-014-03 Data survives a restart of the container.
4. AC-014-04 Idle RSS stays under 20 MB after 5 minutes with one connected device.

## Test plan
Manual checks in M7, with the figures recorded in `docs/specs/README.md`.

## Assumptions
- The server terminates no TLS itself. A reverse proxy or tunnel is expected for anything outside the LAN.

## Open questions
