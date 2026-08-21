-- EC12: confirmed, redacted and integrity-verifiable reproduction bundle exports.
CREATE TABLE IF NOT EXISTS reproduction_bundles (
    bundle_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    conversation_id TEXT REFERENCES conversations(id) ON DELETE SET NULL,
    run_id TEXT REFERENCES agent_runs(run_id) ON DELETE SET NULL,
    title TEXT NOT NULL,
    preview_digest TEXT NOT NULL,
    archive_rel_path TEXT NOT NULL,
    archive_sha256 TEXT NOT NULL,
    archive_bytes INTEGER NOT NULL,
    entry_count INTEGER NOT NULL,
    redacted_entry_count INTEGER NOT NULL,
    generated_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_reproduction_bundles_project
ON reproduction_bundles(project_id, generated_at DESC);
