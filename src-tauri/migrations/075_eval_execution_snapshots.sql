-- EC14：为固定评测保存可复核的执行环境与最终证据；默认值兼容历史记录。
ALTER TABLE agent_eval_runs ADD COLUMN snapshot_schema_version INTEGER NOT NULL DEFAULT 0;
ALTER TABLE agent_eval_runs ADD COLUMN snapshot_json TEXT NOT NULL DEFAULT '{}';
ALTER TABLE agent_eval_runs ADD COLUMN duration_ms INTEGER NOT NULL DEFAULT 0;
ALTER TABLE agent_eval_runs ADD COLUMN evidence_digest TEXT;
