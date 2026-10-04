-- UX-P0: tenancy foundation. Introduces organizations, users and role-based
-- membership so turaes can host multiple tenants, plus scoped API tokens and an
-- audit log. Existing rows are assigned to a seeded `default` organization.
--
-- Note: `servers` stays platform-global infrastructure (owned by operators), so
-- it is intentionally NOT org-scoped here; only applications are tenant-scoped.

CREATE TABLE IF NOT EXISTS organizations (
    id          TEXT PRIMARY KEY,
    slug        TEXT NOT NULL UNIQUE,
    name        TEXT NOT NULL,
    created_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Backfill target: every existing application belongs to this organization.
INSERT OR IGNORE INTO organizations (id, slug, name)
VALUES ('default', 'default', 'Default');

CREATE TABLE IF NOT EXISTS users (
    id           TEXT PRIMARY KEY,
    github_id    INTEGER NOT NULL UNIQUE,
    login        TEXT NOT NULL,
    name         TEXT,
    created_at   TEXT NOT NULL DEFAULT (datetime('now')),
    last_seen_at TEXT
);

CREATE TABLE IF NOT EXISTS memberships (
    id          TEXT PRIMARY KEY,
    org_id      TEXT NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    user_id     TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    role        TEXT NOT NULL DEFAULT 'viewer',
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(org_id, user_id)
);
CREATE INDEX IF NOT EXISTS idx_memberships_user ON memberships(user_id);
CREATE INDEX IF NOT EXISTS idx_memberships_org ON memberships(org_id);

CREATE TABLE IF NOT EXISTS api_tokens (
    id           TEXT PRIMARY KEY,
    org_id       TEXT NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    user_id      TEXT REFERENCES users(id) ON DELETE SET NULL,
    name         TEXT NOT NULL,
    token_hash   TEXT NOT NULL UNIQUE,
    scopes       TEXT NOT NULL DEFAULT 'read',
    last_used_at TEXT,
    created_at   TEXT NOT NULL DEFAULT (datetime('now')),
    revoked_at   TEXT
);
CREATE INDEX IF NOT EXISTS idx_api_tokens_org ON api_tokens(org_id);

CREATE TABLE IF NOT EXISTS audit_log (
    id              TEXT PRIMARY KEY,
    org_id          TEXT REFERENCES organizations(id) ON DELETE SET NULL,
    actor_user_id   TEXT REFERENCES users(id) ON DELETE SET NULL,
    application_id  TEXT REFERENCES applications(id) ON DELETE SET NULL,
    action          TEXT NOT NULL,
    target_type     TEXT,
    target_id       TEXT,
    metadata        TEXT,
    created_at      TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_audit_org ON audit_log(org_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_audit_app ON audit_log(application_id, created_at DESC);

-- Tenant-scope existing applications into the default organization.
ALTER TABLE applications ADD COLUMN org_id TEXT NOT NULL DEFAULT 'default';
CREATE INDEX IF NOT EXISTS idx_applications_org ON applications(org_id);
