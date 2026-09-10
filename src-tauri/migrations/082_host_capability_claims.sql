-- Host Capability Broker 的跨进程派发台账。
-- 同一个规范化请求只能 claim 一次；started 行在崩溃后保留并阻止自动重放，
-- 因为外部设备上的副作用可能已经发生。
CREATE TABLE IF NOT EXISTS host_capability_claims (
    idempotency_key TEXT PRIMARY KEY,
    run_id TEXT NOT NULL REFERENCES agent_runs(run_id) ON DELETE CASCADE,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    tool_call_id TEXT NOT NULL,
    capability_id TEXT NOT NULL,
    request_digest TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'started'
        CHECK(status IN ('started','succeeded','failed','indeterminate')),
    subject_json TEXT NOT NULL DEFAULT '{}',
    claimed_at INTEGER NOT NULL,
    finished_at INTEGER,
    exit_code INTEGER,
    error_kind TEXT,
    UNIQUE(run_id, tool_call_id, capability_id, request_digest)
);

CREATE INDEX IF NOT EXISTS idx_host_capability_claims_run
ON host_capability_claims(run_id, claimed_at);

CREATE INDEX IF NOT EXISTS idx_host_capability_claims_status
ON host_capability_claims(status, claimed_at);
