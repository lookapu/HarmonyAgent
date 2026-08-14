-- Migration 019: 清理遗留的 endpoint_health 幽灵表。
-- 该表由 001 创建，熔断/健康状态实际全部在内存中维护，无任何写入源；
-- 早期的 010_remove_endpoint_health.sql 未被注册进迁移数组，导致表在所有库中残留。
DROP TABLE IF EXISTS endpoint_health;
