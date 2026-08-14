-- 会话事件日志（事件溯源 Session：仅追加事件日志，消息历史可由事件回放派生）
-- 设计：补充真源 —— 现有 messages 表仍是主存储，session_events 记录消息生命周期
-- （用户消息/助手消息/工具调用/工具结果/系统说明），提供可回放的事件投影能力，
-- 为后续把事件日志升级为唯一真源铺路。
CREATE TABLE IF NOT EXISTS session_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    conversation_id TEXT NOT NULL,
    seq INTEGER NOT NULL,
    event_type TEXT NOT NULL,
    payload TEXT NOT NULL DEFAULT '{}',
    created_at INTEGER NOT NULL DEFAULT (unixepoch())
);
CREATE INDEX IF NOT EXISTS idx_session_events_conv_seq ON session_events(conversation_id, seq);
