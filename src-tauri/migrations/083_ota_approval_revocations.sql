-- 会话停止时固定撤销当前 OTA 调用，后续同调用不允许重新签发。
CREATE TABLE IF NOT EXISTS ota_approval_revocations (
    call_id TEXT PRIMARY KEY,
    run_id TEXT NOT NULL,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    revoked_at INTEGER NOT NULL,
    reason TEXT NOT NULL
);
