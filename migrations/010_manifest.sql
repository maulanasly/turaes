-- Manifest v1: application kinds beyond long-lived services.
--
-- kind: 'service' (default, current behavior), 'static' (publish a directory,
-- served by the embedded file server, always healthy), 'worker' (supervised
-- background process: no port, no health gate, no proxy route).
-- command: JSON argv array for interpreted apps (e.g. ["/opt/venv/bin/python",
-- "worker.py"]); mutually exclusive with the single-binary path in the
-- manifest, stored here for unit rendering.
-- workdir: WorkingDirectory override (default {state_dir}/{app}).
-- publish_dir: source directory synced for 'static' apps.

ALTER TABLE applications ADD COLUMN kind TEXT NOT NULL DEFAULT 'service';
ALTER TABLE applications ADD COLUMN command TEXT;
ALTER TABLE applications ADD COLUMN workdir TEXT;
ALTER TABLE applications ADD COLUMN publish_dir TEXT;
