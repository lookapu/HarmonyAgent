-- Migration 010: Task-level trace for agent runs（任务级执行轨迹与指标聚合）
CREATE TABLE IF NOT EXISTS task_runs (
    id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL,
    project_id TEXT NOT NULL DEFAULT '',
    provider_id TEXT,
    model TEXT,
    status TEXT NOT NULL DEFAULT 'success', -- success | error | cancelled
    error_kind TEXT,
    error_message TEXT,
    tool_rounds INTEGER NOT NULL DEFAULT 0,
    retry_count INTEGER NOT NULL DEFAULT 0,
    input_tokens INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    cost_cny REAL NOT NULL DEFAULT 0,
    duration_ms INTEGER NOT NULL DEFAULT 0,
    started_at INTEGER NOT NULL,
    finished_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_task_runs_project ON task_runs(project_id, started_at);
CREATE INDEX IF NOT EXISTS idx_task_runs_conv ON task_runs(conversation_id, started_at);
