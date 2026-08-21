-- EC11: versioned team share imports, reversible changes and project eval sets.
CREATE TABLE IF NOT EXISTS team_share_imports (
    batch_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    package_id TEXT NOT NULL,
    package_name TEXT NOT NULL,
    package_version TEXT NOT NULL,
    source_uri TEXT NOT NULL,
    source_revision TEXT NOT NULL,
    package_digest TEXT NOT NULL,
    state TEXT NOT NULL CHECK(state IN ('applied','reverted')),
    imported_at INTEGER NOT NULL,
    reverted_at INTEGER
);
CREATE INDEX IF NOT EXISTS idx_team_share_imports_project
ON team_share_imports(project_id, imported_at DESC);

CREATE TABLE IF NOT EXISTS team_share_changes (
    change_id TEXT PRIMARY KEY,
    batch_id TEXT NOT NULL REFERENCES team_share_imports(batch_id) ON DELETE CASCADE,
    item_kind TEXT NOT NULL CHECK(item_kind IN ('memory','convention','eval_set')),
    stable_key TEXT NOT NULL,
    local_id TEXT,
    action TEXT NOT NULL CHECK(action IN ('inserted','updated','staged_conflict','unchanged')),
    before_json TEXT,
    after_digest TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_team_share_changes_batch
ON team_share_changes(batch_id, item_kind, stable_key);

CREATE TABLE IF NOT EXISTS team_eval_sets (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    stable_key TEXT NOT NULL,
    name TEXT NOT NULL,
    version TEXT NOT NULL,
    cases_json TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    source_kind TEXT NOT NULL DEFAULT 'team_share',
    source_ref TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE(project_id, source_ref, stable_key)
);
CREATE INDEX IF NOT EXISTS idx_team_eval_sets_project
ON team_eval_sets(project_id, enabled, updated_at DESC);
