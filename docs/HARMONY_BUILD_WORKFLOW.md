# HarmonyOS 可恢复构建工作流

`build_project` 是 HarmonyAgent 的统一 HarmonyOS 构建入口。它按固定顺序执行：

1. `environment`：确认工程身份，解析统一语义模型，并解析可执行的 Hvigor 启动器与 SDK 环境；
2. `dependencies`：从语义模型读取所有模块的外部 OHPM 声明，核对模块级或根级 `oh_modules`，按策略安装并写后读验证；
3. `build`：在 Workspace 并发门禁内执行可选 clean 与 Hvigor 构建，流式保存完整日志；
4. `artifacts`：递归发现 HAP/HSP/HAR。Hvigor 返回成功但没有产物时，工作流仍判为失败。

## 参数与依赖策略

既有 `mode`、`module` 和 `clean` 参数保持兼容，新增 `dependencies`：

- `auto`（默认）：仅在声明的外部依赖未出现在模块级或根级 `oh_modules` 时运行 `ohpm install`；
- `force`：无论当前安装状态如何都同步依赖；
- `skip`：明确跳过安装，适合离线或由外部流程管理依赖的场景；若发现缺失依赖会在日志中预警。

工作区内 `file:`/`link:` 依赖和已解析到本地模块的依赖不要求出现在 `oh_modules`，不会触发错误安装。

## Checkpoint 与恢复

工作流将脱敏后的状态写入 `.deveco-agent/harmony-build-workflow.json`，记录 schema、构建参数键、工程指纹、完成阶段、当前阶段、错误摘要和产物证据。

- 工程指纹覆盖 ArkTS/TS、配置、资源描述和 Native 源码，跳过依赖、缓存、构建产物和 Agent 自身状态；
- 只有参数键和工程指纹均一致，且上一次状态为 `running` 或 `failed`，才进入恢复模式；
- 工程源码、配置或构建参数变化后自动创建新流程，不复用旧结论；
- 环境始终重新确认，依赖阶段使用文件系统证据跳过已完成安装，构建阶段重新执行，避免把中断时的半成品当作成功；
- `completed` checkpoint 仅用于审计，不会让用户下一次主动构建被短路。

Checkpoint 写入失败不会遮蔽真实构建结果；构建失败仍由结构化错误与完整日志负责诊断。

## 验收

自动化测试覆盖参数策略校验、相同指纹失败恢复、源码变化拒绝恢复、外部依赖缺失/安装证据以及 HAP 产物发现。全仓 Rust、崩溃 E2E、前端测试、lint 和生产构建作为阶段门禁。
