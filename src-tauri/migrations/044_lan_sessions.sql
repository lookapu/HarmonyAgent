-- Migration 044: 局域网访问会话记录（设备信息 + 使用时长）
-- 每次网页端建立 SSE 连接视为一次使用会话：记录设备类型、UA、起止时间与时长。
CREATE TABLE IF NOT EXISTS lan_sessions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    token_id INTEGER NOT NULL,                  -- 归属令牌（lan_tokens.id）
    device TEXT NOT NULL DEFAULT '',            -- 解析后的设备类型（如"手机 (iOS)"）
    user_agent TEXT NOT NULL DEFAULT '',        -- 原始 User-Agent
    started_at INTEGER NOT NULL,                -- 会话开始（unix 秒）
    ended_at INTEGER NOT NULL DEFAULT 0,        -- 会话结束（unix 秒，0=进行中）
    duration_secs INTEGER NOT NULL DEFAULT 0    -- 时长（秒，结束时写入）
);

CREATE INDEX IF NOT EXISTS idx_lan_sessions_token ON lan_sessions (token_id, id);
