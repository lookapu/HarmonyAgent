-- Durable Agent Runtime：任务状态与可补拉事件成为跨进程恢复的执行真源。
-- messages/session_events 继续保留兼容；agent_runs/run_events 负责运行生命周期。
CREATE TABLE IF NOT EXISTS agent_runs (
    run_id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    goal TEXT NOT NULL DEFAULT '',
    state TEXT NOT NULL DEFAULT 'running',
    phase TEXT NOT NULL DEFAULT 'initializing',
    attempt INTEGER NOT NULL DEFAULT 1,
    last_event_seq INTEGER NOT NULL DEFAULT 0,
    recovery_count INTEGER NOT NULL DEFAULT 0,
    resume_policy TEXT NOT NULL DEFAULT 'continue',
    acceptance_json TEXT,
    metadata_json TEXT NOT NULL DEFAULT '{}',
    error TEXT,
    started_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    finished_at INTEGER
);

CREATE INDEX IF NOT EXISTS idx_agent_runs_conversation
ON agent_runs(conversation_id, started_at DESC);

CREATE INDEX IF NOT EXISTS idx_agent_runs_state
ON agent_runs(state, updated_at);

-- 同一会话只能有一个非终态 durable run；与进程内 TaskRegistry 双层防并发。
CREATE UNIQUE INDEX IF NOT EXISTS idx_agent_runs_one_active
ON agent_runs(conversation_id)
WHERE state IN ('queued','running','waiting_approval','waiting_user','verifying');

CREATE TABLE IF NOT EXISTS run_events (
    event_id TEXT PRIMARY KEY,
    run_id TEXT NOT NULL REFERENCES agent_runs(run_id) ON DELETE CASCADE,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    seq INTEGER NOT NULL,
    event_type TEXT NOT NULL,
    payload TEXT NOT NULL DEFAULT '{}',
    created_at INTEGER NOT NULL,
    UNIQUE(run_id, seq)
);

CREATE INDEX IF NOT EXISTS idx_run_events_cursor
ON run_events(run_id, seq);

-- 工具调用恢复所需的副作用/幂等元数据。
ALTER TABLE tool_runs ADD COLUMN idempotency_key TEXT;
ALTER TABLE tool_runs ADD COLUMN effect_kind TEXT NOT NULL DEFAULT 'unknown';
ALTER TABLE tool_runs ADD COLUMN recovery_policy TEXT NOT NULL DEFAULT 'verify';
ALTER TABLE tool_runs ADD COLUMN prepared_at INTEGER;
ALTER TABLE tool_runs ADD COLUMN finished_at INTEGER;

CREATE INDEX IF NOT EXISTS idx_toolruns_recovery
ON tool_runs(status, recovery_policy, created_at);

CREATE UNIQUE INDEX IF NOT EXISTS idx_session_events_conv_seq_unique
ON session_events(conversation_id, seq);
