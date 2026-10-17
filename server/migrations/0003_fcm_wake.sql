ALTER TABLE devices ADD COLUMN wake_attempts INTEGER NOT NULL DEFAULT 0;
ALTER TABLE devices ADD COLUMN unreachable_at INTEGER;
ALTER TABLE devices ADD COLUMN gateway_off_at INTEGER;
ALTER TABLE devices ADD COLUMN lifecycle_json TEXT;
