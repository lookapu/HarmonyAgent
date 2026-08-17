-- Migration 048: ohpm 三方库推荐缓存（官方 landscape 推荐区镜像）
-- 数据源：https://ohpm.openharmony.cn/ohpm/tech-map/ide-page
--   （ohpm 官网 landscape 页面调用的 IDE 版接口：免登录、无鉴权，一次请求返回全量包列表）
-- 每次刷新全量替换（约 1000+ 包、700KB，量小无需 diff）；
-- 字段含四级中英文分类 / 描述 / 关键词 / 60 天下载量 / 评分，可支撑离线检索与推荐。

CREATE TABLE IF NOT EXISTS ohpm_landscape (
    package_name    TEXT PRIMARY KEY,
    version         TEXT NOT NULL DEFAULT '',
    author_name     TEXT NOT NULL DEFAULT '',
    score           INTEGER NOT NULL DEFAULT 0,
    license         TEXT NOT NULL DEFAULT '',
    down_count_60d  INTEGER NOT NULL DEFAULT 0,
    description     TEXT NOT NULL DEFAULT '',
    keywords        TEXT NOT NULL DEFAULT '',
    file_nums       INTEGER NOT NULL DEFAULT 0,
    file_size       INTEGER NOT NULL DEFAULT 0,
    level1_cn       TEXT NOT NULL DEFAULT '',
    level1_en       TEXT NOT NULL DEFAULT '',
    level2_cn       TEXT NOT NULL DEFAULT '',
    level2_en       TEXT NOT NULL DEFAULT '',
    level3_cn       TEXT NOT NULL DEFAULT '',
    level3_en       TEXT NOT NULL DEFAULT '',
    level4_cn       TEXT NOT NULL DEFAULT '',
    level4_en       TEXT NOT NULL DEFAULT '',
    updated_at      INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_ohpm_l1_cn ON ohpm_landscape(level1_cn);
CREATE INDEX IF NOT EXISTS idx_ohpm_l2_cn ON ohpm_landscape(level2_cn);
CREATE INDEX IF NOT EXISTS idx_ohpm_dl ON ohpm_landscape(down_count_60d);
CREATE INDEX IF NOT EXISTS idx_ohpm_score ON ohpm_landscape(score);
