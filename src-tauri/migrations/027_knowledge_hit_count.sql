-- 027: 知识条目命中次数统计（每次被错误匹配命中 +1，用于排序与展示"最常用经验"）
ALTER TABLE knowledge_entries ADD COLUMN hit_count INTEGER NOT NULL DEFAULT 0;
