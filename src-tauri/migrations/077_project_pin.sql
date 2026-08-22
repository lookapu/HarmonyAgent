-- 项目置顶：列表排序优先（0=否 1=是）
ALTER TABLE projects ADD COLUMN pinned INTEGER NOT NULL DEFAULT 0;
