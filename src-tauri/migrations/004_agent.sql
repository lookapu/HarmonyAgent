-- 004_agent: Agent 功能数据模型（ARCHITECTURE.md §8 定稿）
-- 注意：本文件发布后不可修改，变更走 005 递增编号（迁移纪律）

CREATE TABLE IF NOT EXISTS projects (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    path TEXT NOT NULL UNIQUE,
    kind TEXT NOT NULL DEFAULT 'harmony',
    trusted INTEGER NOT NULL DEFAULT 0,          -- 是否已信任（§3.1）
    default_provider_id TEXT,
    default_model_id TEXT,
    index_state TEXT NOT NULL DEFAULT 'pending', -- pending|building|ready|failed
    rules TEXT,                                  -- 项目级指令（§10，追加在全局 Rules 后）
    last_opened_at INTEGER,
    created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS conversations (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    title TEXT NOT NULL DEFAULT '新会话',
    provider_id TEXT,
    model_id TEXT,
    system_prompt_version INTEGER,               -- 提示词版本快照（§10）
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS messages (
    id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    role TEXT NOT NULL,                    -- user|assistant|system
    content TEXT NOT NULL DEFAULT '',      -- 文本（assistant 为完整回复，含 Markdown）
    references_json TEXT,                  -- @ 引用列表（文件/会话/指令，§2.5）
    plan_json TEXT,                        -- 目标模式的计划卡片（步骤+状态）
    tool_calls_json TEXT,                  -- 该消息关联的工具调用数组（含卡片数据）
    model TEXT,
    tokens_in INTEGER,
    tokens_out INTEGER,
    summary TEXT,                          -- 上下文压缩时 fast 模型生成的摘要
    created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS tool_runs (
    id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    tool_name TEXT NOT NULL,
    input_json TEXT,
    result_json TEXT,
    status TEXT NOT NULL,                  -- running|ok|error|cancelled|ask
    card_type TEXT,                        -- tool|file|diff|build|deploy|ask
    duration_ms INTEGER,
    created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS project_index_cache (
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    kind TEXT NOT NULL,                    -- project|routes|modules|deps|build_errors
    data_json TEXT NOT NULL,
    built_at INTEGER NOT NULL,
    PRIMARY KEY (project_id, kind)
);

CREATE TABLE IF NOT EXISTS permissions (   -- 严格模式 L2 记忆（§3.4）
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    op_class TEXT NOT NULL,                -- delete|install_overwrite|cmd_other
    allow INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    UNIQUE(project_id, op_class)
);

CREATE INDEX IF NOT EXISTS idx_messages_conv ON messages(conversation_id);
CREATE INDEX IF NOT EXISTS idx_conv_project ON conversations(project_id);
CREATE INDEX IF NOT EXISTS idx_toolruns_conv ON tool_runs(conversation_id);
