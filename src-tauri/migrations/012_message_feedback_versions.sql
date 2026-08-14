-- 012_message_feedback_versions: 消息反馈（点赞/点踩+原因）+ 回复版本（重新生成保留旧版）
-- 1) messages 增加 reasoning 列：AI 思考过程（DeepSeek 推理模型流式输出，默认 NULL）
ALTER TABLE messages ADD COLUMN reasoning TEXT;

-- 2) 消息反馈：一条消息最多一条反馈（重新评价时覆盖）
CREATE TABLE IF NOT EXISTS message_feedback (
    id TEXT PRIMARY KEY,
    message_id TEXT NOT NULL UNIQUE,
    conversation_id TEXT NOT NULL,
    feedback TEXT NOT NULL CHECK(feedback IN ('like','dislike')),
    reason TEXT,
    comment TEXT,
    created_at INTEGER NOT NULL
);

-- 3) 回复版本：重新生成时旧回复移入此表（按用户消息分组保留多版历史）
CREATE TABLE IF NOT EXISTS message_versions (
    id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL,
    user_message_id TEXT NOT NULL,
    content TEXT NOT NULL,
    reasoning TEXT,
    model TEXT,
    created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_message_versions_conv ON message_versions(conversation_id, user_message_id);
