-- 会话级滚动摘要持久化：上下文压缩时生成的摘要写回本列，
-- 新任务启动时先加载注入，保证早期对话要点跨任务继承（配合 chat.rs 主动预算压缩）
ALTER TABLE conversations ADD COLUMN summary TEXT;
