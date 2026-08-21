# ToolResultV2 协议

`ToolResultV2` 是所有内置工具、MCP 工具和子 Agent 工具写入执行账本时使用的统一结果信封。自然语言输出只作为有界诊断附件，不能单独证明任务完成。

## 稳定字段

| 字段 | 含义 |
|---|---|
| `schema_version` | 当前为 `2`；兼容新增字段不提升主版本 |
| `tool` / `status` / `summary` | 工具身份、规范化终态和有界摘要 |
| `modifications` | 已完成修改的目标、操作和副作用等级 |
| `artifacts` | 文件、配置、日志和二进制产物引用 |
| `verification` | 测试、构建、部署、diff 或读取核验及其结论 |
| `recovery` | 重放策略、重试安全性、补偿动作和恢复指引 |
| `suggestions` | 基于工具与错误类型生成的可执行下一步，最多 8 条 |
| `error` | 稳定错误码、类别、可重试性和安全摘要 |
| `metrics` | 耗时、原始输出字符数和产物数 |

为兼容已有消费者，`effect_kind`、`recovery_policy`、`retry_safe`、`side_effects`、`outcome` 和 `compensation` 暂时保留。新消费者应优先读取 `modifications`、`recovery` 和 `status`。

## 状态语义

- `succeeded`：调用成功；若工具是验证器，验证也成功。
- `partial_success`：仅部分动作完成，必须读取 `modifications` 和 `error`。
- `verification_failed`：工具运行到了验证阶段，但验收证据失败。
- `waiting_approval`：副作用尚未执行，等待用户审批。
- `retryable_failure`：瞬态错误且工具契约允许安全重试。
- `permanent_failure`：不可自动重试，需要修复输入、环境或人工处理。
- `cancelled`：调用被取消，不能据此声明目标完成。

## 兼容规则

1. 读取器必须忽略并保留未知字段；Rust 实现通过扁平 `extensions` 往返保存。
2. 新增可选字段使用默认值，能够读取早期缺字段的 V2 记录。
3. 删除字段、改变既有字段含义或类型时才提升主版本，并提供数据库迁移。
4. 证据摘要不包含执行耗时，保证相同结果跨机器仍可稳定去重。
5. 原始输出在进入信封前统一走审计脱敏和长度限制。

## 长输出外部化

超过 20,000 字符的成功或失败输出会完整写入项目内 `.deveco-agent/spill/`。模型上下文只接收 3,000 字符头部、2,000 字符尾部和 `read_file` 引用；尾部用于保留测试失败汇总、退出码等最终结论。产物路径进入 `artifacts`，因此验收、恢复和 UI 不需要从自然语言猜测。写入时清理 7 天前的文件，并在数量过多时收敛到最近 50 份。

## 回归证据

`agent::structured_result::tests` 覆盖：

- 201 个注册工具均产生完整稳定字段；
- 修改、产物、验证、错误、补偿和恢复数据均可机器读取；
- 部分成功、等待审批、可重试失败和永久失败可区分；
- 旧 V2 数据可读取，未知未来字段可无损往返；
- 相同证据不会因执行耗时不同而改变 digest。
