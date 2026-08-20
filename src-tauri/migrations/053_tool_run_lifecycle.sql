-- Migration 053: durable tool-call lifecycle and trace correlation.
-- A row is inserted before execution and updated at terminal state, so a process crash leaves an
-- explicit `running` record that recovery/diagnostics can identify instead of losing the call.
ALTER TABLE tool_runs ADD COLUMN trace_id TEXT;
ALTER TABLE tool_runs ADD COLUMN call_id TEXT;
CREATE INDEX IF NOT EXISTS idx_toolruns_trace ON tool_runs(trace_id, created_at);
CREATE UNIQUE INDEX IF NOT EXISTS idx_toolruns_call_id ON tool_runs(call_id) WHERE call_id IS NOT NULL;
