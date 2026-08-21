# 固定评测集与鸿蒙指纹识别

本文定义 EC-13 的版本化固定评测集。实现入口为 `src-tauri/src/agent/evals.rs`，场景清单位于：

- `src-tauri/tests/fixtures/agent_reliability_scenarios.json`：16 个执行内核可靠性场景；
- `src-tauri/tests/fixtures/harmony_task_scenarios.json`：10 个鸿蒙任务与识别场景。

评测套件名为 `agent_harmony_fixed_v3`。本地命令、桌面端可靠性控制面和团队共享评测集都只能执行仓库中已注册的场景；外部共享包不能注入可执行评测代码。

## 1. 鸿蒙指纹不是关键词开关

`services::harmony_fingerprint` 对工程、代码片段和日志生成 schema v1 的结构化报告：

- `classification`：完整工程、模块、源码、日志或未知；
- `confidence`：多类独立信号的有界合成分数；
- `evidence`：稳定信号码、类别、相对来源和权重；
- `api_style`：观察到 `@kit.*`、`@ohos.*`、混用或未知；
- `recommended_capability_pack`：与真实能力包 ID 对齐的建议；
- `conflicts`：损坏清单等相互矛盾的证据。

识别信号包括 `AppScope/app.json5`、根/模块 `build-profile.json5`、`module.json5`、Hvigor、`.ets`、ArkUI 装饰器与 DSL、`@kit.*` / `@ohos.*` 导入，以及 ArkTS/Hvigor/faultlog 错误签名。目录扫描有深度、文件数和单文件大小上限，跳过依赖、缓存、构建目录与目录符号链接；证据不保存源码正文或绝对路径。

该报告已经用于：

1. `get_project_info` 返回可解释的工程识别证据；
2. 能力包选择从 ArkTS 片段、编译日志和 faultlog 中补充 `project_understanding`、`compile_fix` 或 `device_diagnostics`；
3. 固定评测验证正确识别、能力包路由与普通 TypeScript 的负例。

识别结论不授予文件、设备或网络权限，也不能替代统一语义模型、SDK 对齐和实时设备读取。尤其不能只凭 `@kit.*` 猜出精确 API Level；报告只记录导入风格，版本结论必须绑定当前 product 和本机 SDK 定义。代码片段也不能冒充完整工程，模块目录不能冒充项目根。

## 2. 固定鸿蒙任务场景

| 领域 | 场景 | 穿过的生产内核 | 必须证明的结果 |
|---|---|---|---|
| 识别 | 完整工程、ArkTS 片段、ArkTS 日志、普通 TS 负例 | 指纹识别、能力包选择 | 分类有证据且不误报普通 TypeScript |
| 新建工程 | 标准 Stage 工程 | `create_harmony_project` 同步内核、统一语义模型 | bundle、default product、entry、EntryAbility 与测试骨架完整 |
| 编译修复 | API 14 符号用于 compatible API 12 | 构建错误解析、专项诊断 | 归因为 API 不兼容并携带 product 版本证据 |
| 跨模块修改 | entry 依赖 feature/har | 依赖图、反向影响分析 | 直接模块和上游验证范围都被保留 |
| 真机诊断 | 录制的 SIGSEGV faultlog | 运行时崩溃分析 | 归因为 native crash，不盲猜 ArkTS 修复 |
| 混合工程 | React + 嵌套 HarmonyOS | Workspace 扫描 | 两类模块并存，鸿蒙子工程不丢失 |
| 长会话恢复 | 大窗口预算 + 部署超时 | Context V2 预算、工具恢复契约 | 输入有界，设备副作用不自动重放并等待人工确认 |

新建工程场景直接调用生产工具的同步内核，避免维护一套只为评测通过的模板。其余场景同样调用生产解析、诊断、影响分析、Workspace、Context V2 和工具契约代码。

## 3. 通过条件与边界

- 默认阈值仍为 95%，但仓库单元测试要求全部注册场景达到 100%，任何未实现场景返回 `unhandled` 并失败。
- 每次运行记录 suite、平台、总数、通过数、分数、阈值、逐场景 expected/actual 和时间，并按 [评测运行快照](EVALUATION_RUN_SNAPSHOTS.md) 保存可复核环境。
- 评测 fixture 是版本化输入；变更期望必须和生产策略、文档及测试一同评审。
- 真机场景使用固定 faultlog，保证 macOS/Windows CI 可重复。它验证诊断内核，不声称某台真实设备已连接或发布验收已通过。
- 模型、提示词、工具版本、SDK、设备、成本、耗时和最终证据已由 EC-14 记录；显著回退的 CI 基线比较见 [评测 CI 基线门禁](EVALUATION_CI_GATES.md)。

## 4. 本地门禁

定向门禁：

```bash
cd src-tauri
cargo test --locked agent::evals::tests::reliability_gate --lib
```

阶段提交前仍需执行全量 Rust 测试、worker 崩溃恢复 E2E、Clippy、前端测试、ESLint、生产构建和 `git diff --check`。固定评测不能替代这些门禁。
