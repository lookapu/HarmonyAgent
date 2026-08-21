-- Tool Quality Governance：工具级指标、目标贡献、比较维度与协议兼容目录。
ALTER TABLE tool_runs ADD COLUMN retry_count INTEGER NOT NULL DEFAULT 0;
ALTER TABLE tool_runs ADD COLUMN cancel_requested_at INTEGER;
ALTER TABLE tool_runs ADD COLUMN cancel_observed_at INTEGER;
ALTER TABLE tool_runs ADD COLUMN cancellation_latency_ms INTEGER;
ALTER TABLE tool_runs ADD COLUMN contribution_state TEXT NOT NULL DEFAULT 'unknown';
ALTER TABLE tool_runs ADD COLUMN selection_state TEXT NOT NULL DEFAULT 'unknown';
ALTER TABLE tool_runs ADD COLUMN capability_pack TEXT;
ALTER TABLE tool_runs ADD COLUMN model TEXT;
ALTER TABLE tool_runs ADD COLUMN project_id TEXT;
ALTER TABLE tool_runs ADD COLUMN producer_version TEXT;

CREATE INDEX IF NOT EXISTS idx_tool_runs_quality_window
ON tool_runs(created_at,status,error_code,retry_count);
CREATE INDEX IF NOT EXISTS idx_tool_runs_quality_dimensions
ON tool_runs(capability_pack,model,project_id,protocol_version,created_at);

ALTER TABLE agent_slo_policies ADD COLUMN max_side_effect_repeat_rate REAL NOT NULL DEFAULT 0.0;
ALTER TABLE agent_slo_policies ADD COLUMN max_wrong_tool_selection_rate REAL NOT NULL DEFAULT 0.05;
ALTER TABLE agent_slo_policies ADD COLUMN max_ineffective_call_rate REAL NOT NULL DEFAULT 0.25;

CREATE TABLE IF NOT EXISTS tool_protocol_versions (
    schema_version INTEGER PRIMARY KEY,
    status TEXT NOT NULL,
    min_reader_version INTEGER NOT NULL,
    producer_version TEXT NOT NULL,
    compatibility TEXT NOT NULL,
    migration_notes TEXT NOT NULL,
    created_at INTEGER NOT NULL
);

INSERT OR IGNORE INTO tool_protocol_versions
(schema_version,status,min_reader_version,producer_version,compatibility,migration_notes,created_at)
VALUES
(1,'legacy',1,'pre-2.0.0','read_supported_write_frozen',
 'V1 rows remain readable; all new writes use V2 and preserve unknown fields.',unixepoch()*1000),
(2,'current',1,'2.0.0','backward_read_and_unknown_field_tolerant',
 'V2 adds structured evidence, recovery and metrics; V1 readers must not consume V2-only fields.',unixepoch()*1000);
