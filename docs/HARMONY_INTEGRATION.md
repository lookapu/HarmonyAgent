# DevEco Switch HarmonyOS 集成说明

> 当前状态：已实现。本文描述 2026-08-21 `main` 分支中的工程解析、环境探测、构建、部署、调试和知识能力；具体行为以 Rust 实现为准。

## 1. 模块边界

| 能力 | 主要实现 |
|---|---|
| 工程最小解析 | `services/harmony.rs` |
| 环境与 SDK 探测 | `services/harmony_env.rs` |
| 官方文档 | `services/harmony_docs.rs` |
| API 索引与版本 diff | `services/harmony_api_ref.rs`、`harmony_api_diff.rs` |
| 构建错误知识 | `services/harmony_knowledge.rs` |
| 工程能力面板 | `commands/harmony_analyze.rs`、`commands/project.rs` |
| 构建/部署工具 | `agent/tools/build_tools.rs` |
| 设备工具 | `agent/tools/device_tools.rs` |
| 调试与 LSP | `agent/tools/debug_tools.rs`、`agent/lsp_client.rs` |

HarmonyOS 能力不是独立的第二套 Agent。它通过 `TOOL_SPECS` 进入统一工具执行内核，共享路径边界、权限、预算、租约、结构化结果、证据验收和恢复策略。

## 2. 工程根与项目解析

### 2.1 工程根判定

`is_project_root` 优先检查：

1. `AppScope/app.json5`；
2. 根级 `build-profile.json5`，且顶层必须包含 `app`。

仅存在 `oh-package.json5` 或模块级 `build-profile.json5` 不足以判定工程根。这样可避免把 `entry/` 或 feature 模块误识别为主工程，进而把文件写到 `entry/entry/...`。

### 2.2 最小工程信息

`parse_project` 返回构建和部署需要的 `HarmonyProject`：

- `bundle_name`、`version_code`、`version_name`、`app_label`；
- `main_element`；
- `entry_module`；
- `api_version` 和 SDK 原始版本；
- `signing_configured`；
- 推导的 HAP 输出目录。

解析采用容错策略：单个 JSON5 文件失败只使对应字段为空，不阻塞整个工程。当前 JSON5 helper 通过去注释、尾逗号容错后交给 `serde_json`，不承诺支持任意 JSON5 语法。

### 2.3 配置来源

| 文件 | 读取内容 |
|---|---|
| `AppScope/app.json5` | bundle、版本、label |
| `build-profile.json5` | products、compatible/compile SDK、签名、modules |
| `<module>/src/main/module.json5` | mainElement/ability |
| `<module>/src/main/resources/base/profile/main_pages.json` | 页面路由 |
| ArkTS 源文件 | 装饰器和符号兜底 |
| `oh-package.json5` | ohpm 依赖 |

entry 模块优先来自根 `build-profile.json5` 的 modules；无法确定时扫描一层子目录中的 `src/main/module.json5`。项目能力面板另有更完整的多模块扫描，不应把 `HarmonyProject` 当作完整 ProjectIndex schema。

## 3. 环境探测

`harmony_env.rs` 同时支持自动发现和用户手工配置。探测目标包括：

- DevEco Studio；
- HarmonyOS/OpenHarmony SDK variants 与 API 组件；
- command-line-tools；
- `hdc`、`ohpm`、`hvigor`/wrapper；
- SDK API 声明目录。

应用启动时会把找到的 HarmonyOS 可执行目录注入子进程 PATH。另有内置 Node、JDK 和 Git fallback；HarmonyOS SDK/hdc/ohpm 本身不随仓库捆绑，通常来自 DevEco Studio或用户安装的 command-line-tools。

探测结果有缓存，保存手工配置或显式重新探测时失效。HealthPage 和环境页展示实际候选路径与版本。

## 4. hvigor 与 ohpm

### 4.1 构建命令

`services/harmony.rs` 负责选择 wrapper 和生成基础参数：

- 工程 wrapper 优先于全局命令；
- `assembleHap` 支持 module 和 mode；
- clean 使用独立参数；
- 子进程继承探测后的 Node/JDK/HarmonyOS PATH；
- Windows 子进程使用隐藏控制台配置，避免 GUI 应用弹出命令窗口。

对外工具包括 `build_project`、`build_hap`、`build_generic`、`ohpm_install`、`run_lint`、`run_tests` 等。确切名称和参数以 `TOOL_SPECS` 为准。

### 4.2 产物

标准 HAP 目录推导为：

```text
<module>/build/default/outputs/default
```

查找时优先推导目录并按修改时间选择；找不到后再递归工程、跳过依赖/缓存目录，递归 fallback 会提高 `-signed` 产物优先级。产物路径会进入结构化工具证据，供部署和验收使用。

### 4.3 ohpm

`collect_ohpm_deps` 汇总依赖，`verify_ohpm_install` 结合退出状态、日志和 `oh_modules` 判断结果。依赖检查和安装均通过统一命令执行与输出截断/落盘策略。

## 5. 构建错误

`parse_build_errors` 将日志归一化为：

```text
kind / category / file / line / column / message / suggestion
```

category 包括：

- `type`；
- `dependency`；
- `signing`；
- `sdk`；
- `api_level`；
- `resource`；
- `ohpm`；
- `syntax`；
- `other`。

解析器处理 ArkTS/TypeScript 位置、模块/依赖、签名、SDK/API level、资源和 ohpm 常见模式。未命中规则的日志仍保留原始摘要，不因分类失败丢失构建结果。

`harmony_knowledge.rs` 把内置根因知识与用户知识条目合并，为 Agent 提供修复建议；修复后的真实构建结果仍是验收依据，知识建议本身不是完成证据。

## 6. 部署闭环

典型部署过程：

```text
解析工程
  → 选择/构建 HAP
  → list_devices / 选择默认设备
  → hdc install
  → aa start（bundleName + ability）
  → 读取 hilog/runtime logs
```

关键规则：

- 多设备时使用显式 device id 或项目默认设备；
- 安装和启动是不同副作用步骤，分别记录状态；
- HarmonyOS 启动使用 `aa start`，不是 Android `am start`；
- 部署工具受 L2/项目权限和全局互斥控制；
- 中断或 Worker 崩溃后不能直接重复安装/启动，恢复计划会先验证设备侧效果；
- 用户目标明确要求“部署”时，成功的 deploy 工具证据是目标契约必需项。

部署产物和日志可能较大，超限内容会写入 `.deveco-agent/spill/`，工具结果保留摘要和路径。

## 7. 设备与运行时调试

设备层覆盖：

- hdc 服务启停、设备列举和无线连接；
- 模拟器列举、启动和创建；
- shell 与设备文件操作；
- 应用列表、安装、启动、停止、卸载、清数据和授权；
- 截图、录屏、UI hierarchy、手势和 UI flow；
- Wi-Fi、飞行模式和网络条件；
- CPU、内存、电池和性能采样。

日志来源包括 hilog、运行时日志缓冲和 faultlog。`analyze_crash` 对 JsError、CppCrash、NativeCrash、启动超时等根因进行结构化归类；`log_query` 支持时间、级别、关键词和正则过滤。

调试工具提供 attach、step、next、continue、interrupt、where/info 等动作。具体可用性取决于设备版本、debuggable 构建和本机工具链；工具会返回明确的缺失条件，而不是假定所有设备都支持交互调试。

## 8. ArkTS LSP

`agent/lsp_client.rs` 通过 stdio JSON-RPC 启动 `@arkts/language-server`，维护 initialize、文档同步和请求响应。

对外能力包括 definition、references、symbols、hover、diagnostics，以及注册表中的 rename/format/code action/completion/signature 等扩展工具。找不到 language server 时，代码库搜索与符号索引作为降级路径。

LSP 结果受 SDK、工程配置和 language server 版本影响；“无诊断”不能单独证明项目可构建，最终仍应运行构建或测试。

## 9. API 与文档知识

项目有三类互补数据源：

1. 本机 SDK `.d.ts`：`list/search/read_sdk_api_*`；
2. 官方文档本地镜像：同步、索引、搜索和读取；
3. SQLite API 知识库：模块、详情、版本引入信息和 embedding。

API diff 服务支持查询某 API 的引入版本、跨版本变化和模块最低版本；`check_project_sdk_alignment` 对比工程 SDK 声明与本机安装版本。

新安装时，如果主库 API 表为空，应用会从打包资源中的 seed knowledge DB 后台导入。embedding 是可选 feature：未启用或模型不可用时回退关键词/BM25 检索。

## 10. ohpm 生态

`ohpm_landscape.rs` 缓存官方 landscape 数据，支持状态、刷新、搜索、热度、分类和仓库链接。缓存超过 7 天时应用启动后延迟刷新，失败不阻塞主流程。

Agent 工具 `ohpm_search` / `ohpm_recommend` 与前端生态页共享数据源。推荐结果是候选信息，安装后仍需通过 `ohpm_install` 和真实构建验证兼容性。

## 11. 安全与恢复

HarmonyOS 工具继续遵循通用执行内核：

- 工程路径必须位于受信任 workspace；
- 构建、部署、设备写操作按权限等级审批；
- build/deploy 等资源使用互斥/并发键，避免并行抢占；
- command 参数经过危险模式检查；
- 工具结果脱敏并限制大小；
- 工具调用保存 idempotency、lease、产物和验证证据；
- 读操作可安全重试，部署/安装/写设备等先验证效果；
- 旧 Worker 迟到结果由 fencing 拒绝。

## 12. 验证建议

纯代码测试可以覆盖解析器、命令生成、错误分类、路径、状态机和恢复策略，但无法替代真实环境。涉及 HarmonyOS 的变更应按风险选择：

1. 解析 fixture/真实多模块工程；
2. 运行 Rust 单测和 Clippy；
3. 在安装 DevEco Studio/SDK 的机器执行环境健康检查；
4. 运行 `ohpm install` 和 debug/release 构建；
5. 在至少一台真实设备或匹配 API 的模拟器安装并启动；
6. 读取日志，确认 bundle/ability、签名和 API level；
7. 对部署/设备写操作测试停止、超时和恢复，确认不会重复副作用。

任何文档示例都不能替代当前 `TOOL_SPECS` 参数 schema、环境探测结果和实际设备输出。
