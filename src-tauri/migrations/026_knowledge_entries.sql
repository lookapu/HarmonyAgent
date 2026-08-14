-- 026: 用户自定义鸿蒙知识条目（团队踩坑经验沉淀，按需注入构建/部署失败结果）
CREATE TABLE IF NOT EXISTS knowledge_entries (
    id          TEXT PRIMARY KEY,
    -- 关键词，逗号分隔（小写存储；匹配时对错误文本 lower-case 后 contains）
    keywords    TEXT NOT NULL,
    title       TEXT NOT NULL,
    cause       TEXT NOT NULL DEFAULT '',
    fix         TEXT NOT NULL DEFAULT '',
    -- 用户条目 enabled=0 可临时禁用；内置条目始终 enabled
    enabled     INTEGER NOT NULL DEFAULT 1,
    builtin     INTEGER NOT NULL DEFAULT 0,
    -- 作用域：NULL=全局（所有项目生效）；非空=仅该项目生效（项目专属踩坑经验）
    project_id  TEXT,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER
);
CREATE INDEX IF NOT EXISTS idx_knowledge_enabled ON knowledge_entries(enabled);
CREATE INDEX IF NOT EXISTS idx_knowledge_project ON knowledge_entries(project_id);
