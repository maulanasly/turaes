-- P4: blue/green slots. `active_port` is the slot the proxy routes to (defaults
-- to `applications.port` when null). The other slot is `port + runtime.slot_offset`.

ALTER TABLE applications ADD COLUMN active_port INTEGER;
