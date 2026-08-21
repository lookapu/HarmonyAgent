# ArkTS 代码生成后的验证闭环

HarmonyOS `.ets` 代码发生写入后，统一执行循环不再把“文件已写入”或“某一个检查通过”当成完成。验证规划器会在最后一次成功写入之后重建证据，并要求本机定义、一致性审计、逐文件 LSP 和 Hvigor 构建共同闭环。

## 必需顺序

对仍存在的变更 `.ets` 文件，验证计划依次包含：

1. `lsp_format`：建议步骤，只消除格式噪声，不作为验收证据；
2. `check_sdk_alignment`：必需，绑定当前 product 和本机 SDK，检查 import 对应定义、API Level、权限、SystemCapability、设备与模块清单；
3. `lsp_diagnostics`：必需，对每个仍存在的变更 ETS 文件分别执行，并且输出必须明确为“无诊断错误”；
4. `run_lint`：必需，执行 ArkTS/Hvigor 静态规则检查；
5. `run_tests`：必需，运行受影响测试；
6. `build_project`：必需，Hvigor 构建成功并保留结构化日志/产物证据；
7. `git_diff`：必需，核对最终变更范围和意外修改。

`check_sdk_alignment` 只有在 SDK 状态为 `ok` 或向下兼容的 `ahead`、一致性审计为 `0 error`，且没有 `sdk_index_unavailable` 降级时才算完成。本机 SDK 缺失不能被“0 个确定错误”伪装成通过。

## 证据时序

每个步骤都记录 `completed` 和对应工具序号。只接受最后一次成功写入之后产生的证据；再次修改任何文件会让先前诊断、测试和构建证据过期，计划重新进入待执行状态。

LSP 是逐文件门禁：所有仍存在的变更 ETS 都必须单独通过。删除的 ETS 不会生成无法执行的 LSP 门禁，但删除仍需一致性审计、lint、测试、构建和 diff 验证。

## 执行循环门禁

即使用户目标中的一般验收条件已经满足，只要验证计划仍有必需步骤未完成：

- `acceptance.passed` 会被收敛为 false；
- blocker 列出仍缺少的工具；
- 循环保持在 `Verify` 阶段；
- Verify 阶段的最小工具集显式优先提供 `check_sdk_alignment` 和 `lsp_diagnostics`；
- Agent 不得宣称任务完成或进入交付。

## 边界

- LSP 返回诊断列表虽然是一次成功查询，但不等于代码通过；只有明确无诊断错误才完成门禁；
- 格式化、lint、测试和构建互不替代；
- 一致性 warning 允许开发者评估后继续，但确定性 error 阻断；
- 该闭环不自动提高 API Level、不自动添加权限，也不依赖 DevEco Studio 私有状态。

## 验收

回归测试覆盖完整 ArkTS 闭环、带错误的 LSP 结果不通过、仅构建成功仍停留 Verify、最后写入之前的旧证据失效，以及删除 ETS 不产生不可达的逐文件 LSP 门禁。全量门禁还包括 Worker 恢复、前端测试、lint、生产构建和 diff 检查。
