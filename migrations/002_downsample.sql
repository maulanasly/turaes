-- Downsample metrics to 1-minute buckets and enforce uniqueness so the monitor
-- can upsert a running aggregate per minute (one row per app/region per minute).

CREATE TABLE app_metrics_new (
    id              TEXT PRIMARY KEY,
    application_id  TEXT NOT NULL REFERENCES applications(id) ON DELETE CASCADE,
    cpu_pct         REAL NOT NULL DEFAULT 0,
    mem_bytes       INTEGER NOT NULL DEFAULT 0,
    recorded_at     TEXT NOT NULL DEFAULT (datetime('now'))
);
INSERT INTO app_metrics_new (id, application_id, cpu_pct, mem_bytes, recorded_at)
SELECT lower(hex(randomblob(16))), application_id, avg(cpu_pct), max(mem_bytes),
       strftime('%Y-%m-%d %H:%M:00', recorded_at)
FROM app_metrics
GROUP BY application_id, strftime('%Y-%m-%d %H:%M:00', recorded_at);
DROP TABLE app_metrics;
ALTER TABLE app_metrics_new RENAME TO app_metrics;
CREATE INDEX IF NOT EXISTS idx_app_metrics_app ON app_metrics(application_id, recorded_at DESC);
CREATE UNIQUE INDEX IF NOT EXISTS ux_app_metrics_minute ON app_metrics(application_id, recorded_at);

CREATE TABLE visit_metrics_new (
    id              TEXT PRIMARY KEY,
    application_id  TEXT NOT NULL REFERENCES applications(id) ON DELETE CASCADE,
    region          TEXT NOT NULL DEFAULT 'unknown',
    visits          INTEGER NOT NULL DEFAULT 0,
    uniques         INTEGER NOT NULL DEFAULT 0,
    recorded_at     TEXT NOT NULL DEFAULT (datetime('now'))
);
INSERT INTO visit_metrics_new (id, application_id, region, visits, uniques, recorded_at)
SELECT lower(hex(randomblob(16))), application_id, region, sum(visits), max(uniques),
       strftime('%Y-%m-%d %H:%M:00', recorded_at)
FROM visit_metrics
GROUP BY application_id, region, strftime('%Y-%m-%d %H:%M:00', recorded_at);
DROP TABLE visit_metrics;
ALTER TABLE visit_metrics_new RENAME TO visit_metrics;
CREATE INDEX IF NOT EXISTS idx_visit_metrics_app ON visit_metrics(application_id, recorded_at DESC);
CREATE UNIQUE INDEX IF NOT EXISTS ux_visit_metrics_minute ON visit_metrics(application_id, region, recorded_at);
