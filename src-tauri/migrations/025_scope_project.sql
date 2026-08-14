-- MCP 服务器与技能支持"用户级(全局)/项目级"作用域
-- project_id 为 NULL 表示用户级（对所有项目生效）；非 NULL 表示仅在该项目下生效
ALTER TABLE mcp_servers ADD COLUMN project_id TEXT;
ALTER TABLE skills ADD COLUMN project_id TEXT;

CREATE INDEX IF NOT EXISTS idx_mcp_servers_project ON mcp_servers(project_id);
CREATE INDEX IF NOT EXISTS idx_skills_project ON skills(project_id);
