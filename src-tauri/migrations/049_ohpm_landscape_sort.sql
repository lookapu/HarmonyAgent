-- Migration 049: ohpm 三方库缓存新增官方排序指标列
-- 数据源：https://ohpm.openharmony.cn/ohpmweb/registry/oh-package/openapi/v1/search
--   （官网搜索接口，sortedType=likes/popularity/latest 全量分页拉取，全库约 3500 包）
-- 与 landscape 精选集（ide-page）按包名 join，补齐：
--   likes             点赞数（最受欢迎）
--   popularity        流行度（最流行）
--   latest_publish_time  最新发布时间（毫秒时间戳，最新发布）
-- 旧库刷新后自动填充；未刷新过的旧数据默认为 0。

ALTER TABLE ohpm_landscape ADD COLUMN likes INTEGER NOT NULL DEFAULT 0;
ALTER TABLE ohpm_landscape ADD COLUMN popularity INTEGER NOT NULL DEFAULT 0;
ALTER TABLE ohpm_landscape ADD COLUMN latest_publish_time INTEGER NOT NULL DEFAULT 0;

CREATE INDEX IF NOT EXISTS idx_ohpm_likes ON ohpm_landscape(likes);
CREATE INDEX IF NOT EXISTS idx_ohpm_popularity ON ohpm_landscape(popularity);
CREATE INDEX IF NOT EXISTS idx_ohpm_latest ON ohpm_landscape(latest_publish_time);
