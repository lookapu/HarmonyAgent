# DevEco Switch · CHANGELOG

> 面向 HarmonyOS / OpenHarmony 开发者的桌面 AI 编程 IDE。
> 本文件按版本倒序记录用户可见的变更、迁移要点与回滚指引。

---

## v2.0 — Agent Workspace 收尾（2026-08-16）

定位：从"Provider 切换器"升级为**完整 Agent Workspace**——工具集 117 → **191**，覆盖鸿蒙开发全链路；新增 9 个能力工具 + ToolError 结构化错误；命令面板与 i18n 同步落地；超大单文件按职责拆分。

### ✨ 新增（9 个 A 类工具 + 1 个错误体系升级）

| ID   | 工具 | 能力 |
|------|------|------|
| [14] | `log_query`         | hilog / runtime_log / faultlog 三源结构化查询（since / level / keyword / regex / 设备过滤），输出按时间聚合 + 命中段截断 |
| [23] | `docx_read`         | `.docx` 正文（纯标准库 `zip` + XML 流式解析，零依赖） |
| [26] | `audio_transcribe`  | 调本地 `whisper.cpp` 转写（自动定位 whisper 二进制 + ggml 模型） |
| [28] | `attach_debugger`   | `hdc shell debuggerd -p <pid>` attach + `aa debug` 回退；输出 PID / bundle / wait_secs 与下一步指引 |
| [29] | `step_debug`        | step / next / continue / interrupt / where / info 六动作调试驱动 |
| [30] | `memory_snapshot`   | take / list / diff 三动作；连续两次增长 > 10% 自动提示"疑似泄漏" |
| [36] | `ota_pack`          | 内置 `packagingtool.jar` → `.pkg` 打包（自动找 jar、可选 profile_path 注入签名） |
| [48] | `license_check`     | 扫 `oh-package.json5` / `Cargo.toml` / `pyproject.toml`，对照内置 allow/deny 黑白名单输出违规项 |
| [49] | `vuln_scan`         | 内置 10 个已知漏洞（lodash/axios/requests/spring/jackson 等），按依赖版本匹配，给出 CVE 与建议版本 |
| [65] | `ToolError`         | 7 类 category（network/permission/not_found/invalid_input/internal/timeout/conflict）+ 是否可重试 + 自动建议下一步；`run_tool` 出口自动套信封，零侵入覆盖所有 191 个工具 |

### 🛠 重构与拆分

- **工具集重组**：原 v1 的若干大工具拆分为更聚焦的变体（如 `lsp_*` 9 个、构建/部署/签名分项、调试 `attach/step/breakpoint` 独立），最终工具数 **117 → 191**。
- **TOOL_GROUP**：按 8 个域分组（`build` / `fix` / `explore` / `deploy` / `refactor` / `test` / `debug` / `other`），前端按组渲染与限额。
- **TASK_GROUPS**：与 TOOL_GROUP 对齐，限额与守卫按组生效（修复了"按工具名限额"导致热门工具被全局压制的问题）。

### 🎨 命令面板 + i18n（前端配套）

- 命令面板新增 **28 个高频工具 action**（`Cmd+K` 即时触发），覆盖调试 4 / 重构 5 / 构建 2 / 部署 1 / 安全 4 / 知识 4 / 数据 2 / 治理 5 / 多模态 3。
- 中英文 `i18n` 增加 30 条工具标签（zh + en 各 30 条），前端 fallback `t('toolToolName')` 兼容未命中。

### 🧹 代码结构清理

- `agent/tools/quality_tools.rs` 由 **2400+ 行单文件** 拆为 facade + 4 个子文件，**按方法完整切片**（不按行数切，保证每个 `fn` 跨多行签名 + 函数体完整落在同一文件）：

  | 子文件 | 工具数 | 函数数 | 内容 |
  |--------|-------:|------:|------|
  | `quality_metrics.rs`  | 7 | 15 | code_metrics / metric_export / log_aggregate / log_query / memory_snapshot / snippet_insert / replay_trace + 7 个 helper + `FileMetrics` / `SOURCE_EXTS` / `SKIP_DIRS` |
  | `quality_security.rs` | 4 |  9 | obfuscate / sandbox_exec / license_check / vuln_scan + 5 个 helper |
  | `quality_runtime.rs`  | 6 | 11 | api_test / api_mock / api_health / attach_debugger / step_debug / ota_pack + `MockRoute` struct + 4 个 helper + `hdc_shell` |
  | `quality_media.rs`    | 2 |  5 | docx_read / audio_transcribe + 3 个 helper |

  拆分原则：
  - `pub use module::*` 在 facade re-export，对外 `quality_tools::code_metrics(...)` 调用方式零变更。
  - helper 跟随"主消费者"所在文件（如 `parse_dep_line` 跟 `license_check` 走 security）。
  - `pub(super) async fn` → `pub async fn`（`pub use` 不能 re-export 私有项）。
  - `super::xxx` → `crate::agent::tools::xxx`（facade 不可见，需走绝对路径）。
  - 跨文件共享的常量（如 `SKIP_DIRS`）按"谁需要谁就近复制"，避免反向依赖；确实跨多处用的，`scanner.rs` 上加 `pub`。

- 根目录清理 **59 个调试/分析脚本**，统一归档到 `scripts/legacy/`（含 11 个 Python 处理脚本 + 48 个旧日志/测试产物）。
- `.gitignore` 增补 `scripts/legacy/` / `__pycache__/` / `*.pyc` / `*.log` 等规则，避免误提交临时文件。

### 📚 文档

- `README.md` 从 1642 字节扩到 12k+ 字节，重新定位为"Agent Workspace"，补全工具清单、能力矩阵、命令面板使用、安全治理、内置运行时说明。
- `docs/tool-enhancement-backlog.txt` 升级为 v2 完成态（56/76 兑现，3 项外联 figma/feishu/jira 按用户要求暂缓）。
- `docs/ARCHITECTURE.md` 同步更新：拆出 quality 子文件后的模块图、TOOL_GROUP × TASK_GROUPS 关系表。

### ✅ 验证

- `cargo check --lib`：**0 error / 0 warning**
- `cargo test --lib`：**346 passed / 0 failed**（其中 7 个为新 ToolError 单元测试）
- 拆分前后 `quality_tools::xxx(...)` 调用点 **0 处需要修改**（facade re-export 兼容）

### 🔄 迁移要点

- 无破坏性变更。`quality_tools` 公共 API 100% 兼容，外部 import 不需修改。
- `agent::scanner::SKIP_DIRS` 由 `const` → `pub const`（被 `quality_security` 借用），如外部代码依赖其私有性请注意。
- 命令面板默认展示顺序变化：高频工具置顶，长尾工具折叠到二级菜单。

---

## v1.0 — 初版提交（2026-08-14）

定位：HarmonyOS 桌面 AI 编程 IDE 雏形，**117 个 Agent 工具** + 多 Provider 路由 + 内置运行时。

### 基础能力

- **AI Agent 内核**：多轮对话、子 Agent 派生（`spawn_agents`）、任务计划（`plan_task`）、TodoWrite 进度跟踪、`undo_edit` 撤销栈、跨轮诊断记忆、`ask_user` 主动提问、后台任务（`run_command --background`）、运行时日志（`hdc shell hilog -L E`）。
- **鸿蒙深度集成**：hdc 设备管理 / 真机无线连接 / 模拟器启停 / hvigor 构建 / ohpm 依赖 / faultlog 崩溃归因 / hilog 实时回流 / 多模块工作区识别。
- **多 Provider 路由**：华为 / 智谱 / 通义等多家 LLM 接入 + 本地 HTTP 代理 + 熔断器 + 自动 failover + 费用追踪 + 请求日志。
- **API 知识库**：内置 HarmonyOS API 索引（向量检索 + 符号索引） + 跨版本 diff + 兼容性扫描 + 用户笔记。
- **安全治理**：工具调用白名单 / 工具限额 / 任务守卫 / 预算控制 / 权限管理 / 审批拦截流水线（pre/post hooks）。
- **内置运行时**：自带 Node + JDK + Git 运行环境（`src-tauri/runtime/`），用户机器无需预装开发环境。
- **代码理解**：分级扫描（`check_code` / `deep_scan` / `codebase_search` / `get_symbol_details`） / 符号索引 / 文件系统工具集。
- **生态能力**：MCP 服务器管理 / Skill 启停 / 鸿蒙官方文档检索 / Web 搜索与抓取 / 知识库导入导出。

### 关键模块

| 模块 | 行数（v1 末） | 说明 |
|------|-------------:|------|
| `agent/tools/mod.rs`        | ~4200 | 工具注册表（TOOL_SPECS / TOOL_GROUP / 191 个工具 dispatcher） |
| `agent/tools/fs_tools.rs`   | ~1500 | 文件读写 / 搜索 / 折叠 / gitignore |
| `agent/tools/build_tools.rs`| ~1200 | hvigor / ohpm / 签名 / 部署 / 产物分析 |
| `agent/tools/cmd_tools.rs`  | ~ 700 | run_command 危险命令黑名单 + 沙箱 + 后台任务 |
| `agent/agent_board.rs` 等   | 各 200-600 | Agent 编排、反思、记忆、会话事件、任务队列 |

### 已知遗留（v1 末 → v2 修复）

- 对话 SSE 流式响应在 chunk 边界切多字节字符 → `U+FFFD` 永久入库（v1.1 修：字节缓冲整行解码）
- `list_dir` 不遵循 `.gitignore`（v1.1 修：含子目录 + 子模块规则）
- `read_file` 注释占比高时无折叠，淹没代码（v1.1 修：连续长注释块折叠为一行摘要）
- gitignore 运行时静默失效（`canonicalize` 加 `\\?\` 前缀与 `normalize` 不一致，v1.1 修）
- 大量调试/分析脚本散落根目录（v2 修：归档到 `scripts/legacy/`）

---

## 版本对照速查

| 维度 | v1.0 | v2.0 | 增量 |
|------|-----:|-----:|-----:|
| 工具数（TOOL_SPECS） | 117 | 191 | **+74**（+63.2%） |
| 新增工具 | — | 9 + ToolError | — |
| TOOL_GROUP 域 | 3 | 8 | +5 |
| 命令面板 actions | 0 | 28 | +28 |
| i18n 工具标签 | 0 | 30 条 × 2 语言 | +60 |
| 文档（README + ARCHITECTURE + backlog） | 60+1248+0 | 12000+1500+700 | +10× |
| cargo test | 282 passed | **346 passed** | +64 |
| 编译错误/警告 | 0/0 | **0/0** | 持平 |
| 根目录调试脚本 | 59 | 0 | -59 |

---

## 维护说明

- 工具总数以 `src-tauri/src/agent/tools/mod.rs` 中 `TOOL_SPECS` 数组长度为准（当前 191）。
- 任务分组以 `TASK_GROUPS` 常量为准（当前 8 个：`build` / `fix` / `explore` / `deploy` / `refactor` / `test` / `debug` / `other`）。
- `quality_tools::*` 通过 facade 暴露，**禁止**直接 import 4 个子文件（`quality_metrics` 等）—— 内部模块，外部耦合面随 facade 走。
- 任何对工具的"按行数切分"禁止。**必须按方法完整切片**，签名 + 函数体在同一文件内。脚本辅助可见 `scripts/legacy/_split_quality.py`。
- CHANGELOG 任何变更在 commit message 里写 `docs(changelog): <一句话>`，不要直接编辑本文件然后 commit `docs:`。
