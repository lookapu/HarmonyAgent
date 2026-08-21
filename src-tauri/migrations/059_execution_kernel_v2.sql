-- Agent Execution Kernel V2：可认领持久队列、工具协议 V2、DAG 调度与企业治理。
ALTER TABLE agent_task_queue ADD COLUMN payload_json TEXT NOT NULL DEFAULT '{}';
ALTER TABLE agent_task_queue ADD COLUMN resume_token TEXT;
ALTER TABLE agent_task_queue ADD COLUMN claimed_at INTEGER;
ALTER TABLE agent_task_queue ADD COLUMN last_checkpoint_at INTEGER;
ALTER TABLE agent_task_queue ADD COLUMN next_attempt_at INTEGER;
ALTER TABLE agent_task_queue ADD COLUMN concurrency_key TEXT;
ALTER TABLE agent_task_queue ADD COLUMN tenant_id TEXT NOT NULL DEFAULT 'local';

CREATE INDEX IF NOT EXISTS idx_agent_task_queue_dispatch
ON agent_task_queue(state,next_attempt_at,priority DESC,created_at ASC);
CREATE INDEX IF NOT EXISTS idx_agent_task_queue_concurrency
ON agent_task_queue(concurrency_key,state);

ALTER TABLE tool_runs ADD COLUMN protocol_version INTEGER NOT NULL DEFAULT 1;
ALTER TABLE tool_runs ADD COLUMN error_code TEXT;
ALTER TABLE tool_runs ADD COLUMN compensation_json TEXT;
ALTER TABLE tool_runs ADD COLUMN metrics_json TEXT;

ALTER TABLE agent_dag_nodes ADD COLUMN attempt INTEGER NOT NULL DEFAULT 1;
ALTER TABLE agent_dag_nodes ADD COLUMN max_attempts INTEGER NOT NULL DEFAULT 2;
ALTER TABLE agent_dag_nodes ADD COLUMN next_attempt_at INTEGER;
ALTER TABLE agent_dag_nodes ADD COLUMN condition_json TEXT NOT NULL DEFAULT '{}';
ALTER TABLE agent_dag_nodes ADD COLUMN failure_policy TEXT NOT NULL DEFAULT 'fail_fast';
ALTER TABLE agent_dag_nodes ADD COLUMN concurrency_key TEXT;
ALTER TABLE agent_dag_edges ADD COLUMN condition_json TEXT NOT NULL DEFAULT '{}';
ALTER TABLE agent_dag_edges ADD COLUMN required INTEGER NOT NULL DEFAULT 1;

CREATE INDEX IF NOT EXISTS idx_agent_dag_runnable
ON agent_dag_nodes(root_run_id,state,next_attempt_at,created_at);

CREATE TABLE IF NOT EXISTS agent_slo_policies (
    policy_id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL DEFAULT 'local',
    name TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    acceptance_target REAL NOT NULL DEFAULT 0.95,
    recovery_target REAL NOT NULL DEFAULT 0.90,
    evidence_target REAL NOT NULL DEFAULT 0.95,
    max_duration_ms INTEGER NOT NULL DEFAULT 3600000,
    max_cost_cny REAL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS agent_alerts (
    alert_id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL DEFAULT 'local',
    run_id TEXT,
    policy_id TEXT,
    severity TEXT NOT NULL,
    code TEXT NOT NULL,
    message TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'open',
    details_json TEXT NOT NULL DEFAULT '{}',
    created_at INTEGER NOT NULL,
    resolved_at INTEGER
);
CREATE INDEX IF NOT EXISTS idx_agent_alerts_open
ON agent_alerts(tenant_id,state,severity,created_at DESC);

CREATE TABLE IF NOT EXISTS agent_audit_events (
    audit_id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL DEFAULT 'local',
    run_id TEXT,
    conversation_id TEXT,
    actor TEXT NOT NULL,
    action TEXT NOT NULL,
    resource TEXT NOT NULL,
    outcome TEXT NOT NULL,
    details_json TEXT NOT NULL DEFAULT '{}',
    created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_agent_audit_run
ON agent_audit_events(run_id,created_at);

CREATE TABLE IF NOT EXISTS agent_quota_usage (
    tenant_id TEXT NOT NULL DEFAULT 'local',
    period TEXT NOT NULL,
    runs INTEGER NOT NULL DEFAULT 0,
    tool_calls INTEGER NOT NULL DEFAULT 0,
    failed_tools INTEGER NOT NULL DEFAULT 0,
    duration_ms INTEGER NOT NULL DEFAULT 0,
    cost_cny REAL NOT NULL DEFAULT 0,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY(tenant_id,period)
);

INSERT OR IGNORE INTO agent_slo_policies
(policy_id,tenant_id,name,enabled,acceptance_target,recovery_target,evidence_target,max_duration_ms,created_at,updated_at)
VALUES ('local-default','local','Local Agent SLO',1,0.95,0.90,0.95,3600000,unixepoch()*1000,unixepoch()*1000);
