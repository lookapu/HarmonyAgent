-- Tool Execution Kernel V2：真实执行线程身份与卡死归因。
-- 工具执行迁移到专用 OS 线程后，登记线程身份便于观测；卡死（调用方已放弃等待但
-- 线程仍在运行）以 stuck_count 累计，作为停滞/卡顿控制面指标。
ALTER TABLE tool_execution_workers ADD COLUMN thread_id INTEGER;
ALTER TABLE tool_execution_workers ADD COLUMN thread_name TEXT;
ALTER TABLE tool_execution_workers ADD COLUMN stuck_count INTEGER NOT NULL DEFAULT 0;
