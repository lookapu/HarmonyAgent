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
| `validator` | 可选的 artifact read、diff、tests、build、deploy 或受限 command 验证器 |
| `recovery_action` | `restore_snapshot`、`git_revert`、`redeploy_previous`、`verify_then_compensate` 或 `manual_review` |

安全默认值：

- 未知或 MCP 工具按 `write + keyed + verify + always` 处理，禁止依据名字伪装成只读工具。
- Git 提交/推送、部署、删除、任意命令等不可安全重放操作使用 `destructive + none + manual`。
- 只有工具描述明确声明“副作用：无”时，注册工具才可能成为只读契约。
- `run_command` 和 `sandbox_exec` 可请求更长超时，但不能突破 15 分钟外层保险丝。
- 非只读工具必须声明恢复动作；文件编辑恢复快照，Git 提交使用补偿提交，部署回退前一产物，其余危险操作要求人工核对真实状态。
- `run_command` 只有参数明确表示 test/build/check/diff/status 时才产出验证证据，普通命令成功不能冒充验收。

契约通过 `tool_help` 展示给模型，并随 `export_tools_meta` 以结构化 JSON 导出。`agent::tools::contracts::tests::every_registered_tool_has_complete_execution_metadata` 对注册表逐项检查字段完整性和有效超时。
