-- EC09: explicit, fail-closed project authorization for MCP servers.
-- Existing rows remain visible in settings but are not exposed to Agent until configured.
ALTER TABLE mcp_servers ADD COLUMN authorization_state TEXT NOT NULL DEFAULT 'unconfigured';
ALTER TABLE mcp_servers ADD COLUMN allowed_tools TEXT NOT NULL DEFAULT '[]';
ALTER TABLE mcp_servers ADD COLUMN allowed_roots TEXT NOT NULL DEFAULT '["."]';
ALTER TABLE mcp_servers ADD COLUMN network_policy TEXT NOT NULL DEFAULT 'deny';
ALTER TABLE mcp_servers ADD COLUMN credential_keys TEXT NOT NULL DEFAULT '[]';

CREATE INDEX IF NOT EXISTS idx_mcp_project_authorized
ON mcp_servers(project_id, enabled, authorization_state);
