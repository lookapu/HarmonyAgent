-- Migration 036: 代码片段库（[06] snippet_insert 工具：自定义常用代码片段 CRUD）
CREATE TABLE IF NOT EXISTS snippets (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL UNIQUE,
    body TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    language TEXT NOT NULL DEFAULT 'ArkTS',
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_snippets_language ON snippets(language);
