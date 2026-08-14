-- 每条回复用时（ms）：assistant 消息记录该次任务从开始到完成的耗时，
-- 前端在回复正文上方展示“⏱ 用时 mm:ss”，历史对话同样可见
ALTER TABLE messages ADD COLUMN duration_ms INTEGER;
