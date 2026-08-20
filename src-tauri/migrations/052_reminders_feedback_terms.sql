-- 052_reminders_feedback_terms: 会话内定时提醒 + 记忆纠偏词袋
-- 1) 定时提醒（对齐 deepseek-harness schedule 子系统）：after/at 一次性 + every 固定间隔
--    重复三类；到期以普通对话消息注入原会话（session-local，不中断当前轮次，无外部通知渠道）
CREATE TABLE IF NOT EXISTS message_reminders (
    id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL,
    kind TEXT NOT NULL CHECK(kind IN ('after','at','every')),
    prompt TEXT NOT NULL,
    scheduled_at INTEGER NOT NULL,
    every_seconds INTEGER,
    active INTEGER NOT NULL DEFAULT 1,
    created_at INTEGER NOT NULL,
    last_dispatch_at INTEGER
);
CREATE INDEX IF NOT EXISTS idx_message_reminders_due ON message_reminders(active, scheduled_at);

-- 2) 记忆纠偏词袋（对齐 deepseek-harness feedback 的消费侧）：用户 dislike 某条回复后，
--    其内容高频词按负面计数；记忆注入检索时对命中负面词的条目录降权/排除，
--    like 为正向加权——让"用户嫌弃的答案"不再反复出现
CREATE TABLE IF NOT EXISTS feedback_terms (
    project_id TEXT NOT NULL,
    term TEXT NOT NULL,
    polarity TEXT NOT NULL CHECK(polarity IN ('neg','pos')),
    count INTEGER NOT NULL DEFAULT 1,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (project_id, term, polarity)
);
