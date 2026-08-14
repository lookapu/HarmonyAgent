-- Migration 001: Core tables
CREATE TABLE IF NOT EXISTS providers (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    provider_type TEXT NOT NULL DEFAULT 'openai-compatible',
    base_url TEXT NOT NULL,
    api_key TEXT,
    npm_package TEXT,
    is_active INTEGER NOT NULL DEFAULT 0,
    in_failover_queue INTEGER NOT NULL DEFAULT 0,
    priority INTEGER NOT NULL DEFAULT 0,
    cost_multiplier REAL NOT NULL DEFAULT 1.0,
    limit_daily_cny REAL,
    limit_monthly_cny REAL,
    settings_json TEXT DEFAULT '{}',
    notes TEXT,
    icon TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS models (
    id TEXT PRIMARY KEY,
    provider_id TEXT NOT NULL REFERENCES providers(id) ON DELETE CASCADE,
    model_id TEXT NOT NULL,
    display_name TEXT,
    tool_call INTEGER NOT NULL DEFAULT 1,
    context_limit INTEGER DEFAULT 200000,
    output_limit INTEGER DEFAULT 8192,
    input_modalities TEXT DEFAULT '["text"]',
    output_modalities TEXT DEFAULT '["text"]',
    input_price_per_mtok REAL DEFAULT 0,
    output_price_per_mtok REAL DEFAULT 0,
    is_default INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_models_provider ON models(provider_id, model_id);

CREATE TABLE IF NOT EXISTS versions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    version TEXT NOT NULL UNIQUE,
    install_path TEXT,
    is_active INTEGER NOT NULL DEFAULT 0,
    npm_tag TEXT,
    installed_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS endpoint_health (
    provider_id TEXT PRIMARY KEY REFERENCES providers(id) ON DELETE CASCADE,
    status TEXT NOT NULL DEFAULT 'unknown',
    latency_ms INTEGER,
    last_check_at INTEGER,
    last_success_at INTEGER,
    last_failure_at INTEGER,
    consecutive_failures INTEGER NOT NULL DEFAULT 0,
    circuit_state TEXT NOT NULL DEFAULT 'closed',
    circuit_opened_at INTEGER,
    error_message TEXT
);

CREATE TABLE IF NOT EXISTS proxy_config (
    id INTEGER PRIMARY KEY DEFAULT 1,
    enabled INTEGER NOT NULL DEFAULT 0,
    listen_address TEXT NOT NULL DEFAULT '127.0.0.1',
    listen_port INTEGER NOT NULL DEFAULT 15800,
    auto_failover INTEGER NOT NULL DEFAULT 0,
    max_retries INTEGER NOT NULL DEFAULT 3,
    streaming_first_byte_timeout_s INTEGER NOT NULL DEFAULT 60,
    streaming_idle_timeout_s INTEGER NOT NULL DEFAULT 120,
    non_streaming_timeout_s INTEGER NOT NULL DEFAULT 600,
    circuit_failure_threshold INTEGER NOT NULL DEFAULT 4,
    circuit_error_rate_threshold REAL NOT NULL DEFAULT 0.6
);

INSERT OR IGNORE INTO proxy_config (id) VALUES (1);

CREATE TABLE IF NOT EXISTS settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
