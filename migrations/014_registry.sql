-- Registry: versioned binary distribution for CI pushes.
--
-- A push carries bytes + provenance only. The app record stays the sole
-- authority for runtime config (port, health, domain, env): push payloads
-- can never change how an app runs. Deploys resolve `version`/`channel` to
-- an immutable content hash and join the existing deploy path there.
--
-- Blobs live in the content-addressed `ArtifactStore` on disk; these tables
-- hold identity + provenance. GC treats `artifacts.hash` as referenced.

-- Which CI repos may push into which app (`owner/name`).
CREATE TABLE IF NOT EXISTS registry_links (
    id             TEXT PRIMARY KEY,
    org_id         TEXT NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    application_id TEXT NOT NULL REFERENCES applications(id) ON DELETE CASCADE,
    repo           TEXT NOT NULL,
    created_by     TEXT,
    created_at     TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(org_id, application_id, repo)
);
CREATE INDEX IF NOT EXISTS idx_registry_links_app
    ON registry_links(org_id, application_id);

-- One row per stored blob (single binary or one file of a bundle).
CREATE TABLE IF NOT EXISTS artifacts (
    id          TEXT PRIMARY KEY,
    org_id      TEXT NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    hash        TEXT NOT NULL UNIQUE,
    size_bytes  INTEGER NOT NULL DEFAULT 0,
    media_type  TEXT NOT NULL DEFAULT 'application/octet-stream',
    arch        TEXT,
    created_by  TEXT,
    created_at  TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_artifacts_org_created
    ON artifacts(org_id, created_at);

-- An immutable named pointer to one artifact. `files_json` records bundle
-- members (`[{path, hash, mode}]`); the first entry is the deployable file.
CREATE TABLE IF NOT EXISTS releases (
    id             TEXT PRIMARY KEY,
    org_id         TEXT NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    application_id TEXT NOT NULL REFERENCES applications(id) ON DELETE CASCADE,
    version        TEXT NOT NULL,
    artifact_id    TEXT NOT NULL REFERENCES artifacts(id) ON DELETE RESTRICT,
    commit_sha     TEXT,
    build_url      TEXT,
    notes          TEXT,
    files_json     TEXT NOT NULL DEFAULT '[]',
    is_yanked      INTEGER NOT NULL DEFAULT 0,
    created_by     TEXT,
    created_at     TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(org_id, application_id, version)
);
CREATE INDEX IF NOT EXISTS idx_releases_app
    ON releases(org_id, application_id, created_at);

-- Mutable channel pointers (`stable`, `latest`, …) into immutable releases.
CREATE TABLE IF NOT EXISTS release_channels (
    org_id         TEXT NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    application_id TEXT NOT NULL REFERENCES applications(id) ON DELETE CASCADE,
    channel        TEXT NOT NULL,
    release_id     TEXT NOT NULL REFERENCES releases(id) ON DELETE CASCADE,
    updated_at     TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (org_id, application_id, channel)
);
