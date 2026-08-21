-- Conversation Context V2：长会话分层上下文、事实来源与摘要游标。
-- messages/session_events/run_events 仍保存原始事实；本迁移只建立可重建的上下文投影，
-- 摘要不得成为文件、工具、Run 或外部状态的唯一真源。

CREATE TABLE IF NOT EXISTS conversation_context_state (
    conversation_id TEXT PRIMARY KEY REFERENCES conversations(id) ON DELETE CASCADE,
    schema_version INTEGER NOT NULL DEFAULT 2,
    summary TEXT,
    summary_from_message_rowid INTEGER NOT NULL DEFAULT 0,
    summary_to_message_rowid INTEGER NOT NULL DEFAULT 0,
    summary_event_seq INTEGER NOT NULL DEFAULT 0,
    task_snapshot_json TEXT NOT NULL DEFAULT '{}',
    budget_json TEXT NOT NULL DEFAULT '{}',
    facts_digest TEXT,
    invalidation_epoch INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS conversation_context_facts (
    id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    project_id TEXT,
    run_id TEXT,
    fact_kind TEXT NOT NULL,
    fact_key TEXT NOT NULL,
    value_json TEXT NOT NULL,
    source_kind TEXT NOT NULL,
    source_ref TEXT NOT NULL,
    scope TEXT NOT NULL DEFAULT 'conversation',
    confidence REAL NOT NULL DEFAULT 1.0 CHECK(confidence >= 0.0 AND confidence <= 1.0),
    version INTEGER NOT NULL DEFAULT 1,
    observed_at INTEGER NOT NULL,
    invalidated_at INTEGER,
    invalidation_reason TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_context_facts_active_key
ON conversation_context_facts(conversation_id, fact_kind, fact_key)
WHERE invalidated_at IS NULL;

CREATE INDEX IF NOT EXISTS idx_context_facts_project
ON conversation_context_facts(project_id, scope, updated_at DESC);

CREATE INDEX IF NOT EXISTS idx_context_facts_source
ON conversation_context_facts(source_kind, source_ref);

CREATE TABLE IF NOT EXISTS conversation_context_artifacts (
    id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    run_id TEXT,
    artifact_kind TEXT NOT NULL,
    uri TEXT NOT NULL,
    label TEXT NOT NULL DEFAULT '',
    digest TEXT,
    metadata_json TEXT NOT NULL DEFAULT '{}',
    source_ref TEXT NOT NULL,
    valid INTEGER NOT NULL DEFAULT 1,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE(conversation_id, artifact_kind, uri)
);

CREATE INDEX IF NOT EXISTS idx_context_artifacts_conversation
ON conversation_context_artifacts(conversation_id, valid, updated_at DESC);

CREATE TABLE IF NOT EXISTS conversation_context_snapshots (
    id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    run_id TEXT,
    schema_version INTEGER NOT NULL DEFAULT 2,
    message_rowid INTEGER NOT NULL DEFAULT 0,
    event_seq INTEGER NOT NULL DEFAULT 0,
    summary TEXT,
    task_snapshot_json TEXT NOT NULL DEFAULT '{}',
    budget_json TEXT NOT NULL DEFAULT '{}',
    facts_digest TEXT,
    created_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_context_snapshots_cursor
ON conversation_context_snapshots(conversation_id, message_rowid DESC, created_at DESC);
