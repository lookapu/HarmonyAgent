-- 007_model_enabled: 模型启用开关
-- 注意：本文件发布后不可修改，变更走 008 递增编号（迁移纪律）
ALTER TABLE models ADD COLUMN enabled INTEGER NOT NULL DEFAULT 1;
