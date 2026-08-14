-- Migration 002: Request logs and cost tracking
CREATE TABLE IF NOT EXISTS request_logs (
    id TEXT PRIMARY KEY,
    provider_id TEXT REFERENCES providers(id) ON DELETE SET NULL,
    model TEXT,
    input_tokens INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    cache_read_tokens INTEGER NOT NULL DEFAULT 0,
    cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
    total_cost_cny REAL NOT NULL DEFAULT 0,
    latency_ms INTEGER,
    first_token_ms INTEGER,
    status_code INTEGER,
    error_message TEXT,
    session_id TEXT,
    is_streaming INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_logs_created ON request_logs(created_at DESC);
CREATE INDEX IF NOT EXISTS idx_logs_provider ON request_logs(provider_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_logs_model ON request_logs(model);

CREATE TABLE IF NOT EXISTS usage_daily (
    date TEXT NOT NULL,
    provider_id TEXT,
    model TEXT,
    request_count INTEGER NOT NULL DEFAULT 0,
    input_tokens INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    total_cost_cny REAL NOT NULL DEFAULT 0,
    PRIMARY KEY (date, provider_id, model)
);

CREATE TABLE IF NOT EXISTS model_pricing (
    model_id TEXT PRIMARY KEY,
    display_name TEXT,
    input_cost_per_mtok REAL NOT NULL DEFAULT 0,
    output_cost_per_mtok REAL NOT NULL DEFAULT 0,
    cache_read_cost_per_mtok REAL NOT NULL DEFAULT 0,
    cache_creation_cost_per_mtok REAL NOT NULL DEFAULT 0,
    currency TEXT NOT NULL DEFAULT 'CNY'
);
