-- Migration 067: user-controlled pins for context that compression must preserve.
CREATE TABLE IF NOT EXISTS conversation_context_pins (
    id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    pin_kind TEXT NOT NULL CHECK(pin_kind IN ('message','decision','file','acceptance')),
    source_ref TEXT NOT NULL,
    label TEXT NOT NULL,
    content TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE(conversation_id, pin_kind, source_ref)
);

CREATE INDEX IF NOT EXISTS idx_context_pins_conversation
    ON conversation_context_pins(conversation_id, updated_at DESC);
