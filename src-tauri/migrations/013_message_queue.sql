-- Migration 013: 运行中提交的消息排队（调整方向 / 发送到 Agent）
-- queued=1：消息已入库但未提交给模型（流式运行时发送）
-- agent_owned=1：由 Agent 在任务内安全点并入当前任务（"发送到 Agent"按钮）
-- agent_owned=0：当前任务结束后自动续跑处理（普通排队）
ALTER TABLE messages ADD COLUMN queued INTEGER NOT NULL DEFAULT 0;
ALTER TABLE messages ADD COLUMN agent_owned INTEGER NOT NULL DEFAULT 0;
