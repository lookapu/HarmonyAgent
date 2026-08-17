-- 消息分页查询索引：(conversation_id, created_at, id) 复合索引支撑
-- WHERE conversation_id = ? ORDER BY created_at DESC, id DESC LIMIT n 的游标分页
CREATE INDEX IF NOT EXISTS idx_messages_conv_time ON messages(conversation_id, created_at, id);
