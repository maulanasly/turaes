-- Durable at-least-once webhook delivery, one row per alert event.
CREATE TABLE IF NOT EXISTS alert_deliveries (
    id              TEXT PRIMARY KEY,
    alert_id        TEXT NOT NULL REFERENCES alerts(id) ON DELETE CASCADE,
    event           TEXT NOT NULL CHECK (event IN ('firing', 'resolved')),
    payload         TEXT NOT NULL,
    attempts        INTEGER NOT NULL DEFAULT 0,
    next_attempt_at TEXT NOT NULL DEFAULT (datetime('now')),
    lease_until     TEXT,
    delivered_at    TEXT,
    last_error      TEXT,
    created_at      TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(alert_id, event)
);
CREATE INDEX IF NOT EXISTS idx_alert_deliveries_pending
    ON alert_deliveries(delivered_at, next_attempt_at, created_at);

-- Keep one active row per rule key despite concurrent monitor evaluations.
-- Collapse any historical duplicates before installing the constraint,
-- preserving the newest firing row by rowid.
DELETE FROM alerts
WHERE status = 'firing'
  AND rowid NOT IN (
      SELECT MAX(rowid) FROM alerts WHERE status = 'firing' GROUP BY key
  );
CREATE UNIQUE INDEX IF NOT EXISTS idx_alerts_one_firing_per_key
    ON alerts(key) WHERE status = 'firing';
