-- 会话任务清单持久化（todo_write 工具状态）：重启应用后历史会话仍可恢复任务清单展示
-- 设计：会话级一行存储（JSON 快照），与进程内 SessionContext.todo_lists 互为缓存；
-- 写路径：todo_write/plan 镜像时同步 upsert；读路径：内存优先、库兜底。
CREATE TABLE IF NOT EXISTS conversation_todos (
    conversation_id TEXT PRIMARY KEY,
    items_json TEXT NOT NULL DEFAULT '[]',
    updated_at INTEGER NOT NULL DEFAULT (unixepoch())
);
