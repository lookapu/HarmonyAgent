-- Migration 043: 局域网多令牌（含有效期/撤销）
-- 由单 token_hash 升级为多令牌：每个令牌独立哈希+盐+有效期，可单独生成/撤销。
CREATE TABLE IF NOT EXISTS lan_tokens (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL DEFAULT '',              -- 备注名称（如"手机""平板"）
    token_hash TEXT NOT NULL,                   -- sha256(salt+token) 十六进制哈希
    token_salt TEXT NOT NULL,                   -- 每个令牌独立的随机盐
    expires_at INTEGER NOT NULL DEFAULT 0,      -- 到期时间戳（unix 秒，0=永不过期）
    created_at INTEGER NOT NULL,                -- 创建时间戳
    last_used_at INTEGER NOT NULL DEFAULT 0     -- 最近一次成功鉴权时间戳
);

-- 平滑迁移：把旧 lan_config 里的单令牌搬进 lan_tokens（永不过期），
-- lan_config 仅保留 开关/端口/只读/失败锁定 等全局状态。
INSERT INTO lan_tokens (name, token_hash, token_salt, expires_at, created_at, last_used_at)
SELECT '默认令牌', token_hash, token_salt, 0, CAST(strftime('%s','now') AS INTEGER), 0
FROM lan_config WHERE token_hash != '';
