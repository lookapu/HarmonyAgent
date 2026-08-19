-- Migration 050: 任务账本（Ledger 协议）
-- 任务执行状态外部化：conversations 新增 ledger 列（JSON，TaskLedger 结构），
-- 任务未完成/中断（超时/停止/护栏收尾）时落库当前执行轨迹，断点续跑时加载合并
-- （编号 append-only 续接），任务确认完成后清空。中断状态不静默丢失。

ALTER TABLE conversations ADD COLUMN ledger TEXT;
