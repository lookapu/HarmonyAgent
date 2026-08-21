-- Migration 066: make project memory a sourced, versioned Context V2 layer.
ALTER TABLE project_memories ADD COLUMN source_kind TEXT NOT NULL DEFAULT 'legacy_user';
ALTER TABLE project_memories ADD COLUMN source_ref TEXT NOT NULL DEFAULT '';
ALTER TABLE project_memories ADD COLUMN scope TEXT NOT NULL DEFAULT 'project';
ALTER TABLE project_memories ADD COLUMN confidence REAL NOT NULL DEFAULT 1.0;
ALTER TABLE project_memories ADD COLUMN version INTEGER NOT NULL DEFAULT 1;
ALTER TABLE project_memories ADD COLUMN confirmed INTEGER NOT NULL DEFAULT 1;
ALTER TABLE project_memories ADD COLUMN pinned INTEGER NOT NULL DEFAULT 0;
ALTER TABLE project_memories ADD COLUMN invalidation_condition TEXT NOT NULL DEFAULT '';
ALTER TABLE project_memories ADD COLUMN invalidated_at INTEGER;
ALTER TABLE project_memories ADD COLUMN invalidation_reason TEXT;

CREATE INDEX IF NOT EXISTS idx_memories_context
    ON project_memories(project_id, enabled, confirmed, pinned, invalidated_at, updated_at);
