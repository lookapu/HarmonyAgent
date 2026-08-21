-- EC10: unified provenance, detached signatures, quotas and durable circuit breakers.
CREATE TABLE IF NOT EXISTS extension_governance (
    extension_kind TEXT NOT NULL CHECK(extension_kind IN ('skill','mcp','workflow')),
    extension_id TEXT NOT NULL,
    project_id TEXT,
    source_uri TEXT,
    source_revision TEXT,
    content_sha256 TEXT NOT NULL,
    signature_algorithm TEXT,
    signer_key_id TEXT,
    signer_public_key TEXT,
    signature TEXT,
    verification_state TEXT NOT NULL DEFAULT 'unsigned'
      CHECK(verification_state IN ('unsigned','verified','invalid','drifted')),
    calls_per_minute INTEGER NOT NULL DEFAULT 60 CHECK(calls_per_minute BETWEEN 1 AND 10000),
    failure_threshold INTEGER NOT NULL DEFAULT 5 CHECK(failure_threshold BETWEEN 1 AND 100),
    cooldown_seconds INTEGER NOT NULL DEFAULT 60 CHECK(cooldown_seconds BETWEEN 1 AND 86400),
    window_started_at INTEGER NOT NULL DEFAULT 0,
    window_calls INTEGER NOT NULL DEFAULT 0,
    consecutive_failures INTEGER NOT NULL DEFAULT 0,
    circuit_open_until INTEGER,
    last_error TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY(extension_kind, extension_id)
);

CREATE INDEX IF NOT EXISTS idx_extension_governance_project
ON extension_governance(project_id, extension_kind, updated_at DESC);

-- Preserve compatibility while making legacy provenance explicit. Re-registration
-- replaces these sentinel digests with hashes of the actual extension payload.
INSERT OR IGNORE INTO extension_governance
(extension_kind,extension_id,project_id,source_uri,source_revision,content_sha256,
 verification_state,calls_per_minute,failure_threshold,cooldown_seconds,created_at,updated_at)
SELECT 'skill',id,project_id,
       CASE WHEN repo_host IS NULL OR repo_owner IS NULL OR repo_name IS NULL THEN NULL
            ELSE 'https://' || repo_host || '.com/' || repo_owner || '/' || repo_name END,
       repo_branch,COALESCE(content_hash,'sha256:legacy-unverified'),'unsigned',60,5,60,
       installed_at,COALESCE(updated_at,installed_at)
FROM skills;

INSERT OR IGNORE INTO extension_governance
(extension_kind,extension_id,project_id,source_uri,content_sha256,verification_state,
 calls_per_minute,failure_threshold,cooldown_seconds,created_at,updated_at)
SELECT 'mcp',id,project_id,homepage,'sha256:legacy-unverified','unsigned',60,5,60,
       created_at,created_at FROM mcp_servers;
