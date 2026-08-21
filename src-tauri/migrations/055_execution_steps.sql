-- Durable execution graph：计划项与实际工具调用统一映射为可恢复步骤。
CREATE TABLE IF NOT EXISTS execution_steps (
    step_id TEXT PRIMARY KEY,
    run_id TEXT NOT NULL REFERENCES agent_runs(run_id) ON DELETE CASCADE,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    source TEXT NOT NULL,
    external_id TEXT NOT NULL,
    ordinal INTEGER NOT NULL DEFAULT 0,
    title TEXT NOT NULL,
    tool_name TEXT,
    input_hash TEXT,
    state TEXT NOT NULL DEFAULT 'pending',
    effect_kind TEXT NOT NULL DEFAULT 'read',
    recovery_policy TEXT NOT NULL DEFAULT 'replay',
    verification_state TEXT NOT NULL DEFAULT 'not_required',
    result_summary TEXT,
    started_at INTEGER,
    updated_at INTEGER NOT NULL,
    finished_at INTEGER,
    UNIQUE(run_id, source, external_id)
);

CREATE INDEX IF NOT EXISTS idx_execution_steps_run
ON execution_steps(run_id, ordinal, updated_at);

CREATE INDEX IF NOT EXISTS idx_execution_steps_recovery
ON execution_steps(run_id, state, recovery_policy);
