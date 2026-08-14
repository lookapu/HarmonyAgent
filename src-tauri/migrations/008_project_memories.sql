-- Migration 008: Project memories（项目长期记忆，随 system_prompt 注入对话）
CREATE TABLE IF NOT EXISTS project_memories (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    category TEXT NOT NULL DEFAULT 'general',   -- general|code|build|deploy|decision|pitfall
    title TEXT NOT NULL,
    content TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,         -- 禁用后不再注入，但仍保留
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_memories_project ON project_memories(project_id, enabled);
