# 工具执行契约

工具执行契约是调度、审批、重试、崩溃恢复和工具帮助页共同使用的单一真源。每个已注册工具以及未知 MCP 工具都会得到完整契约，不再依赖调用点里的零散判断。

| 字段 | 取值与语义 |
|---|---|
| `effect` | `read`、`write`、`destructive` |
| `idempotency` | `natural`（天然幂等）、`keyed`（由调用键防重）、`none`（不可自动重放） |
| `timeout_ms` | 外层 Tool Worker 硬超时；构建/部署等长任务使用扩大后的契约值 |
| `cancellation` | 当前统一为 `cooperative`：发出停止信号、终止可控进程树，并对未退出线程标记 stuck |
| `retry_safe` | 仅明确白名单中的幂等查询为 `true` |
| `approval` | `none`、`project_trust`、`always`，与 L0/L1/L2 权限等级一致 |
| `recovery` | `replay`、`verify`、`manual` |

安全默认值：

- 未知或 MCP 工具按 `write + keyed + verify + always` 处理，禁止依据名字伪装成只读工具。
- Git 提交/推送、部署、删除、任意命令等不可安全重放操作使用 `destructive + none + manual`。
- 只有工具描述明确声明“副作用：无”时，注册工具才可能成为只读契约。
- `run_command` 和 `sandbox_exec` 可请求更长超时，但不能突破 15 分钟外层保险丝。

契约通过 `tool_help` 展示给模型，并随 `export_tools_meta` 以结构化 JSON 导出。`agent::tools::contracts::tests::every_registered_tool_has_complete_execution_metadata` 对注册表逐项检查字段完整性和有效超时。
