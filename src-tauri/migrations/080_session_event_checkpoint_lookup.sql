-- 活跃 executor 恢复按 conversation + trace + event type 读取最新安全点。
-- 复合索引避免超长会话在每次恢复时倒序扫描无关消息与工具事件。
CREATE INDEX IF NOT EXISTS idx_session_events_checkpoint_lookup
ON session_events(conversation_id, trace_id, event_type, seq DESC);
