-- Agent Execution Kernel V3：多进程 Worker 注册、fencing token 与执行尝试账本。
ALTER TABLE agent_task_queue ADD COLUMN lease_token TEXT;
ALTER TABLE agent_task_queue ADD COLUMN claim_epoch INTEGER NOT NULL DEFAULT 0;
ALTER TABLE agent_task_queue ADD COLUMN last_worker_id TEXT;
ALTER TABLE agent_task_queue ADD COLUMN recovery_count INTEGER NOT NULL DEFAULT 0;

CREATE TABLE IF NOT EXISTS agent_workers (
    worker_id TEXT PRIMARY KEY,
    worker_kind TEXT NOT NULL DEFAULT 'desktop',
    pid INTEGER NOT NULL,
    hostname TEXT NOT NULL,
    version TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'active',
    capacity INTEGER NOT NULL DEFAULT 1,
    active_tasks INTEGER NOT NULL DEFAULT 0,
    started_at INTEGER NOT NULL,
    last_heartbeat_at INTEGER NOT NULL,
    draining_at INTEGER,
    stopped_at INTEGER,
    metadata_json TEXT NOT NULL DEFAULT '{}'
);
CREATE INDEX IF NOT EXISTS idx_agent_workers_liveness
ON agent_workers(state,last_heartbeat_at);

CREATE TABLE IF NOT EXISTS agent_task_attempts (
    task_id TEXT NOT NULL REFERENCES agent_task_queue(task_id) ON DELETE CASCADE,
    attempt INTEGER NOT NULL,
    worker_id TEXT NOT NULL,
    lease_token TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'running',
    checkpoint_json TEXT NOT NULL DEFAULT '{}',
    error TEXT,
    started_at INTEGER NOT NULL,
    last_heartbeat_at INTEGER NOT NULL,
    finished_at INTEGER,
    PRIMARY KEY(task_id,attempt)
);
CREATE INDEX IF NOT EXISTS idx_agent_task_attempts_worker
ON agent_task_attempts(worker_id,state,last_heartbeat_at);

CREATE INDEX IF NOT EXISTS idx_agent_task_queue_owner
ON agent_task_queue(worker_id,state,lease_expires_at);
