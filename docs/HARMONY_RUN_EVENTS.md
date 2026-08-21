# HarmonyOS Run 事件

HarmonyOS 不维护独立于 Agent 的“第二套 Run”。构建、部署和运行诊断直接写入当前 `ToolCtx.run_id` 对应的持久 `run_events`，继承单调 `seq`、会话归属、Worker 租约 fencing 和现有时间线重放能力。

## 事件契约

| 事件 | 关键证据 |
| --- | --- |
| `harmony.build.planned` | 工程路径与指纹、scope、mode、确定性目标和 workflow key |
| `harmony.build.completed` | 耗时、完整日志路径、产物路径/类型/模块/产品/签名状态/SHA-256 |
| `harmony.build.failed` | 退出码、主导类别、结构化错误位置和日志证据 |
| `harmony.deploy.started` | 设备、bundle、HAP、签名状态、首次或覆盖安装；多设备时每台一条 |
| `harmony.deploy.installed` | 安装后的设备、bundle、模式和命令证据 |
| `harmony.deploy.completed` | Ability、稳定状态和运行日志监听状态 |
| `harmony.deploy.failed` | 失败阶段、类别、Hilog 证据与补偿恢复结果 |
| `harmony.runtime.anomaly` | Hilog 或 faultlog 来源、ArkTS/Native/AppFreeze 类别、摘要、位置与有界证据 |
| `harmony.deploy.batch.started` | 多设备策略、并发上限、确定性设备集合和 HAP |
| `harmony.deploy.batch.completed` | 每台设备的独立终态与有界摘要、成功/失败计数 |
| `harmony.ui_flow.completed` | 设备、步骤/断言数量、真实终态、UI 树和截图证据路径 |

每条事件由数据库事务分配单调序号，因此同一 Run 内可以按 `seq` 重建“构建 → 安装 → 启动 → 状态 → 异常”的真实先后关系。多设备部署共享 Run，但每条设备事件都带独立 `device_id`，不会把一台设备的结论套到另一台。

## 异常归一

- ArkTS：`TypeError`、`ReferenceError`、`SyntaxError`、`RangeError` 分别保留稳定类别。
- Native：`Native Crash`、`SIGSEGV`、`CppCrash` 归为 `native_crash`。
- 卡死：`AppFreeze`、`ANR`、`not responding` 归为 `app_freeze`。
- 其它错误保留为 `runtime_error`，不会猜测成更具体的根因。

实时 Hilog 仍保留内存环形缓冲供 `read_runtime_logs` 快速读取；识别到异常时，摘要和有界证据同时写入持久 Run。前端 `runtime-anomaly` 事件也携带 `run_id`，可拒绝旧任务的延迟通知。

## 一致性与安全

Run 事件复用 Agent 已有写入 fencing：只有持有当前调度租约的 Worker 能追加事件。部署结束后遗留的旧 Hilog 监听即使晚到，也不能向已被接管的 Run 写入证据。事件正文只保存有界日志片段；完整构建日志和产物仍以本地 artifact 路径与摘要哈希引用。

## 验收

测试覆盖 ArkTS、Native、AppFreeze 与 ANR 分类，以及现有 Run 事件的顺序、重放和陈旧 Worker 拒写不变量。阶段门禁包含全量 Rust、两组 worker 崩溃恢复 E2E、前端测试、ESLint、生产构建与差异检查。
