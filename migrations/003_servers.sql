-- N0: node registry. The local host is a well-known row (id = 'local'); every
-- application is placed on a server (defaults to local).

CREATE TABLE IF NOT EXISTS servers (
    id            TEXT PRIMARY KEY,
    name          TEXT NOT NULL UNIQUE,
    address       TEXT NOT NULL,
    ssh_host      TEXT,
    ssh_port      INTEGER,
    ssh_user      TEXT,
    ssh_key_enc   TEXT,
    is_local      INTEGER NOT NULL DEFAULT 0,
    status        TEXT NOT NULL DEFAULT 'unknown',
    last_seen_at  TEXT,
    agent_version TEXT,
    created_at    TEXT NOT NULL DEFAULT (datetime('now'))
);

INSERT OR IGNORE INTO servers (id, name, address, is_local, status)
VALUES ('local', 'localhost', '127.0.0.1', 1, 'online');

ALTER TABLE applications ADD COLUMN server_id TEXT NOT NULL DEFAULT 'local';
CREATE INDEX IF NOT EXISTS idx_applications_server ON applications(server_id);
