-- 长会话健康度指标（长会话加固）：压缩次数与最近压缩时间。
-- compress_count 由主动压缩/超限压缩/手动压缩三处统一递增（bump_compress_count），
-- 供会话健康度与摘要退化检测使用（compute_session_health）。

ALTER TABLE conversation_context_state ADD COLUMN compress_count INTEGER NOT NULL DEFAULT 0;
ALTER TABLE conversation_context_state ADD COLUMN last_compress_at INTEGER;
