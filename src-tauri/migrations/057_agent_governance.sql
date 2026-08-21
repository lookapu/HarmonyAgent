-- 商业级 Agent 治理：目标契约、补救计数、长任务租约与终态质量快照。
ALTER TABLE agent_runs ADD COLUMN goal_contract_json TEXT;
ALTER TABLE agent_runs ADD COLUMN remediation_count INTEGER NOT NULL DEFAULT 0;
ALTER TABLE agent_runs ADD COLUMN heartbeat_at INTEGER;
ALTER TABLE agent_runs ADD COLUMN lease_expires_at INTEGER;
ALTER TABLE agent_runs ADD COLUMN quality_json TEXT;

CREATE INDEX IF NOT EXISTS idx_agent_runs_lease
ON agent_runs(state, lease_expires_at)
WHERE state IN ('queued','running','waiting_approval','waiting_user','verifying');
