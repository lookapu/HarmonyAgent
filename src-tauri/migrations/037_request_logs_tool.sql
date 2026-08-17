-- Migration 037: request_logs 增加工具维度（[69] 最耗 token 工具统计）
-- 本地代理转发时从请求头 x-deveco-tool 读取工具名写入；
-- 直连调用等无工具上下文的请求为 NULL，不影响既有按模型聚合统计。
ALTER TABLE request_logs ADD COLUMN tool_name TEXT;
CREATE INDEX IF NOT EXISTS idx_logs_tool ON request_logs(tool_name, created_at DESC);
