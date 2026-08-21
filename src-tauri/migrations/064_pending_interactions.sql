-- Durable pending user interactions. The live oneshot channel remains process-local;
-- this table is the auditable source for recovery and post-crash explanation.

CREATE TABLE IF NOT EXISTS pending_interactions (
    request_id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    run_id TEXT,
    kind TEXT NOT NULL CHECK(kind IN ('tool_approval', 'plan_review', 'ask_user', 'diagnose')),
    state TEXT NOT NULL DEFAULT 'pending' CHECK(state IN (
        'pending', 'approved', 'rejected', 'answered', 'skipped',
        'timed_out', 'cancelled', 'interrupted'
    )),
    payload_json TEXT NOT NULL DEFAULT '{}',
    response_json TEXT,
    owner_worker_id TEXT,
    expires_at INTEGER,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    resolved_at INTEGER
);

CREATE INDEX IF NOT EXISTS idx_pending_interactions_conversation
ON pending_interactions(conversation_id, state, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_pending_interactions_run
ON pending_interactions(run_id, state, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_pending_interactions_owner
ON pending_interactions(owner_worker_id, state, expires_at);
