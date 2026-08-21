-- Tool Execution Kernel V2：独立执行器租约、fencing、恢复裁决和尝试账本。
ALTER TABLE tool_runs ADD COLUMN execution_worker_id TEXT;
ALTER TABLE tool_runs ADD COLUMN lease_token TEXT;
ALTER TABLE tool_runs ADD COLUMN attempt INTEGER NOT NULL DEFAULT 0;
ALTER TABLE tool_runs ADD COLUMN heartbeat_at INTEGER;
ALTER TABLE tool_runs ADD COLUMN lease_expires_at INTEGER;
ALTER TABLE tool_runs ADD COLUMN verification_state TEXT NOT NULL DEFAULT 'none';
ALTER TABLE tool_runs ADD COLUMN recovery_count INTEGER NOT NULL DEFAULT 0;
ALTER TABLE tool_runs ADD COLUMN outcome_committed_at INTEGER;

CREATE TABLE IF NOT EXISTS tool_execution_workers (
    worker_id TEXT PRIMARY KEY,
    process_worker_id TEXT NOT NULL,
    pid INTEGER NOT NULL,
    platform TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'active',
    capacity INTEGER NOT NULL DEFAULT 4,
    active_tools INTEGER NOT NULL DEFAULT 0,
    started_at INTEGER NOT NULL,
    last_heartbeat_at INTEGER NOT NULL,
    stopped_at INTEGER,
    metadata_json TEXT NOT NULL DEFAULT '{}'
);

CREATE TABLE IF NOT EXISTS tool_execution_attempts (
    call_id TEXT NOT NULL REFERENCES tool_runs(id) ON DELETE CASCADE,
    attempt INTEGER NOT NULL,
    worker_id TEXT NOT NULL,
    lease_token TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'running',
    started_at INTEGER NOT NULL,
    last_heartbeat_at INTEGER NOT NULL,
    finished_at INTEGER,
    outcome_digest TEXT,
    error TEXT,
    PRIMARY KEY(call_id, attempt)
);

CREATE INDEX IF NOT EXISTS idx_tool_execution_workers_liveness
ON tool_execution_workers(state,last_heartbeat_at);
CREATE INDEX IF NOT EXISTS idx_tool_execution_attempts_worker
ON tool_execution_attempts(worker_id,state,last_heartbeat_at);
CREATE INDEX IF NOT EXISTS idx_tool_runs_execution_lease
ON tool_runs(status,execution_worker_id,lease_expires_at);
CREATE INDEX IF NOT EXISTS idx_tool_runs_verification
ON tool_runs(verification_state,recovery_policy,created_at);
