-- Migration 020: 数据层清理。
-- 1. 删除从未被读写的 versions 幽灵表（历史上用于记录已安装 Agent 版本，实际版本查询走 npm 实时命令）。
-- 2. 清理无外键约束表中的孤儿数据：删除会话/项目后残留的 task_runs、message_feedback、message_versions。
--    这些表在 010/012 中未声明 REFERENCES + ON DELETE CASCADE，会话删除时不会级联，长期累积垃圾行。

DROP TABLE IF EXISTS versions;

-- 孤儿 task_runs：conversation_id 指向不存在的会话
DELETE FROM task_runs
WHERE conversation_id NOT IN (SELECT id FROM conversations);

-- 孤儿 message_feedback：message_id 指向不存在的消息
DELETE FROM message_feedback
WHERE message_id NOT IN (SELECT id FROM messages);

-- 孤儿 message_versions：conversation_id 指向不存在的会话
DELETE FROM message_versions
WHERE conversation_id NOT IN (SELECT id FROM conversations);

-- 孤儿 tool_runs（理论上 004 已建外键级联，这里对历史库做一次兜底）
DELETE FROM tool_runs
WHERE conversation_id NOT IN (SELECT id FROM conversations);
