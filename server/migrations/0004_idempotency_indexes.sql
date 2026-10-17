-- Idempotency-Key on POST /api/v1/messages and /api/v1/batches (spec 007).
CREATE TABLE idempotency_keys (id INTEGER PRIMARY KEY, key TEXT NOT NULL UNIQUE,
  request_sha256 BLOB NOT NULL, response TEXT NOT NULL, created_at INTEGER NOT NULL);
CREATE INDEX idempotency_created ON idempotency_keys(created_at);
-- Device acks by frame id, the dashboard day counts and the sweeper (spec 003);
-- messages_state also covers the sweeper's queued/dispatched scans.
CREATE INDEX messages_frame ON messages(device_id, frame_id) WHERE frame_id IS NOT NULL;
CREATE INDEX messages_state ON messages(state, id);
CREATE INDEX messages_sent_at ON messages(sent_at) WHERE sent_at IS NOT NULL;
CREATE INDEX messages_delivered_at ON messages(delivered_at) WHERE delivered_at IS NOT NULL;
CREATE INDEX messages_failed_at ON messages(failed_at) WHERE failed_at IS NOT NULL;
CREATE INDEX messages_expired_at ON messages(expired_at) WHERE expired_at IS NOT NULL;
