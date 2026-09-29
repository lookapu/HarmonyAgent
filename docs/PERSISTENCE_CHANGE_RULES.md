# 持久化变更判定表

> 来源：DeepSeek Harness `docs/persistence-changes/README.md`（2026-09 引入），按本项目 `migrations/` 现状裁剪。
> 目的：把「改一个持久化字段要不要写 migration、旧记录还能不能读」从口头约定变成判定。

## 适用范围

`src-tauri/migrations/` 下所有 SQL、`db/mod.rs` 里的 schema 注册、以及任何写进
`messages` / `session_events` / `tool_runs` / `agent_runs` / `run_events` /
`execution_steps` / `conversations` 的**结构**变更（新增/改类型/改名/删列/改非空约束）。

**不适用**：纯数据修正（UPDATE 已有行的值）、索引/性能调整、不改结构的查询改动。

## 判定表

改动 SQLite 已落盘的表结构时，按下表取**最低判定**：

| 检测到的变更 | 最低判定 |
|---|---|
| 新增可空列（`ALTER TABLE ... ADD COLUMN ... NULL`） | `same-version` |
| 新增索引 / 新增触发器 | `same-version` |
| 新增一张表，且旧代码路径不读它 | `same-version` |
| 新增 NOT NULL 列且带 DEFAULT | `same-version`（旧行由 DEFAULT 兜底） |
| 新增 NOT NULL 列且无 DEFAULT | `version-bump`（必须先回填再改约束，走两段迁移） |
| 列改类型 / 删列 / 改列名 / 加唯一约束 | `version-bump` |
| 改主键 / 改外键 / 改 CHECK 约束到更严 | `version-bump` |

- `same-version`：旧记录照常可读，**不写 migration**，只在代码里兼容。
- `version-bump`：必须新增一个 `migrations/NNN_*.sql` 并在 `db/mod.rs` 注册。

## 两条硬规则

1. **规则作用于完整变更。** 一次提交里同时做了「新增可空列」和「删一列」，
   最低判定取最严的 `version-bump` —— 允许的那部分不能掩盖同一次提交里的破坏性部分。

2. **同一次提交的破坏性部分不得被兼容部分掩盖。** 判定的对象是这次改动的
   *整体*，不是每一行 diff 的善意叠加。review 时按整体看。

## SQLite 特别注意

本项目会话事件走 SQLite 表（`session_events` 等），不是 JSONL 多代格式。
DeepSeek Harness 自己从 `session-persistence-sqlite` 迁回了 JSONL 代际，
**本项目不动存储引擎** —— 50+ migration 已就位，只借它的判定纪律。

对应到 SQLite 的等价做法：

- 「JSONL 代际」≈「新增一个 migration 文件 + `db/mod.rs` 注册一个版本号」
- 「schema 快照摘要」≈「migration 文件本身的 DDL 就是快照」——我们不额外维护 digest 链

## Review 清单

改 `migrations/` 或持久化表结构时，逐条确认：

- [ ] 判定表查过了吗？最低判定是什么？
- [ ] `same-version` 的改动，老版本代码读新库会不会炸？
- [ ] `version-bump` 的改动，`db/mod.rs` 里的版本号递增了吗？
- [ ] 这次提交里有没有混入更严格的变更？
- [ ] 旧数据（用户已有 DB）能正常读吗？不能读的那部分怎么兜底？
