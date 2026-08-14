-- Migration 003: MCP servers and skills
CREATE TABLE IF NOT EXISTS mcp_servers (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    server_type TEXT NOT NULL DEFAULT 'local',
    command TEXT NOT NULL,
    args TEXT DEFAULT '[]',
    env TEXT DEFAULT '{}',
    enabled INTEGER NOT NULL DEFAULT 1,
    description TEXT,
    homepage TEXT,
    created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS skills (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    description TEXT,
    directory TEXT,
    repo_owner TEXT,
    repo_name TEXT,
    repo_branch TEXT DEFAULT 'main',
    enabled INTEGER NOT NULL DEFAULT 1,
    content_hash TEXT,
    installed_at INTEGER NOT NULL,
    updated_at INTEGER
);

CREATE TABLE IF NOT EXISTS skill_repos (
    owner TEXT NOT NULL,
    name TEXT NOT NULL,
    branch TEXT NOT NULL DEFAULT 'main',
    enabled INTEGER NOT NULL DEFAULT 1,
    PRIMARY KEY (owner, name)
);
