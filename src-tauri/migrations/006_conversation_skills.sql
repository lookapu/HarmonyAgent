-- 006_conversation_skills: 会话置顶/归档 + 技能子目录导入
-- 注意：本文件发布后不可修改，变更走 007 递增编号（迁移纪律）

-- 会话是否置顶（1=置顶，排序优先）
ALTER TABLE conversations ADD COLUMN is_pinned INTEGER NOT NULL DEFAULT 0;

-- 会话是否归档（1=归档，默认列表隐藏）
ALTER TABLE conversations ADD COLUMN archived INTEGER NOT NULL DEFAULT 0;

-- 技能在仓库内的子目录（NULL/空=仓库根；用于同一仓库导入多个子技能，如 anthropics/skills 的 skills/docx）
ALTER TABLE skills ADD COLUMN subdir TEXT;
