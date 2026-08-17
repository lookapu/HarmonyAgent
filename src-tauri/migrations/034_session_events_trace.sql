-- Trace ID 串联：会话事件增加任务级 trace_id
-- 一次任务（一次用户消息触发的完整执行）的所有事件共享同一 trace_id，
-- 全链路可 grep；前端 timeline 视图按 trace_id 折叠。
ALTER TABLE session_events ADD COLUMN trace_id TEXT;
CREATE INDEX IF NOT EXISTS idx_session_events_trace ON session_events(trace_id);
