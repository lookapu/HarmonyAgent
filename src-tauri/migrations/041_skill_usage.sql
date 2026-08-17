-- Migration 041: Skill 调用记录（use_skill 工具每次显式调用落一条，供技能管理页/统计页展示）
CREATE TABLE IF NOT EXISTS skill_usage (
    id TEXT PRIMARY KEY,
    skill_id TEXT NOT NULL,
    skill_name TEXT NOT NULL,
    conversation_id TEXT,
    project_id TEXT NOT NULL DEFAULT '',
    created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_skill_usage_skill ON skill_usage(skill_id, created_at);
CREATE INDEX IF NOT EXISTS idx_skill_usage_project ON skill_usage(project_id, created_at);
CREATE INDEX IF NOT EXISTS idx_skill_usage_conv ON skill_usage(conversation_id, created_at);
