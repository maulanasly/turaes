-- Server capacity: host-level CPU/memory samples, one row per server per
-- minute (same rollup contract as app_metrics). The local control plane
-- samples itself every monitor tick; remote nodes report no host stats yet
-- (their capacity reads as unknown until agent host-stats land).
CREATE TABLE IF NOT EXISTS server_metrics (
    id              TEXT PRIMARY KEY,
    server_id       TEXT NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
    cpu_pct         REAL NOT NULL DEFAULT 0,
    mem_bytes       INTEGER NOT NULL DEFAULT 0,
    mem_total_bytes INTEGER NOT NULL DEFAULT 0,
    recorded_at     TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_server_metrics_server ON server_metrics(server_id, recorded_at DESC);
CREATE UNIQUE INDEX IF NOT EXISTS ux_server_metrics_minute ON server_metrics(server_id, recorded_at);
