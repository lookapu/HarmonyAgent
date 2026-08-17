-- Migration 045: 模型手动排序（默认模型强制置顶，其余按 sort_order 排列；存量数据默认 0，即按 created_at 相对顺序）
ALTER TABLE models ADD COLUMN sort_order INTEGER NOT NULL DEFAULT 0;
