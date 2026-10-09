-- Catalog: curated starter templates for one-click app creation.
-- Templates are platform-global (same for every org); the `GET
-- /api/v1/orgs/{org}/catalog` endpoint is org-scoped only for auth, so every
-- member sees the same rows. Adding a template is an INSERT, not a release.

CREATE TABLE IF NOT EXISTS catalog_templates (
    id            TEXT PRIMARY KEY,
    slug          TEXT NOT NULL UNIQUE,
    name          TEXT NOT NULL,
    description   TEXT NOT NULL DEFAULT '',
    kind          TEXT NOT NULL DEFAULT 'service',
    defaults_json TEXT NOT NULL DEFAULT '{}',
    sort_order    INTEGER NOT NULL DEFAULT 0,
    created_at    TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_catalog_templates_order
    ON catalog_templates(sort_order, slug);

-- `defaults_json` keys map 1:1 onto wizard draft fields (see
-- `static/js/lib/appForm.js` `applyTemplate`); unknown keys are ignored.
INSERT OR IGNORE INTO catalog_templates (id, slug, name, description, kind, defaults_json, sort_order)
VALUES
    ('tmpl_axum_service', 'axum-service', 'Rust web service',
     'Long-running HTTP service with health and metrics endpoints, routed through the proxy.',
     'service',
     '{"kind":"service","port":8000,"health_path":"/health","description":""}',
     10),
    ('tmpl_static_site', 'static-site', 'Static site',
     'Directory of files served by turaes itself. No process is started.',
     'static',
     '{"kind":"static","port":8001,"description":""}',
     20),
    ('tmpl_worker', 'worker', 'Background worker',
     'Supervised process with no port and no route: queues, jobs, watch loops.',
     'worker',
     '{"kind":"worker","description":""}',
     30);
