# 003 — Data model and store

Status: implemented
Updated: 2026-10-16

## Design

### Connection setup
- The database file is `$RUSTYBEE_DATA_DIR/rustybee.db`, with `./data` as the default.
- PRAGMAs set on every connection: `journal_mode=WAL`, `synchronous=NORMAL`, `foreign_keys=ON`, `busy_timeout=5000`, `temp_store=MEMORY`, `cache_size=-2000`.
- **Writer:** one `rusqlite::Connection` owned by a dedicated thread. Callers send closures through `store.write(|tx| …)`, which runs them in a transaction and returns the result through a oneshot channel.
- **Readers:** a small pool of 2–4 read-only connections, used through `spawn_blocking`.
- `PRAGMA wal_checkpoint(TRUNCATE)` runs hourly.
- **Migrations:** embedded `migrations/NNNN_*.sql`, tracked with `PRAGMA user_version` and applied at start in one transaction each.
- **Time:** stored as INTEGER Unix ms. Public ids are TEXT ULIDs; every table also has an INTEGER rowid primary key.

### Schema (migration 0001)
```sql
CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE sessions (id INTEGER PRIMARY KEY, token_sha256 BLOB NOT NULL UNIQUE,
  created_at INTEGER NOT NULL, last_seen_at INTEGER NOT NULL, expires_at INTEGER NOT NULL,
  user_agent TEXT);
CREATE TABLE api_keys (id INTEGER PRIMARY KEY, public_id TEXT NOT NULL UNIQUE, name TEXT NOT NULL,
  prefix TEXT NOT NULL, token_sha256 BLOB NOT NULL UNIQUE, created_at INTEGER NOT NULL,
  last_used_at INTEGER, revoked_at INTEGER);
CREATE TABLE devices (id INTEGER PRIMARY KEY, public_id TEXT NOT NULL UNIQUE, name TEXT NOT NULL,
  token_sha256 BLOB NOT NULL UNIQUE, enabled INTEGER NOT NULL DEFAULT 1,
  is_default INTEGER NOT NULL DEFAULT 0, send_delay_ms INTEGER NOT NULL DEFAULT 5000,
  receive_enabled INTEGER NOT NULL DEFAULT 1, sim INTEGER, filters_json TEXT NOT NULL DEFAULT '{"mode":"off","rules":[]}',
  fcm_token TEXT, manufacturer TEXT, model TEXT, os_version TEXT, app_version TEXT, app_build INTEGER,
  telemetry_json TEXT, sims_json TEXT, connected INTEGER NOT NULL DEFAULT 0,
  last_seen_at INTEGER, last_wake_at INTEGER, created_at INTEGER NOT NULL, removed_at INTEGER);
CREATE TABLE pairing_codes (id INTEGER PRIMARY KEY, code_sha256 BLOB NOT NULL UNIQUE,
  created_at INTEGER NOT NULL, expires_at INTEGER NOT NULL, used_at INTEGER);
CREATE TABLE batches (id INTEGER PRIMARY KEY, public_id TEXT NOT NULL UNIQUE,
  device_id INTEGER NOT NULL REFERENCES devices(id), total INTEGER NOT NULL,
  created_at INTEGER NOT NULL);
CREATE TABLE messages (id INTEGER PRIMARY KEY, public_id TEXT NOT NULL UNIQUE,
  batch_id INTEGER REFERENCES batches(id) ON DELETE CASCADE,
  device_id INTEGER NOT NULL REFERENCES devices(id),
  direction TEXT NOT NULL CHECK (direction IN ('out','in')), peer TEXT NOT NULL, body TEXT NOT NULL,
  sim INTEGER, state TEXT NOT NULL, parts INTEGER, error_code TEXT, error_message TEXT,
  frame_id TEXT, device_acked_at INTEGER, inbound_uid TEXT,
  created_at INTEGER NOT NULL, dispatched_at INTEGER, sent_at INTEGER, delivered_at INTEGER,
  failed_at INTEGER, expired_at INTEGER, received_at INTEGER,
  UNIQUE (device_id, inbound_uid));
CREATE INDEX messages_device_state ON messages(device_id, state, id);
CREATE INDEX messages_created ON messages(created_at, id);
CREATE INDEX messages_peer ON messages(peer, id);
CREATE INDEX messages_batch ON messages(batch_id);
CREATE TABLE webhooks (id INTEGER PRIMARY KEY, public_id TEXT NOT NULL UNIQUE, name TEXT NOT NULL,
  url TEXT NOT NULL, events_json TEXT NOT NULL, secret TEXT NOT NULL, active INTEGER NOT NULL DEFAULT 1,
  created_at INTEGER NOT NULL);
CREATE TABLE webhook_deliveries (id INTEGER PRIMARY KEY, public_id TEXT NOT NULL UNIQUE,
  webhook_id INTEGER NOT NULL REFERENCES webhooks(id) ON DELETE CASCADE, event TEXT NOT NULL,
  message_id INTEGER REFERENCES messages(id) ON DELETE SET NULL, payload TEXT NOT NULL,
  attempts INTEGER NOT NULL DEFAULT 0, next_attempt_at INTEGER, lease_until INTEGER,
  last_status INTEGER, last_error TEXT, created_at INTEGER NOT NULL,
  delivered_at INTEGER, aborted_at INTEGER);
CREATE INDEX deliveries_due ON webhook_deliveries(next_attempt_at) WHERE delivered_at IS NULL AND aborted_at IS NULL;
CREATE TABLE seen_frames (device_id INTEGER NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
  frame_id TEXT NOT NULL, seen_at INTEGER NOT NULL, PRIMARY KEY (device_id, frame_id)) WITHOUT ROWID;
```
- Migration `0002` adds:
  - `messages.internal INTEGER NOT NULL DEFAULT 0`: 1 for admin login codes (spec 005). Internal messages never reach webhooks or the public API; the web UI lists them, with the body replaced once the message finishes.
  - `devices.filters_rev INTEGER NOT NULL DEFAULT 0`: goes up by one on every change to `receive_enabled` or the filters (spec 002).
- Settings keys used by spec 005: `admin_phone` (E.164) and `otp_send_failures` (a count).
- Batch counters are not stored. They are computed with `GROUP BY state` over `messages_batch`.
- `seen_frames` makes inbound frames idempotent by envelope id. Rows older than 7 days are pruned daily.

### Backup
`rustybee backup <path>` runs `VACUUM INTO '<path>'`, which is safe while the server is running.

## Acceptance criteria
1. AC-003-01 Starting on an empty data directory creates the database at the latest `user_version`. Starting again does nothing.
2. AC-003-02 All PRAGMAs are in effect on every connection.
3. AC-003-03 Concurrent writes from many tasks are serialized without `SQLITE_BUSY` errors.
4. AC-003-04 `backup` produces a database that opens and holds the same rows.

## Test plan
| AC | Test |
|---|---|
| 01–04 | `server/src/store` unit tests, using a temp dir |

## Assumptions
- Removing a device is a soft delete: `removed_at` is set, `token_sha256` is replaced with random bytes, `fcm_token` and `is_default` are cleared, and the row is hidden from every device list, selection and dispatch. Its messages and batches stay in history. Every outbound message of the device still in `queued` or `dispatched` becomes `failed`/`cancelled`.
- Message bodies are stored in plain text. The database file is protected by filesystem permissions, with no at-rest encryption.
- No automatic retention. Messages are kept until the user deletes them; a prune setting may come later.

## Open questions
