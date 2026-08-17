-- Migration 042: 局域网访问（LAN Access）配置
-- 控制进程内 HTML 服务器的开关/端口/token/只读模式，以及鉴权失败锁定状态。
CREATE TABLE IF NOT EXISTS lan_config (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    enabled INTEGER NOT NULL DEFAULT 0,        -- 总开关（随应用启动自动开启）
    port INTEGER NOT NULL DEFAULT 12345,       -- 监听端口（被占用时自动顺延）
    token_hash TEXT NOT NULL DEFAULT '',       -- token 的 sha256(salt+token) 十六进制哈希
    token_salt TEXT NOT NULL DEFAULT '',       -- 每个 token 独立的随机盐
    read_only INTEGER NOT NULL DEFAULT 0,      -- 只读模式：写接口一律 403
    fail_count INTEGER NOT NULL DEFAULT 0,     -- 连续鉴权失败次数
    lock_until INTEGER NOT NULL DEFAULT 0      -- 锁定截止时间戳（unix 秒，0=未锁定）
);

INSERT OR IGNORE INTO lan_config (id) VALUES (1);
