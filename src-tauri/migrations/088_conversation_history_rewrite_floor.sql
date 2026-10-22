-- Migration 088: 历史改写水位（时间旅行/删除消息后，旧 Run 不再可被恢复）
--
-- 回到历史点或删除一段消息只改写 messages/ledger，agent_runs 与 run_events 属于
-- 审计真源必须保留。但前端重开会话时按「最近一次 Run」恢复未完成任务与检查点，
-- 会把已被归档的未来分支任务重新复活（甚至把其部分输出注入回退后的对话）。
-- 这里记录最近一次历史改写时间：早于该时刻启动的 Run 只是旧分支的审计记录，
-- 不再作为可恢复 Run 返回；改写之后新建的 Run 仍正常参与恢复。
ALTER TABLE conversations ADD COLUMN history_rewritten_at_ms INTEGER NOT NULL DEFAULT 0;
