-- Migration 087: 快照锚点记录消息 id
--
-- 时间旅行按 rowid 定位锚点；messages 表没有 AUTOINCREMENT，删除末端消息后
-- SQLite 会复用最大 rowid ⇒ 旧快照可能落到一条完全不同的新消息上。
-- 新快照同时写入 anchor_message_id，恢复与分支检查点优先按 id 定位；
-- 旧数据的该列为 NULL，继续按 rowid 兼容（删除消息时已同步清理失效快照）。
ALTER TABLE conversation_snapshots ADD COLUMN anchor_message_id TEXT;
