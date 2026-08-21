-- Audit trail for every natural-language summary versus structured Context V2 facts.

CREATE TABLE IF NOT EXISTS conversation_context_reconciliations (
    id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    run_id TEXT,
    summary_digest TEXT NOT NULL,
    facts_digest TEXT,
    status TEXT NOT NULL CHECK(status IN ('consistent', 'corrected')),
    conflicts_json TEXT NOT NULL DEFAULT '[]',
    authoritative_block TEXT NOT NULL DEFAULT '',
    created_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_context_reconciliations_conversation
ON conversation_context_reconciliations(conversation_id, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_context_reconciliations_run
ON conversation_context_reconciliations(run_id, created_at DESC);
