-- Explicit per-app maintenance mode: the proxy parks the app's hostnames
-- (503 maintenance page) while units keep running for instant restore.
-- Independent of supervisor status; toggled via the maintenance endpoint.
ALTER TABLE applications ADD COLUMN maintenance INTEGER NOT NULL DEFAULT 0;
