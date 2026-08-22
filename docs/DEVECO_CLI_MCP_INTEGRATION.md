# 官方 DevEco CLI 的 MCP 接入规划

> 状态：已落地（2026-08-22）
> 关联：`mcpTemplates.ts` 内置模板、`utils/process.rs` npm 全局 bin 解析、`mcp_policy.rs` 子进程 PATH 增强
> 背景决策见上一轮架构讨论：**以 MCP 为主接入官方 DevEco CLI，不重构底层、不做代码级融合**。

## 1. 背景与目标

DevEco CLI（`@deveco/deveco-cli`，`devecocli` 命令）是华为官方面向 AI Agent 的"原子化能力调度枢纽"：把 DevEco Studio 的 hvigor / ohpm / hdc / emulator / hilog 工具链、HarmonyOS 官方知识库和 70+ 精品 Skills 封装为 AI 可调用的标准化接口。它内置 MCP 服务器（`devecocli serve mcp`），对外提供基于 LSP 的 ArkTS / C++ 实时语法检查（Check MCP）等结构化工具。

本规划的目标：

1. **以 MCP 方式接入官方 DevEco CLI**，让 Agent 获得官方维护的鸿蒙原子能力与知识检索；
2. **克隆仓库即可用**：内置 MCP 模板 + 后端命令解析增强，用户装好 `devecocli` 后在 MCP 页面一键创建并授权；
3. 明确与现有 201 个自研工具的分工，避免重复建设。

## 2. 现状盘点

### 2.1 官方 DevEco CLI 能力（截至 1.3.0）

| 命令 | 能力 | 形态 |
| --- | --- | --- |
| `serve mcp` | 内置 MCP 服务器（stdio）：基于 LSP 的 ArkTS / C++ 语法检查，编译前错误拦截 | **MCP 工具** |
| `docs` | 本地 HarmonyOS 文档检索（`search` / `read` / `catalog`），标题命中优先排序 | CLI 命令 |
| `create` | 基于官方模板创建工程（`--app-name` / `--bundle-name` / `--api-level` 等） | CLI 命令 |
| `build` | 驱动 hvigor 构建、HAP 打包、多目标产物与签名 | CLI 命令 |
| `run` | 构建后安装并启动应用（`--module` / `--device` / `--product` 等） | CLI 命令 |
| `device` / `emulator` | 设备与模拟器生命周期管理（`list` / `view` / `start` / `create` / `delete`） | CLI 命令 |
| `log` | hilog 日志查看（`--level` / `--bundle-name` / `--follow` 实时跟随） | CLI 命令 |
| `skills` | 技能市场管理（`list` / `find` / `add` / `remove`） | CLI 命令 |
| `init` | 把 deveco-cli Skill 或 MCP 服务配置进智能体（`--skill` 与 `--mcp` 互斥，支持工程级与用户级 MCP） | 集成引导 |

关键事实：官方 MCP 服务器（`serve mcp`）当前主要暴露**语法检查类工具**（Check MCP）；`create/build/run/device/docs/log` 等完整命令通过 CLI 命令形态供 Agent 以 Skill / 直接调用方式使用。结构化 JSON 输出是官方对 AI 调用的默认设计。

### 2.2 本项目现有基础设施

- **MCP 生态**：服务器 CRUD、导入导出、测试连接、长驻 stdio 客户端、工具发现与调用转发，以及按项目授权（`allowed_tools` / `allowed_roots` / `network_policy` / `credential_keys`，失败关闭，见 [MCP 项目授权与作用域](MCP_PROJECT_AUTHORIZATION.md)）；
- **模板机制**：`src/data/mcpTemplates.ts` 内置常用 MCP 服务器模板，MCP 页面一键填充（已有社区版 `deveco-mcp`，缺官方 CLI 模板）；
- **命令解析**：`utils/process.rs` 的 `resolve_program` 已覆盖内置 Node/JDK/Git、ohpm 直调、系统 npx、鸿蒙工具链额外 PATH、常见安装目录（nvm/brew/sdkman）——但尚未覆盖 **npm 全局 bin**；
- **基座 CLI 先例**：`commands/version.rs` 已把 `@deveco-test/deveco-code`（deveco 命令）作为基座管理安装/升级，用户级 npm 全局目录探测逻辑现成；
- **鸿蒙自研工具**：201 个工具中 `build_tools.rs` / `device_tools.rs` / `debug_tools.rs` / `lsp_client.rs` 已覆盖 hvigor、ohpm、hdc、hilog、ArkTS LSP 的大部分场景。

## 3. 方案选型：MCP 而非 Skill

| 维度 | MCP（选定） | Skill |
| --- | --- | --- |
| 工具形态 | 结构化工具（name/schema/返回），参数校验、错误结构化 | SKILL.md 文本指令，模型自己拼命令 |
| 安全边界 | 走项目级授权 + 审批 + 审计流水线（`mcp__` 前缀工具） | 无独立执行体，靠提示词约束 |
| 官方增量 | Check MCP（LSP 实时语法检查）只有 MCP 形态 | 拿不到 Check MCP |
| 维护成本 | 官方维护协议，服务器升级自动跟随 | 内容漂移需自行跟踪 |
| 适配本项目 | 现有 MCP 基础设施完整，接入成本最低 | 需要转译，且与"结构化执行"理念不符 |

结论：**MCP 为主**。官方 70+ 精品 Skills 不作为 Skill 原样导入，而是后续筛选转译为能力包/提示词资产（见 §9 落地清单 `DC-06`）。

## 4. 接入架构

```text
┌────────────────────────────────────────────────────────────┐
│ React MCP 页面                                              │
│ 模板「DevEco CLI（华为官方）」一键创建 → 测试 → 项目级授权    │
└──────────────────────────┬─────────────────────────────────┘
                           │ invoke
┌──────────────────────────▼─────────────────────────────────┐
│ Rust MCP 客户端（mcp_client.rs）                            │
│ process::command("devecocli", ["serve", "mcp"])            │
│  ├─ resolve_program：PATH 未命中 → npm 全局 bin shim 直调   │  ← 本次增强
│  └─ configure_child_environment：PATH 前插内置 node bin     │  ← 本次增强
│     + 追加 npm 全局 bin；DEVECO_HOME 经 credential_keys 放行 │
└──────────────────────────┬─────────────────────────────────┘
                           │ stdio（JSON-RPC）
┌──────────────────────────▼─────────────────────────────────┐
│ devecocli serve mcp（Node 22 + DevEco Studio 6.1+）         │
│ Check MCP：ArkTS / C++ 语法检查（LSP）、官方知识检索等      │
└────────────────────────────────────────────────────────────┘
```

### 4.1 关键实现：GUI 启动 PATH 受限的解决

桌面应用 GUI 启动（LaunchServices）PATH 极简（`/usr/bin:/bin:/usr/sbin:/sbin`），`devecocli` 是 npm 全局安装的 JS shim（`#!/usr/bin/env node`），直接 `Command::new("devecocli")` 会找不到程序；即使找到，shim 内部 `env node` 也依赖 PATH。因此需要两层增强：

1. **命令解析**（`utils/process.rs`）：`resolve_program` 在 PATH 未命中后，追加探测用户级 npm 全局 bin（Windows `%APPDATA%\npm`；macOS/Linux `~/.npm-global/bin`），命中的 shim 用**内置 Node 直调**（`node <shim> serve mcp`），绕开 shebang 对 PATH 的依赖——与现有 `resolve_system_npx` 同一机制；
2. **子进程 PATH**（`mcp_policy.rs` 的 `configure_child_environment` → `apply_mcp_child_env`）：PATH 前插内置 Node bin（shim 内部 spawn node 可命中）、追加 npm 全局 bin（`devecocli` 等全局 CLI 在子进程内可解析）。

这两层对所有 MCP 服务器与所有走 `process::command` 的命令生效，不只服务 deveco-cli；npm 全局 bin 是用户自己安装的工具目录，加入解析面不扩大授权面（MCP 服务器本身是用户授权的可执行代码）。

### 4.2 DEVECO_HOME 的传递

deveco-cli 依赖 DevEco Studio（`DEVECO_HOME` 或自动探测）。按 [MCP 项目授权与作用域](MCP_PROJECT_AUTHORIZATION.md)，MCP 子进程不继承桌面完整环境，**服务器 env 中只有列入 `credential_keys` 白名单的变量才会传入子进程**。因此：

- 模板 env 提供 `DEVECO_HOME` 占位（macOS 默认 `/Applications/DevEco-Studio.app`，Windows 默认 `C:\Program Files\Huawei\DevEco Studio`）；
- 授权时**必须把 `DEVECO_HOME` 加入 `credential_keys`**，否则子进程拿不到；
- 用户本机只有默认路径时 `DEVECO_HOME` 可留空，deveco-cli 会自动探测。

## 5. 内置模板（克隆即用）

`src/data/mcpTemplates.ts` 新增 `deveco-cli` 模板：

| 字段 | 值 |
| --- | --- |
| key | `deveco-cli` |
| name | DevEco CLI（华为官方） |
| command | `devecocli serve mcp` |
| env | `DEVECO_HOME`（可选，占位提示默认路径） |
| 前置条件 | `npm install -g @deveco/deveco-cli@stable`、Node 22+、DevEco Studio 6.1+ |
| homepage | https://gitcode.com/openharmony-sig/deveco-cli |

使用路径：MCP 页面 → 点模板 → 创建（未安装时按提示先 `npm i -g`）→ 测试连接 → 克隆到项目 → 项目级授权（`allowed_tools` 全选、`network_policy=deny`、`credential_keys` 含 `DEVECO_HOME`、`allowed_roots` 为项目根）。

## 6. 与现有自研工具的分工

| 能力域 | 自研工具（现状） | 官方 MCP / CLI | 分工建议 |
| --- | --- | --- | --- |
| ArkTS/C++ 语法检查 | `lsp_diagnostics`（ArkTS LSP，按文件增量下发） | Check MCP（ArkTS + **C++**，编译前拦截） | 并存：官方 C++ 检查补齐自研缺口；ArkTS 侧按实测质量择优 |
| 官方文档/API 检索 | `search_harmony_docs` / `search_sdk_api` / SDK API 索引 | `docs search/read/catalog`（本地官方知识库） | 并存互补：自研索引离线可查，官方知识库权威更新快 |
| 构建 / 依赖 | `build_project` / `ohpm_install`（hvigor/ohpm 直调） | `build`（hvigor 封装） | 自研优先（证据链完整、已接入验收）；官方作对照 |
| 设备 / 部署 | `deploy` / `list_devices` / `connect_device` / `start_emulator` 等 | `run` / `device` / `emulator` | 自研优先；官方作对照 |
| 日志 | `read_runtime_logs` / `search_hilog` / 崩溃归因 | `log`（hilog 实时跟随） | 自研优先；官方作对照 |
| 工程脚手架 | `create_harmony_project`（自研模板） | `create`（官方模板，API 从 SDK 动态获取） | 官方优先：API Level 校验与模板随官方更新 |
| 技能市场 | Skill 管理（GitHub 导入） | `skills`（官方技能市场） | 评估接入官方市场作为新 Skill 来源（`DC-06`） |

原则：**自研工具已经具备完整证据链、审批与验收集成的场景保持自研优先**；官方能力只补缺口（C++ 检查、权威知识库、官方脚手架）或作对照基准。避免同一任务域出现两套并行执行路径导致模型选择漂移。

## 7. 落地清单（已完成项标注 ✅）

- [x] `DC-01` 内置 MCP 模板：`mcpTemplates.ts` 新增 `deveco-cli`（官方）模板，含 `DEVECO_HOME` 占位与安装指引；
- [x] `DC-02` 命令解析增强：`utils/process.rs` 的 `resolve_program` 追加 npm 全局 bin 探测，shim 用内置 Node 直调；
- [x] `DC-03` MCP 子进程 PATH 增强：`apply_mcp_child_env` 前插内置 Node bin、追加 npm 全局 bin；
- [x] `DC-04` 本规划文档与 README 入口；
- [ ] `DC-05` 环境页展示 `devecocli` 安装状态与版本（复用 `version.rs` 基座 CLI 探测模式，新增 `@deveco/deveco-cli` 探测）；
- [ ] `DC-06` 官方精品 Skills 评估转译：筛选多设备适配 / ArkTS 语法 / 应用质量等核心场景，转译为能力包或提示词资产，不原样导入 Skill；
- [ ] `DC-07` Check MCP 与 `lsp_diagnostics` 的实测对比：在固定评测集（见 [固定评测集与鸿蒙指纹识别](FIXED_EVALUATION_SUITE.md)）增加语法检查场景对照，按命中率/耗时择优并记录；
- [ ] `DC-08` 官方 `docs` 知识库接入评估：`devecocli docs` 为本地检索，评估其索引格式能否并入现有 API 知识库更新链路；
- [ ] `DC-09` 一键初始化引导：MCP 页面模板创建时检测 `devecocli` 缺失并给出安装命令（或跳转环境页）。

## 8. 验收标准

1. 克隆仓库、安装 `@deveco/deveco-cli@stable` 后，MCP 页面一键创建官方模板并通过"测试连接"；
2. GUI 启动（非终端）环境下，Agent 能调用 Check MCP 工具完成 ArkTS 语法检查并拿到结构化结果；
3. 授权后 `mcp__deveco-cli__*` 工具进入 Agent 工具面，调用走审批/审计/脱敏流水线；
4. `DEVECO_HOME` 未列入 `credential_keys` 时子进程拿不到该变量（失败关闭不变式保持）；
5. `python3 scripts/check-docs.py` 通过（新文档链接有效），`cargo test` / `npm test` 无回归。

## 9. 风险与边界

1. **官方 MCP 服务器成熟度**：`serve mcp` 工具面随版本演进，`allowed_tools` 白名单按实际 `tools/list` 交集收敛，不硬编码工具名全集；
2. **版本跟随**：deveco-cli 与 DevEco Studio 版本强绑定，Studio 升级后需重新验证 Check MCP 行为；模板 description 注明最低版本 6.1+；
3. **不扩大授权面**：npm 全局 bin 进入解析面只影响"用户自己安装的命令能否被找到"，不改变 MCP 项目的工具/目录/网络/凭据白名单语义；
4. **两套执行路径的选择漂移**：通过 §6 分工与能力包（`compile_fix` 等）保持工具面稳定，避免同一场景同时暴露自研与官方工具导致模型乱选；
5. **网络策略**：deveco-cli MCP 为本地服务，授权建议 `network_policy=deny`（默认），`docs` 检索走本地索引不受影响。
