-- Migration 085: 本地使用统计（usage_events）
--
-- ## 它是什么
--
-- **只落本地、默认关闭**的使用统计。开关是 settings 表的 `telemetry_enabled` 键，
-- **默认不存在 = 关闭**（见 queries::usage_events_enabled）。
--
-- 它记的是「用户用了哪些功能」，与已有的 request_logs 互补：
--   - request_logs 记 LLM 请求（token / 成本 / 延迟），按 provider·model 聚合；
--   - usage_events 记**产品功能**被触达（打开哪个页面、用了哪个工具、Agent 成败）。
-- 后者回答的是「哪些功能有人在用、哪些是死代码」，前者回答的是「花了多少钱」。
--
-- ## 为什么事件名走白名单（不收任意字符串）
--
-- 埋点系统最经典的失败模式是事件名漂移：今天叫 `page_view`，
-- 明天叫 `pageView` / `view_page` / `navigate`，于是按事件名聚合的报表
-- 一半数据对不上，而且**没人会发现**——因为 SQL 不报错。
-- 故 record_usage_event 拒绝白名单外的事件名（见 queries.rs 的 EVENT_NAMES）。
--
-- ## 为什么 props 是 TEXT 而不是 JSON1
--
-- 只做「按事件名 + 时间窗」聚合，不做 props 内的过滤查询。
-- SQLite 的 json_extract 能做，但索引用不上、收益不抵复杂度。
--
-- ## 保留策略
--
-- 保留最近 20000 条，超出时删最旧的（queries::prune_usage_events）。
-- 本地统计不需要无限历史，而一个不裁剪的表会随使用量无声地吃掉磁盘。
-- 裁剪在每次写入后触发，单次 DELETE ... WHERE id NOT IN (最近 N) 有索引支撑。

CREATE TABLE IF NOT EXISTS usage_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    -- 白名单事件名，见 queries.rs::EVENT_NAMES
    event_name TEXT NOT NULL,
    -- 记录时的应用版本：便于按版本对比使用行为
    app_version TEXT NOT NULL DEFAULT '',
    -- JSON 字符串，附加上下文（如 { "page": "cost" }）。**不得含路径/用户名/API Key**
    props TEXT NOT NULL DEFAULT '{}',
    created_at INTEGER NOT NULL DEFAULT (unixepoch())
);
CREATE INDEX IF NOT EXISTS idx_usage_events_name_time ON usage_events(event_name, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_usage_events_time ON usage_events(created_at DESC);
