# DevEco Switch

> **Agent Workspace for HarmonyOS Developers** — 一站式桌面 AI 编程工作台

[![Platform](https://img.shields.io/badge/platform-Windows%20%7C%20macOS%20%7C%20Linux-blue)]()
[![Tauri](https://img.shields.io/badge/Tauri-2.x-orange)]()
[![License](https://img.shields.io/badge/license-MIT-green)]()

简体中文 | [English](README.en.md)

面向 HarmonyOS / OpenHarmony 开发者的桌面 AI 编程 IDE。把多 Provider 路由、Agent 工具链、鸿蒙工程理解、设备调试、API 知识库、本地代理熔断塞进一个原生应用，让模型真正"懂"鸿蒙项目、能干活。

## 它是什么

不是简单的 Provider 切换器。**201 个 Agent 工具**覆盖鸿蒙开发的全链路——从新建工程到崩溃归因，从代码扫描到真机部署：

| 维度 | 能力 |
|------|------|
| 🤖 **AI Agent 内核** | Rust 后端多轮工具循环、子 Agent 派生、任务计划、TodoWrite、撤销栈、主动提问、失败反思与证据驱动验收 |
| 📱 **鸿蒙深度集成** | hdc 设备管理 / 真机无线连接 / 模拟器启停 / hvigor 构建 / ohpm 依赖 / faultlog 崩溃归因 / hilog 实时回流 / 多模块工作区识别 |
| 🔌 **多 Provider 路由** | 华为/智谱/通义等多家 LLM 接入 + 本地 HTTP 代理 + 熔断器 + 自动 failover + 费用追踪 + 请求日志 |
| 📚 **API 知识库** | 内置 HarmonyOS API 索引（向量检索 + 符号索引） + 跨版本 diff + 兼容性扫描 + 用户笔记（knowledge entries） |
| 🛡 **安全与可靠性** | 工具白名单 / 限额 / 预算 / 审批流水线 + 目标契约 / 持久队列 / DAG / Worker 租约 / 崩溃恢复 / SLO 与审计 |
| 📦 **内置运行时** | 便携版 Node + JDK + Git 随安装包捆绑（构建时由 CI 从官方源自动下载），用户机器无需预装开发环境 |
| 💬 **会话管理** | 多会话 / 上下文压缩（compact） / LLM 调用回放（llm_replay） / 事件溯源（session_events） / 会话标签 / 置顶 / 消息队列 / 任务看门狗（卡死自动 abort） / **会话时间旅行（快照回溯）** / **跨会话引用（@ 会话）** / **定时提醒（schedule）** |
| 🧠 **代码理解** | LSP 语义级分析（ArkTS 语言服务器） + 分级扫描（check_code / deep_scan / codebase_search / get_symbol_details） / 符号索引 / 文件系统工具集 |
| 🌐 **生态能力** | MCP 服务器管理 / Skill 启停与使用统计 / ohpm 生态面板 / 鸿蒙官方文档检索 / Web 搜索与抓取 / 知识库导入导出 |
| 📡 **LAN 访问** | 内置局域网服务，手机/平板浏览器即可查看会话、发消息、管理会话（token 鉴权 + 只读文件查看） |

## 核心特性

### 1. 真·AI Agent，不是聊天机器人

- **子 Agent 并行**：`spawn_agents` 派生子任务独立跑（最多 50 条运行记录），Agent 间通过消息板（pub/sub）协作
- **任务计划**：`plan_task` 把复杂任务拆步骤，前端实时渲染进度（todo → doing → done/failed）
- **主动提问**：`ask_user` 中断 Agent 流程等你回答（oneshot 通道，停止时自动取消）
- **撤销栈**：`undo_edit` 给 Agent 的 edit_file/write_file 加快照栈（每会话最多 40 条，FIFO 淘汰）
- **跨轮诊断**：构建/部署/崩溃的根因结论按项目缓存，system prompt 自动注入，避免模型"重复踩坑"；记忆注入带 **BM25 相关性排序 + 最近更新置顶（front_page）+ 负反馈词袋纠偏**（点踩过的内容不再反复出现）
- **失败反思**：工具调用失败后自动沉淀反思片段注入下一轮 system prompt，让 Agent 记住自己的失败模式
- **时间旅行**：每轮工具执行后自动保存会话快照（消息锚点 + 账本 + 摘要），可回到任意历史决策点重新引导（对齐 langgraph checkpoint）
- **定时提醒**：`schedule_create`（after/at/every）设定会话内提醒，到期自动注入对话 + 桌面通知
- **后台任务**：`run_command --background` 长任务立即返回 job_id，完成时摘要注入下一轮请求
- **运行时日志**：部署后自动 `hdc shell hilog -L E` 监听，异常时自动落诊断 → 前端事件

### 2. 鸿蒙工程专属能力

- **多模块识别**：`har / hsp / haps` 模块自动扫描，工作区模块树渲染
- **真机调试**：`list_devices` / `connect_device` / `manage_hdc` / `start_emulator` / `device_file` / `device_shell` / `attach_debugger` / `step_debug`
- **构建部署**：`build_project`（hvigorw assembleHap）/ `deploy` / `deploy_all` / `analyze_hap_size` / `ota_pack`（.pkg 打包）
- **崩溃归因**：`analyze_crash` 扫 faultlog，结构化 JsError / CppCrash / 启动超时 等 7 类根因
- **API 知识库**：`search_sdk_api` / `read_sdk_api_module` / `search_harmony_docs` / `diff_api_versions` / `scan_api_compat`
- **ohpm 生态面板**：`ohpm_search` / `ohpm_recommend` + 前端生态浏览器（分类 / 评分 / 下载量 / 依赖树）
- **环境探测**：`environment_check` / `get_env_info` / `check_sdk_alignment` / `get_installed_apps`

### 3. LSP 语义级代码理解

不靠"文本扫描"猜——直接以 stdio JSON-RPC 拉起 `@arkts/language-server`，与 DevEco Studio 同源的 ArkTS 分析能力：

- `lsp_definition`：跳转定义（含 SDK .d.ts 与跨模块）
- `lsp_references`：全局引用
- `lsp_symbols`：文档符号（struct / 类 / 状态变量）
- `lsp_hover`：悬停文档与 API 说明
- `lsp_diagnostics`：实时类型 / 语法诊断（按文件模型增量下发）

SDK 路径自动探测：`DEVECO_SDK_HOME` → DevEco Studio 安装路径 → 用户目录 `Huawei/Sdk`。

### 4. 多 Provider + 本地代理

- 不绑定任何 LLM 厂商
- 本地 HTTP 代理 + 熔断器（circuit_breaker） + 自动 failover
- 每日费用统计 / 请求日志 / token 用量追踪
- Provider 配置可视化编辑，导入导出方便
- 多开实例只启动一份代理（ProxyLock 持有者机制）

### 5. 安全优先

- **工具白名单**：危险工具（部署/构建/网络）按项目配置白名单
- **审批流水线**：pre/post hooks（pipeline.rs）拦截敏感操作
- **任务守卫**：task_guard 防止 Agent 跑飞
- **预算控制**：budget / cost_guard 模块控制单次/每日成本
- **工具限额**：tool_limits 按 8 个任务组（build / fix / explore / deploy / refactor / test / debug / other）限制调用次数，热门工具不再被全局压制
- **权限管理**：permissions 模块按工具类型分级

> 当前限制：`run_command` 虽有工作区路径校验、危险模式拒绝和审批，但命令进程仍以宿主用户权限运行；兼容工具名 `sandbox_exec` 只是“临时副本试运行”，不是 OS 级文件系统或网络沙箱。不要用它运行不可信仓库脚本。详见 [安全边界与威胁模型](docs/SECURITY_BOUNDARY.md)。

### 6. 证据驱动的可靠执行

- **目标契约**：从用户目标提取修改、验证、构建、测试、部署、提交和推送等必需条件；模型只能申请完成，运行内核依据真实工具证据裁决
- **Durable Run**：任务状态、阶段、事件游标、执行步骤、检查点和验收结果写入 SQLite，WebView 刷新或进程异常后仍能判断真实终态
- **持久化调度与 DAG**：任务队列支持优先级、租约、重试、恢复令牌和并发键；主任务与子 Agent 以 DAG 节点记录依赖、失败策略与验收结果
- **多 Worker 防重**：桌面进程通过心跳、租约令牌和 fencing 控制写入权，过期 Worker 的迟到结果不能覆盖当前 Owner
- **工具执行隔离**：工具调用运行在专用 OS 线程，panic 被隔离；副作用工具按幂等键、prepared/committed 状态和验证策略恢复，避免崩溃后重复执行
- **可靠性控制面**：成本页展示 Run、队列、Worker、工具执行器、卡死调用和 SLO；内置故障场景评测及进程/线程崩溃 E2E 门禁

### 7. LAN 局域网访问

内置 HTML 服务（默认 `http://<本机IP>:12345/`），手机/平板/电脑浏览器直接使用：

- 浏览会话列表、查看消息、发送新消息、管理会话（新建/归档/置顶/删除/清空）
- **token 鉴权**：每台设备/访客分配 6 位 token，支持备注、有效期、启用/禁用，失败登录留痕
- **只读文件查看**：`read_project_file` 仅暴露项目内 ≤5MB 文本文件，任何写/删/移动操作不注册到 LAN 路由
- 详情见 [docs/LAN_ACCESS.md](docs/LAN_ACCESS.md)

### 8. 生产力工具

- **命令面板**：`Cmd/Ctrl+K` 唤起，28 个高频工具 action 即时触发（调试/重构/构建/安全/知识/数据/治理/多模态）
- **@ 引用**：输入框 `@` 引用项目文件（MRU 排序）或**同项目其他会话**（`conv:` 前缀注入标题 + 摘要）
- **会话标签与置顶**：给会话打标签、置顶常用项目会话
- **时间线面板**：会话事件溯源可视化（session_events）
- **通知中心**：Agent 任务完成/失败推送
- **性能监控**：PerfMonitor 实时追踪渲染与 IPC 性能
- **审计日志**：工具调用、权限审批全程留痕

## 工作流示例

**从 0 到 1：让 Agent 帮你建一个鸿蒙工程并部署到真机**

```
1. 用户：在 testhy 里建一个 HarmonyOS Stage 工程
2. Agent 拆 plan：create_harmony_project → 写 AppScope → 写 entry → ohpm_install → build_project → deploy
3. 工具流：todo_write → write_file × 7（hvigor/oh-package/build-profile/AppScope/entry...）→ ohpm_install → build_project
4. 失败自愈：build 失败 → show_diagnose_card(category=type) → edit_file 修复 → 重新 build
5. 部署：deploy → start_ability → read_runtime_logs
6. 异常捕获：hilog 检测到 TypeError → 自动落诊断 → Agent 主动修
```

## 技术架构

```
┌─────────────────────────────────────────────────────┐
│  React 19 + TypeScript + Tailwind 4 + Vite 8        │
│  - i18next (中/英/auto) + react-markdown + katex    │
│  - Zustand store: project / theme / chat / memory   │
│  - 14 pages（Home 工作区 + 13 个管理页）              │
└─────────────────────────────────────────────────────┘
                        │ Tauri IPC
┌─────────────────────────────────────────────────────┐
│  Rust (Tauri 2 + hyper + rusqlite + tokio)          │
│  - 298 个 Tauri IPC 入口 · 56 个 service 模块        │
│  - agent/ 37 个顶层模块 · tools/ 29 文件 · 201 工具  │
│  - SQLite + 77 个迁移 · Run/步骤/工具全链路事件溯源  │
│  - 内置运行时：Node + JDK + Git（runtime/）          │
└─────────────────────────────────────────────────────┘
```

### 模块地图

```
src-tauri/src/
├── agent/                  # AI Agent 内核（37 个顶层模块）
│   ├── runtime.rs           #   - Durable Run 状态机与事件游标
│   ├── scheduler.rs         #   - 持久队列、Worker 租约与 fencing
│   ├── coordinator.rs       #   - 执行步骤与恢复检查点
│   ├── context.rs           #   - 长会话分层上下文、事实来源与摘要游标
│   ├── recovery.rs          #   - 副作用感知的恢复计划与验证要求
│   ├── acceptance.rs        #   - 目标契约与工具证据验收
│   ├── governance.rs        #   - 动态预算、可靠性策略与质量快照
│   ├── dag.rs               #   - 主/子 Agent DAG 与依赖调度
│   ├── tool_runtime.rs      #   - 工具 Worker、专用线程、租约与幂等
│   ├── sandbox.rs           #   - 沙箱策略、能力声明与 OCI 启动契约
│   ├── structured_result.rs #   - 工具结果 V2、产物/验证/补偿证据
│   ├── enterprise.rs        #   - SLO、告警、审计与配额
│   ├── evals.rs             #   - 可靠性场景评测与故障注入
│   ├── ask.rs               #   - 主动提问（oneshot 通道）
│   ├── jobs.rs              #   - 后台任务（kill_tree + 512KB 输出环）
│   ├── subagents.rs         #   - 子 Agent 派生（最近 50 条）
│   ├── agent_board.rs       #   - Agent 消息板（A2A pub/sub）
│   ├── reflexion.rs         #   - 失败反思（工具级教训注入）
│   ├── lsp_client.rs        #   - ArkTS LSP 客户端（stdio JSON-RPC）
│   ├── todo.rs              #   - 任务清单（内存 + DB 双写）
│   ├── undo.rs              #   - 撤销栈（每会话 40 条）
│   ├── scanner.rs           #   - 分级代码扫描
│   ├── diagnostics.rs       #   - 跨轮诊断记忆
│   ├── crash.rs             #   - 崩溃归因（JsError / CppCrash / 7 类）
│   ├── runtime_log.rs       #   - 设备运行日志环形缓冲
│   ├── exec_ctx.rs          #   - 工具执行上下文（停止标志）
│   ├── session_ctx.rs       #   - 会话级运行态（统一收敛）
│   ├── invariants.rs         #   - 写操作不变式（.env/证书/迁移 SQL）
│   ├── session_events.rs    #   - 会话事件溯源
│   └── tools/               #   - 201 个 Agent 工具（29 文件）
│       ├── mod.rs               # 工具注册表（TOOL_SPECS）+ 协议分发
│       ├── protocol.rs          # 工具调用标记解析
│       ├── errors.rs            # 结构化错误信封（ToolError 7 类）
│       ├── pipeline.rs          # pre/post hooks 钩子
│       ├── guards.rs            # 钩子实现
│       ├── fs_tools.rs          # 文件系统（115KB）
│       ├── ui_tools.rs          # UI 自动化
│       ├── build_tools.rs       # hvigor 构建
│       ├── device_tools.rs      # 设备调试
│       ├── test_tools.rs        # 测试
│       ├── explore_tools.rs     # 探索
│       ├── project_tools.rs     # 项目
│       ├── compose_tools.rs     # 组合工具
│       ├── meta_tools.rs        # 元工具
│       ├── skill_tools.rs       # Skill
│       ├── debug_tools.rs       # 调试
│       ├── cmd_tools.rs         # 命令
│       ├── memory_tools.rs      # 记忆
│       ├── git_tools.rs         # Git
│       ├── doc_tools.rs         # 文档
│       ├── media_tools.rs       # 多模态
│       ├── web_tools.rs         # Web
│       ├── quality_tools.rs     # 质量门禁 facade
│       ├── quality_metrics.rs   #   质量度量（7 工具）
│       ├── quality_security.rs  #   安全扫描（4 工具）
│       ├── quality_runtime.rs   #   运行时质量（6 工具）
│       ├── quality_media.rs     #   媒体质量（2 工具）
│       └── schedule_tools.rs    # 定时提醒（schedule_create/list/delete）
├── commands/               # 38 个命令模块（合计 298 个 IPC 注册入口）
├── services/               # 业务服务（56 个）
│   ├── proxy_service.rs    #   - 本地代理
│   ├── circuit_breaker.rs  #   - 熔断器
│   ├── model_router.rs     #   - 模型路由
│   ├── embedding.rs        #   - 向量嵌入（GPU 优先自动回退）
│   ├── sdk_api.rs          #   - SDK API 索引
│   ├── lan_server.rs       #   - 局域网访问服务
│   ├── ohpm_landscape.rs   #   - ohpm 生态数据
│   ├── agent_limits.rs     #   - Agent 限额（按任务组）
│   ├── tool_cache.rs       #   - 工具结果缓存
│   ├── reminders.rs        #   - 定时提醒派发（30s 轮询）
│   ├── harmony_*.rs        #   - 鸿蒙集成（6 文件）
│   └── ...
├── db/                     # SQLite + 68 个顺序迁移
├── utils/                  # 工具（13 文件，含任务看门狗）
├── tray/                   # 系统托盘
└── runtime/                # 内置 Node + JDK + Git（约 700MB，不入库，见下）
```

> **关于大文件**：`src-tauri/runtime/`（便携运行时）、`src-tauri/resources/`（种子知识库 + embedding 模型，约 340MB）与 `portable-build/`（绿色版产物）共约 1GB，属构建产物/下载资源，**不随 Git 仓库分发**（见 `.gitignore`）。本机构建请保留这些目录；克隆用户可从 Release 安装包获取完整运行时，或参照 [release.yml](.github/workflows/release.yml) 的下载逻辑自行准备。

## 201 个 Agent 工具按域分组

| 域（TOOL_GROUP） | 代表工具 |
|------|------|
| **build（构建）** | `create_harmony_project` `build_project` `ohpm_install` `ota_pack` `analyze_hap_size` |
| **fix（修复）** | `edit_file` `multi_edit` `undo_edit` `show_diagnose_card` `analyze_crash` |
| **explore（探索）** | `read_file` `list_dir` `find_files` `grep_files` `codebase_search` `get_symbol_details` |
| **deploy（部署）** | `deploy` `deploy_all` `connect_device` `list_devices` `device_file` `device_shell` |
| **refactor（重构）** | `deep_scan` `check_code` `lsp_definition` `lsp_references` `lsp_symbols` `lsp_hover` `lsp_diagnostics` |
| **test（测试）** | `run_tests` `write_unit_tests` `api_test` `api_mock` `api_health` |
| **debug（调试）** | `attach_debugger` `step_debug` `log_query` `read_logcat` `search_hilog` `memory_snapshot` `dump_battery` |
| **other（其他）** | `web_search` `web_fetch` `http_request` `save_memory` `search_knowledge` `spawn_agents` `plan_task` `ask_user` `license_check` `vuln_scan` `docx_read` `audio_transcribe` `memorize` `ui_focus` `schedule_create` `schedule_list` `schedule_delete` |

完整清单见 `src-tauri/src/agent/tools/mod.rs` 的 `TOOL_SPECS` 数组（含中英文描述与 side_effect 标注）。

## 安装

从 [Releases](https://github.com/lookapu/HarmonyAgent/releases) 下载安装包：

- **Windows**: `.exe`（NSIS 安装包）或 `.msi`
- **macOS**: `.dmg` 或 `.app.tar.gz`

最终用户运行上述安装包不需要本地 Python 环境。Python 仅用于部分开发、文档和发布辅助脚本。

### macOS 首次打开

应用未签名，macOS 会阻止。终端一条命令搞定：

```bash
xattr -cr "/Applications/DevEco Switch.app"
```

或：系统设置 → 隐私与安全性 → 滚动到底部 → 点击"仍要打开"

## 从源码构建

```bash
# 按锁文件安装前端依赖
npm ci

# 开发模式（热更新）
npx tauri dev

# 生产构建（需本机已准备 src-tauri/runtime 与 src-tauri/resources，见下方说明）
npx tauri build
```

> **内置运行时说明**：便携版 Node / JDK / Git（约 700MB）与知识库种子、embedding 模型（约 340MB）不随仓库分发。
> 本机构建请保留本地 `src-tauri/runtime/`、`src-tauri/resources/` 目录；CI 会自动从官方源下载运行时（见 [release.yml](.github/workflows/release.yml)）。
> 缺少这些目录时：内置环境（Node/JDK/Git）、API 知识库与向量检索功能不可用，其余功能不受影响。

### 系统要求

- **构建机**：Rust stable、Node 22（与 CI 基线一致）、Tauri 2 系统依赖；打包完整版还需准备 `src-tauri/runtime/` 与 `src-tauri/resources/`（约 1GB）
- **运行机**：Windows 10+ / macOS 11+ / Ubuntu 22.04+

## 文档

- [持续演进任务路线图](docs/ROADMAP.md) — 长会话、Agent 工具链、HarmonyOS 闭环与生态集成的阶段任务和验收标准
- [Agent 能力演进路线（2026）](docs/AGENT_EVOLUTION_ROADMAP_2026.md) — 安全沙箱、大仓理解、真实评测与 12 周执行顺序
- [安全边界与威胁模型](docs/SECURITY_BOUNDARY.md) — 当前保证、明确限制和真实沙箱最低契约
- [官方 DevEco CLI 的 MCP 接入](docs/DEVECO_CLI_MCP_INTEGRATION.md) — 内置 MCP 模板、命令解析增强与自研工具分工策略
- [长会话上下文 V2](docs/CONTEXT_V2.md) — 数据映射、事实优先级、预算和兼容策略
- [架构文档 v2](docs/ARCHITECTURE.md) — 产品定位、模块边界、设计取舍
- [LAN 访问说明](docs/LAN_ACCESS.md) — 局域网服务的启用、token 管理与安全边界
- [工具集增强清单](docs/TOOL_ENHANCEMENTS.md) — 工具能力演进与兑现状态
- [Harness 增强清单](docs/HARNESS_ENHANCEMENTS.md) — 外部参考仓库能力对齐记录
- [更新日志](CHANGELOG.md) — 版本变更、迁移要点与回滚指引

## 开发指南

- 前端入口：`src/App.tsx` + `src/pages/Home.tsx`（Agent Workspace 主界面）
- 后端入口：`src-tauri/src/lib.rs` + `src-tauri/src/main.rs`
- Agent 工具注册：`src-tauri/src/agent/tools/mod.rs` 的 `TOOL_SPECS` 数组
- 数据库迁移：`src-tauri/migrations/`（当前 68 个，已执行的迁移不可修改，新增请递增编号）
- 旧调试脚本：`scripts/legacy/`（仅留档，请勿引用）

## 打赏支持

如果 HarmonyAgent 对你有帮助，欢迎打赏支持，开源维护不易：

<p align="center">
  <img src="docs/alipay-qr.jpg" alt="支付宝打赏二维码" width="200" />
  <img src="docs/wechat-qr.jpg" alt="微信支付打赏二维码" width="200" />
</p>

<p align="center">支付宝 &nbsp;·&nbsp; 微信支付</p>

## License

MIT

---

**致开发者**：这是个工作流极度密集的桌面应用，前端 + Rust + 鸿蒙工具链 三个领域都有深坑。建议先跑 `npx tauri dev` 体验 Agent Workspace，再按需深入某个模块。
