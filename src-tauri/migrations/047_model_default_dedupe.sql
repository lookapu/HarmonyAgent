-- Migration 047: 默认模型自愈 —— 每个 Provider 有且仅有一个默认模型
-- 原因：历史数据/同步流程可能产生「多个默认」或「无默认」，导致排序时默认模型不在最上面。
-- 规则：排序最靠前（sort_order → created_at → id）的模型为默认。
-- 1) 清掉多余的默认标记（保留每个 Provider 排序最靠前的那个默认）
UPDATE models SET is_default = 0
WHERE is_default = 1 AND id NOT IN (
    SELECT id FROM models m1
    WHERE NOT EXISTS (
        SELECT 1 FROM models m2
        WHERE m2.provider_id = m1.provider_id AND m2.is_default = 1
          AND (m2.sort_order, m2.created_at, m2.id) < (m1.sort_order, m1.created_at, m1.id)
    )
);
-- 2) 无默认的 Provider，自动指定排序最靠前的模型为默认
UPDATE models SET is_default = 1
WHERE id IN (
    SELECT m1.id FROM models m1
    WHERE m1.provider_id NOT IN (SELECT DISTINCT provider_id FROM models WHERE is_default = 1)
      AND NOT EXISTS (
          SELECT 1 FROM models m2
          WHERE m2.provider_id = m1.provider_id
            AND (m2.sort_order, m2.created_at, m2.id) < (m1.sort_order, m1.created_at, m1.id)
      )
);
