-- Recovery Orchestrator：新 Run 与被恢复 Run 建立明确血缘，并持久化机器可读恢复计划。
ALTER TABLE agent_runs ADD COLUMN parent_run_id TEXT;
ALTER TABLE agent_runs ADD COLUMN recovery_plan_json TEXT;
ALTER TABLE agent_runs ADD COLUMN recovery_mode TEXT NOT NULL DEFAULT 'fresh';

CREATE INDEX IF NOT EXISTS idx_agent_runs_parent
ON agent_runs(parent_run_id, started_at DESC);

