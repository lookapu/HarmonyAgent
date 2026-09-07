-- 用户批准（或修订后批准）的执行计划属于 Durable Run，而不是仅存在于聊天循环内存。
-- 恢复 Run 会继承父 Run 的最终计划，确保重启/中断后仍按同一执行路径推进。
ALTER TABLE agent_runs ADD COLUMN approved_plan TEXT;

