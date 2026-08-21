-- Migration 068: durable conversation branch lineage and structured-only merges.

CREATE TABLE IF NOT EXISTS conversation_branches (
    id TEXT PRIMARY KEY,
    source_conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    branch_conversation_id TEXT NOT NULL UNIQUE REFERENCES conversations(id) ON DELETE CASCADE,
    anchor_kind TEXT NOT NULL CHECK(anchor_kind IN ('latest','message','checkpoint','build_failure','git_commit')),
    anchor_ref TEXT,
    anchor_message_rowid INTEGER,
    created_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_conversation_branches_source
    ON conversation_branches(source_conversation_id, created_at DESC);

CREATE TABLE IF NOT EXISTS conversation_branch_merges (
    id TEXT PRIMARY KEY,
    source_conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    target_conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    decisions_merged INTEGER NOT NULL DEFAULT 0,
    artifacts_merged INTEGER NOT NULL DEFAULT 0,
    evidence_merged INTEGER NOT NULL DEFAULT 0,
    manifest_json TEXT NOT NULL DEFAULT '{}',
    created_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_conversation_branch_merges_target
    ON conversation_branch_merges(target_conversation_id, created_at DESC);
