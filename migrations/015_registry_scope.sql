-- Registry tenancy + deployment provenance hardening.
--
-- 1. Artifact dedup was global (UNIQUE(hash)): any org knowing a hash could
--    adopt foreign bytes into its own releases. Scope it per org.
-- 2. Deployments name only a content hash; record which release (and human
--    version) a versioned deploy resolved, so yanked builds stay answerable
--    and rollback can refuse them.

-- SQLite cannot drop a constraint: recreate the table preserving rows.
-- Pre-existing rows are unique by hash (the old constraint), so the copy is
-- 1:1; each blob keeps its first-writer org.
CREATE TABLE artifacts_new (
    id          TEXT PRIMARY KEY,
    org_id      TEXT NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    hash        TEXT NOT NULL,
    size_bytes  INTEGER NOT NULL DEFAULT 0,
    media_type  TEXT NOT NULL DEFAULT 'application/octet-stream',
    arch        TEXT,
    created_by  TEXT,
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(org_id, hash)
);
INSERT INTO artifacts_new SELECT * FROM artifacts;
DROP TABLE artifacts;
ALTER TABLE artifacts_new RENAME TO artifacts;
CREATE INDEX IF NOT EXISTS idx_artifacts_org_created
    ON artifacts(org_id, created_at);

ALTER TABLE deployments ADD COLUMN release_id TEXT REFERENCES releases(id);
ALTER TABLE deployments ADD COLUMN resolved_version TEXT;
