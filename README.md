# rustybee

A personal SMS gateway: your Android phone sends and receives SMS on behalf of a small self-hosted server.

- `server/`: one Rust binary with SQLite. It serves the device WebSocket, the REST API (`/api/v1`) and the web UI.
- `android/`: a native Kotlin app (Android 16+, package `dev.joowdx.rustybee`) that keeps one WebSocket open from a foreground service.
- `protocol/`: the device protocol and its golden fixtures.
- `deploy/`: Docker image, compose file, systemd unit, reverse proxy notes.
- `docs/specs/`: the specs this code is built from.

## Quick start

```sh
docker build -f deploy/Dockerfile -t rustybee .
docker run -d --name rustybee -p 8080:8080 -v rustybee-data:/data \
  -e RUSTYBEE_PUBLIC_URL=https://sms.example.org -e RUSTYBEE_ADMIN_PASSWORD='change-me-now' rustybee
```

Open the web UI, go to Devices → Pair a device, and scan the QR code with the phone. Then send:

```sh
curl -H "Authorization: Bearer rbk_…" -H 'Content-Type: application/json' \
  -d '{"to":["+15557654321"],"body":"hello"}' https://sms.example.org/api/v1/messages
```

## Licence

MIT
