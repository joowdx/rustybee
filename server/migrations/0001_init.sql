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
