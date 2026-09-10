-- Desktop checkpoint recovery reads the newest rows at or below a rowid high-water mark.
-- SQLite appends rowid to ordinary index keys, so equality on these prefixes can walk the
-- matching rows directly in reverse rowid order without sorting an entire long conversation.
CREATE INDEX IF NOT EXISTS idx_messages_recovery_cursor
ON messages(conversation_id, queued, hidden);

CREATE INDEX IF NOT EXISTS idx_tool_runs_recovery_cursor
ON tool_runs(trace_id);
