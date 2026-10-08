-- UX baseline fixtures for docs/UX-BASELINE.md tasks T1–T5.
-- Run against a fresh dev DB (server boots migrations on first run).
-- Statuses mirror the fleet; the health loop is frozen during sessions via
-- TURAES_MONITOR_INTERVAL_SECS=86400 so rows stay as seeded.
INSERT INTO applications (id, name, binary_path, port, status, kind, domain) VALUES
 ('ux1', 'beruang',      '/srv/beruang/target/release/beruang-gateway',   8001, 'running',   'service', 'beruang.localhost'),
 ('ux2', 'monthly-logs', '/srv/monthly-logs/target/release/monthly-logs', 8002, 'failed',    'service', 'logs.localhost'),
 ('ux3', 'pdf-gen',      '/srv/pdf-gen/target/release/pdfgen',             8003, 'deploying', 'service', 'pdf.localhost'),
 ('ux4', 'uangku',       '/srv/uangku/target/release/uangku',              8004, 'stopped',   'worker',  NULL);
INSERT INTO deployments (id, application_id, status, artifact_hash, log, finished_at) VALUES
 ('uxd1', 'ux1', 'running', 'abc123def4567890abc123def4567890abc123de', 'slot B cut over; health 200 in 3 checks', datetime('now')),
 ('uxd2', 'ux1', 'queued',  '7890abcdef127890abcdef127890abcdef127890ab', NULL, NULL);
