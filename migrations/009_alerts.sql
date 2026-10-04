-- O1: alerting. Firing alerts are the monitor's to-do list (deduplicated by
-- `key`); resolved rows are history. App alerts are org-scoped; platform
-- alerts (backups, …) have NULL org_id and surface to operators.

CREATE TABLE IF NOT EXISTS alerts (
    id              TEXT PRIMARY KEY,
    org_id          TEXT REFERENCES organizations(id) ON DELETE CASCADE,
    severity        TEXT NOT NULL DEFAULT 'warning',
    kind            TEXT NOT NULL,
    key             TEXT NOT NULL,
    subject         TEXT NOT NULL,
    detail          TEXT,
    status          TEXT NOT NULL DEFAULT 'firing',
    application_id  TEXT REFERENCES applications(id) ON DELETE SET NULL,
    fired_at        TEXT NOT NULL DEFAULT (datetime('now')),
    resolved_at     TEXT,
    notified_at     TEXT,
    created_at      TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_alerts_key ON alerts(key, status);
CREATE INDEX IF NOT EXISTS idx_alerts_org ON alerts(org_id, status, fired_at DESC);
CREATE INDEX IF NOT EXISTS idx_alerts_app ON alerts(application_id, status);
