-- Migration 051: 会话时间旅行（快照 + 分支归档）
-- 对齐 langgraph checkpoint 语义：每轮工具执行后保存会话状态快照
-- （消息锚点 rowid + 当时任务账本 + 模型输出摘要），用户可"回到此处"
-- 从历史决策点重新引导；快照点之后的消息 soft-delete（hidden）归档保留，
-- 恢复更早/更晚快照时按 rowid 双向切换可见性（旧分支保留可回溯，不硬删）。
--
-- conversation_snapshots: 快照列表（时间轴展示 + 恢复锚点）
-- messages.hidden: 1 = 归档分支消息（时间旅行后不可见，不参与模型上下文组装）

CREATE TABLE IF NOT EXISTS conversation_snapshots (
    id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    msg_rowid INTEGER NOT NULL,            -- 快照点最后一条可见消息的 rowid（恢复锚点）
    label TEXT NOT NULL DEFAULT '',        -- 模型输出摘要（时间轴展示，40 字符内）
    ledger_json TEXT,                      -- 当时任务账本（恢复时写回 conversations.ledger）
    tool_count INTEGER NOT NULL DEFAULT 0, -- 已执行工具数（时间轴展示）
    created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_snapshots_conv
    ON conversation_snapshots(conversation_id, created_at);

ALTER TABLE messages ADD COLUMN hidden INTEGER NOT NULL DEFAULT 0;
