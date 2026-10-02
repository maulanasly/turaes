-- N1b: agent credentials on the server registry.
-- agent_token_hash lets the control plane authenticate a heartbeat by token
-- without storing it in plaintext; agent_token_enc keeps a sealed copy.

ALTER TABLE servers ADD COLUMN agent_token_hash TEXT;
ALTER TABLE servers ADD COLUMN agent_token_enc TEXT;
CREATE INDEX IF NOT EXISTS idx_servers_agent_token ON servers(agent_token_hash);
