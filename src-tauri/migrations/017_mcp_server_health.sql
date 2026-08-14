-- Migration 017: MCP server health status (last test result)
ALTER TABLE mcp_servers ADD COLUMN last_test_ok INTEGER;
ALTER TABLE mcp_servers ADD COLUMN last_test_at INTEGER;
ALTER TABLE mcp_servers ADD COLUMN last_test_error TEXT;
