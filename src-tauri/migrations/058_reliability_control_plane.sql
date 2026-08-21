-- Agent Reliability Control Plane：结构化工具证据、持久化调度、DAG 与评测历史。
ALTER TABLE tool_runs ADD COLUMN structured_result_json TEXT;
ALTER TABLE tool_runs ADD COLUMN evidence_digest TEXT;
ALTER TABLE tool_runs ADD COLUMN dag_node_id TEXT;

ALTER TABLE agent_runs ADD COLUMN scheduler_task_id TEXT;
ALTER TABLE agent_runs ADD COLUMN root_run_id TEXT;
ALTER TABLE agent_runs ADD COLUMN dag_node_id TEXT;
ALTER TABLE agent_runs ADD COLUMN budget_json TEXT;

CREATE TABLE IF NOT EXISTS agent_task_queue (
    task_id TEXT PRIMARY KEY,
    run_id TEXT NOT NULL UNIQUE REFERENCES agent_runs(run_id) ON DELETE CASCADE,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    goal TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'queued',
    priority INTEGER NOT NULL DEFAULT 50,
    worker_id TEXT,
    attempt INTEGER NOT NULL DEFAULT 0,
    max_attempts INTEGER NOT NULL DEFAULT 3,
    lease_expires_at INTEGER,
    budget_json TEXT NOT NULL DEFAULT '{}',
    checkpoint_json TEXT NOT NULL DEFAULT '{}',
    error TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    finished_at INTEGER
);
CREATE INDEX IF NOT EXISTS idx_agent_task_queue_claim
ON agent_task_queue(state, priority DESC, created_at ASC);
CREATE INDEX IF NOT EXISTS idx_agent_task_queue_lease
ON agent_task_queue(state, lease_expires_at);

CREATE TABLE IF NOT EXISTS agent_dag_nodes (
    node_id TEXT PRIMARY KEY,
    root_run_id TEXT NOT NULL REFERENCES agent_runs(run_id) ON DELETE CASCADE,
    run_id TEXT NOT NULL UNIQUE REFERENCES agent_runs(run_id) ON DELETE CASCADE,
    parent_node_id TEXT,
    name TEXT NOT NULL,
    goal TEXT NOT NULL,
    model TEXT,
    state TEXT NOT NULL DEFAULT 'pending',
    acceptance_json TEXT,
    output_summary TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    finished_at INTEGER
);
CREATE INDEX IF NOT EXISTS idx_agent_dag_nodes_root
ON agent_dag_nodes(root_run_id, created_at);

CREATE TABLE IF NOT EXISTS agent_dag_edges (
    root_run_id TEXT NOT NULL REFERENCES agent_runs(run_id) ON DELETE CASCADE,
    from_node_id TEXT NOT NULL,
    to_node_id TEXT NOT NULL,
    edge_kind TEXT NOT NULL DEFAULT 'depends_on',
    created_at INTEGER NOT NULL,
    PRIMARY KEY(root_run_id, from_node_id, to_node_id)
);

CREATE TABLE IF NOT EXISTS agent_eval_runs (
    eval_run_id TEXT PRIMARY KEY,
    suite TEXT NOT NULL,
    platform TEXT NOT NULL,
    passed INTEGER NOT NULL,
    total_cases INTEGER NOT NULL,
    passed_cases INTEGER NOT NULL,
    score REAL NOT NULL,
    threshold REAL NOT NULL,
    results_json TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_agent_eval_runs_created
ON agent_eval_runs(created_at DESC);
