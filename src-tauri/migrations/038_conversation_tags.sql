-- 038_conversation_tags: 会话标签系统
-- 标签字符串（半角逗号分隔），单 session 颜色在 tags_color 存；色板用前端固定 8 色。
-- 设计取舍：标签字段做在 conversation 行内（不另起表），理由是
--   1) 标签数量少（人手加的不会超过 5-10 个），JSON 解析开销可忽略
--   2) 一行读出即可渲染，避免 JOIN
--   3) 列表/筛选按 tags LIKE '%xxx%' 索引，>1k 会话仍秒级
-- 多标签去重/排序在写入时做（前端 set 后传 string，Rust 端 normalize）。

-- 会话标签：逗号分隔字符串（避免新建关联表）；空串 = 无标签
ALTER TABLE conversations ADD COLUMN tags TEXT NOT NULL DEFAULT '';
