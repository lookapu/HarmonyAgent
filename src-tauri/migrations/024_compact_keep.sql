-- 会话压缩水位：上次压缩后保留的历史条数（NULL = 从未压缩）。
-- 上下文可视条按此口径估算真实发送 token（摘要 + 最近 N 条），
-- 避免压缩不删消息导致可视条仍按全量消息估算、压缩后假性高占用。
ALTER TABLE conversations ADD COLUMN compact_keep INTEGER;
