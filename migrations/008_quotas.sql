-- P1-J: resource limits and per-org quotas.
--
-- Per-app ceilings enforced by the systemd runtime (MemoryMax/CPUQuota; the
-- proc runtime cannot confine, so limits with `proc` are rejected at the API).
-- Per-org quotas cap how much an organization may claim. Quota accounting sums
-- explicit limits only; unlimited apps count toward max_apps but not the
-- memory/CPU budgets.

ALTER TABLE applications ADD COLUMN mem_limit_mb INTEGER;
ALTER TABLE applications ADD COLUMN cpu_quota_pct INTEGER;

CREATE TABLE IF NOT EXISTS org_quotas (
    org_id      TEXT PRIMARY KEY REFERENCES organizations(id) ON DELETE CASCADE,
    max_apps    INTEGER NOT NULL DEFAULT 20,
    max_mem_mb  INTEGER NOT NULL DEFAULT 2048,
    max_cpu_pct INTEGER NOT NULL DEFAULT 200,
    max_domains INTEGER NOT NULL DEFAULT 10
);

INSERT OR IGNORE INTO org_quotas (org_id) VALUES ('default');
