-- Skill manifest v1: explicit version, Agent compatibility, permissions and validation state.
ALTER TABLE skills ADD COLUMN manifest_schema INTEGER NOT NULL DEFAULT 0;
ALTER TABLE skills ADD COLUMN skill_version TEXT NOT NULL DEFAULT '0.0.0';
ALTER TABLE skills ADD COLUMN agent_compat TEXT;
ALTER TABLE skills ADD COLUMN permissions_json TEXT NOT NULL DEFAULT '[]';
ALTER TABLE skills ADD COLUMN compatibility_status TEXT NOT NULL DEFAULT 'legacy_unverified';
