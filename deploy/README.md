# Deploying rustybee

rustybee is a single static binary with an SQLite database in its data
directory. It speaks plain HTTP; put a reverse proxy or tunnel in front of it
for anything outside your LAN.

## Docker

```sh
# from the repository root
docker build -f deploy/Dockerfile -t rustybee .

cd deploy
cp .env.example .env        # set RUSTYBEE_PUBLIC_URL and RUSTYBEE_ADMIN_PASSWORD
docker compose up -d
docker compose ps           # should show "healthy"
curl -fsS http://127.0.0.1:8080/api/v1/health
```

The image is `FROM scratch`, runs as uid 10001 and keeps everything in the
`/data` volume. Compose publishes port 8080 on `127.0.0.1` only, for a proxy or
tunnel on the same host; change it to `"8080:8080"` only when there is no proxy.
`RUSTYBEE_ADMIN_PASSWORD` is only used when no password is set
yet; remove it from `.env` after the first login. To change the password later:

```sh
docker exec -it rustybee /rustybee set-password
```

Multi-arch images build natively for each platform:

```sh
docker buildx build -f deploy/Dockerfile --platform linux/amd64,linux/arm64 -t you/rustybee --push .
```

## systemd (no Docker)

```sh
cd server && cargo build --release
sudo install -m 755 target/release/rustybee /usr/local/bin/rustybee
sudo install -m 644 ../deploy/rustybee.service /etc/systemd/system/
sudoedit /etc/rustybee.env   # RUSTYBEE_PUBLIC_URL=..., RUSTYBEE_ADMIN_PASSWORD=...
sudo systemctl daemon-reload && sudo systemctl enable --now rustybee
```

Data lives in `/var/lib/rustybee` (a systemd `StateDirectory`).

## Reverse proxy

Set `RUSTYBEE_PUBLIC_URL` to the public HTTPS URL and `RUSTYBEE_TRUST_PROXY=1`
so client IPs and the scheme come from `X-Forwarded-*` headers. Phones keep a
long-lived WebSocket on `/api/device/ws`, and the dashboard uses
server-sent events on `/events`; the proxy must not buffer or time them out.

### Caddy

```caddy
sms.example.com {
    reverse_proxy 127.0.0.1:8080
}
```

Caddy handles WebSockets and streaming responses out of the box.

### nginx

```nginx
server {
    listen 443 ssl http2;
    server_name sms.example.com;
    # ssl_certificate ...; ssl_certificate_key ...;

    client_max_body_size 2m;

    location / {
        proxy_pass http://127.0.0.1:8080;
        proxy_http_version 1.1;
        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection $connection_upgrade;
        proxy_read_timeout 600s;
    }

    location /events {
        proxy_pass http://127.0.0.1:8080;
        proxy_http_version 1.1;
        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
        proxy_set_header Connection "";
        proxy_buffering off;
        proxy_cache off;
        proxy_read_timeout 600s;
    }
}

# in the http {} block
map $http_upgrade $connection_upgrade {
    default upgrade;
    ''      close;
}
```

`proxy_read_timeout` must exceed the device ping interval
(`RUSTYBEE_PING_SECONDS`, 270 s by default).

### Cloudflare Tunnel

```yaml
# ~/.cloudflared/config.yml
tunnel: <tunnel-id>
credentials-file: /etc/cloudflared/<tunnel-id>.json
ingress:
  - hostname: sms.example.com
    service: http://127.0.0.1:8080
  - service: http_status:404
```

Cloudflare closes idle WebSockets after about 100 seconds, so set
`RUSTYBEE_PING_SECONDS=60` along with `RUSTYBEE_TRUST_PROXY=1`.

## Backups

`rustybee backup <path>` writes a consistent copy of the live database using
SQLite's online backup, so the server can keep running.

```sh
# Docker
docker exec rustybee /rustybee backup /data/backup-$(date +%F).db
docker cp rustybee:/data/backup-$(date +%F).db .

# systemd
sudo systemd-run --pty -p DynamicUser=yes -p StateDirectory=rustybee \
  -E RUSTYBEE_DATA_DIR=/var/lib/rustybee \
  /usr/local/bin/rustybee backup /var/lib/rustybee/backup-$(date +%F).db
```

To restore, stop the server, replace `rustybee.db` in the data directory with
the backup (and delete any `rustybee.db-wal` / `rustybee.db-shm` next to it),
then start it again.

## Configuration

| Variable | Default | Meaning |
|---|---|---|
| `RUSTYBEE_PUBLIC_URL` | `http://<bind>` | base URL in pairing links and `ws_url` |
| `RUSTYBEE_BIND` | `127.0.0.1:8080` (`0.0.0.0:8080` in the image) | listen address |
| `RUSTYBEE_DATA_DIR` | `./data` (`/data` in the image) | SQLite location |
| `RUSTYBEE_ADMIN_PASSWORD` | — | password set at first start if none exists |
| `RUSTYBEE_PING_SECONDS` | `270` | device ping interval |
| `RUSTYBEE_MESSAGE_TTL_HOURS` | `24` | queued-message lifetime |
| `RUSTYBEE_TRUST_PROXY` | `0` | trust `X-Forwarded-For` / `X-Forwarded-Proto` |
| `RUSTYBEE_OTP_MAX_FAILURES` | `3` | failed login-code sends before password login unlocks |
| `RUSTYBEE_MAX_QUEUED` | `1000` | queued messages per device before sends get 429 `queue_full` |
| `RUSTYBEE_ALLOW_SHORT_CODES` | `0` | `1` to accept 3–6 digit recipients (short codes) |
| `RUSTYBEE_RETENTION_DAYS` | — | prune finished messages and webhook deliveries older than this |
| `RUSTYBEE_TURNSTILE_SITE_KEY` | — | Turnstile on Cloudflare-proxied logins; needs the secret too |
| `RUSTYBEE_TURNSTILE_SECRET` | — | Turnstile Siteverify secret |
| `RUSTYBEE_FCM_CREDENTIALS` | — | Firebase service-account JSON for wake-ups |
| `RUST_LOG` | `info` | tracing filter |
