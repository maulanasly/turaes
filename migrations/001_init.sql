-- turaes initial schema (M0).
-- Ids are UUIDv4 strings; timestamps are RFC3339 text via datetime('now').

CREATE TABLE IF NOT EXISTS applications (
    id            TEXT PRIMARY KEY,
    name          TEXT NOT NULL UNIQUE,
    description   TEXT,
    binary_path   TEXT NOT NULL,
    args          TEXT,
    port          INTEGER NOT NULL,
    health_path   TEXT NOT NULL DEFAULT '/health',
    metrics_path  TEXT DEFAULT '/metrics',
    domain        TEXT,
    runtime       TEXT NOT NULL DEFAULT 'systemd',
    auto_restart  INTEGER NOT NULL DEFAULT 1,
    status        TEXT NOT NULL DEFAULT 'stopped',
    created_at    TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at    TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS deployments (
    id                 TEXT PRIMARY KEY,
    application_id     TEXT NOT NULL REFERENCES applications(id) ON DELETE CASCADE,
    status             TEXT NOT NULL DEFAULT 'queued',
    artifact_hash      TEXT,
    log                TEXT,
    previous_artifact  TEXT,
    started_at         TEXT NOT NULL DEFAULT (datetime('now')),
    finished_at        TEXT
);
CREATE INDEX IF NOT EXISTS idx_deployments_app ON deployments(application_id, started_at DESC);

CREATE TABLE IF NOT EXISTS env_vars (
    id              TEXT PRIMARY KEY,
    application_id  TEXT NOT NULL REFERENCES applications(id) ON DELETE CASCADE,
    key             TEXT NOT NULL,
    value_enc       TEXT NOT NULL,
    created_at      TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(application_id, key)
);

CREATE TABLE IF NOT EXISTS domains (
    id              TEXT PRIMARY KEY,
    application_id  TEXT NOT NULL REFERENCES applications(id) ON DELETE CASCADE,
    domain          TEXT NOT NULL UNIQUE,
    is_primary      INTEGER NOT NULL DEFAULT 0,
    ssl_active      INTEGER NOT NULL DEFAULT 0,
    created_at      TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS health_checks (
    id                   TEXT PRIMARY KEY,
    application_id       TEXT NOT NULL UNIQUE REFERENCES applications(id) ON DELETE CASCADE,
    path                 TEXT NOT NULL DEFAULT '/health',
    interval_seconds     INTEGER NOT NULL DEFAULT 15,
    timeout_seconds      INTEGER NOT NULL DEFAULT 5,
    healthy_threshold    INTEGER NOT NULL DEFAULT 2,
    unhealthy_threshold  INTEGER NOT NULL DEFAULT 3,
    created_at           TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS health_results (
    id                TEXT PRIMARY KEY,
    application_id    TEXT NOT NULL REFERENCES applications(id) ON DELETE CASCADE,
    status            TEXT NOT NULL,
    status_code       INTEGER,
    response_time_ms  INTEGER,
    error_message     TEXT,
    checked_at        TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_health_results_app ON health_results(application_id, checked_at DESC);

CREATE TABLE IF NOT EXISTS app_metrics (
    id              TEXT PRIMARY KEY,
    application_id  TEXT NOT NULL REFERENCES applications(id) ON DELETE CASCADE,
    cpu_pct         REAL NOT NULL DEFAULT 0,
    mem_bytes       INTEGER NOT NULL DEFAULT 0,
    recorded_at     TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_app_metrics_app ON app_metrics(application_id, recorded_at DESC);

CREATE TABLE IF NOT EXISTS visit_metrics (
    id              TEXT PRIMARY KEY,
    application_id  TEXT NOT NULL REFERENCES applications(id) ON DELETE CASCADE,
    region          TEXT NOT NULL DEFAULT 'unknown',
    visits          INTEGER NOT NULL DEFAULT 0,
    uniques         INTEGER NOT NULL DEFAULT 0,
    recorded_at     TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_visit_metrics_app ON visit_metrics(application_id, recorded_at DESC);

CREATE TABLE IF NOT EXISTS events (
    id              TEXT PRIMARY KEY,
    application_id  TEXT REFERENCES applications(id) ON DELETE SET NULL,
    kind            TEXT NOT NULL,
    message         TEXT NOT NULL,
    created_at      TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_events_app ON events(application_id, created_at DESC);
