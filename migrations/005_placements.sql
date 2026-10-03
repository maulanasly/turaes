-- N4: app placements. One row per (app, server) so an app can run on multiple
-- nodes (replicas) and the proxy can load-balance across them. Existing apps
-- are seeded with their current server as the sole placement.

CREATE TABLE IF NOT EXISTS app_servers (
    id              TEXT PRIMARY KEY,
    application_id  TEXT NOT NULL REFERENCES applications(id) ON DELETE CASCADE,
    server_id       TEXT NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
    port            INTEGER NOT NULL,
    created_at      TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(application_id, server_id)
);

INSERT OR IGNORE INTO app_servers (id, application_id, server_id, port)
SELECT lower(hex(randomblob(16))), id, server_id, port FROM applications;

CREATE INDEX IF NOT EXISTS idx_app_servers_app ON app_servers(application_id);
