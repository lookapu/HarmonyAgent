# DevEco Switch 工具能力盘点

> 当前状态：2026-08-21，`main` 分支。历史需求来源见 `tool-enhancement-backlog.txt`；本文件只描述已经落地的能力、事实源和仍明确暂缓的边界。

## 1. 权威口径

| 项目 | 当前值 | 权威来源 |
|---|---:|---|
| 对外 Agent 工具 | 201 | `agent/tools/mod.rs::TOOL_SPECS` |
| 工具实现文件 | 29 | `src-tauri/src/agent/tools/*.rs` |
| 任务分组 | 8 | `TOOL_GROUP` / `TASK_GROUPS` |
| 权限等级 | L0/L1/L2 | `services/permissions.rs` 与工具 hooks |
| 工具协议 | 文本标记 + 原生 function calling | `protocol.rs` / `commands/chat.rs` |

`TOOL_SPECS` 是工具名称、描述和副作用标记的唯一事实源；本文不复制 201 项完整数组，避免新增工具后出现双份清单漂移。

## 2. 八个任务域

| 域 | 用途 | 代表工具 |
|---|---|---|
| `build` | 工程创建、依赖、构建与产物 | `create_harmony_project`、`ohpm_install`、`build_project`、`ota_pack`、`analyze_hap_size` |
| `fix` | 修改、撤销、诊断和修复 | `edit_file`、`multi_edit`、`undo_edit`、`show_diagnose_card`、`analyze_crash` |
| `explore` | 文件、代码库、知识和 API 探索 | `read_file`、`list_dir`、`grep_files`、`codebase_search`、`search_sdk_api` |
| `deploy` | 设备连接、安装和启动 | `list_devices`、`connect_device`、`deploy`、`deploy_all`、`start_ability` |
| `refactor` | 扫描、符号和 LSP 语义操作 | `deep_scan`、`check_code`、`lsp_definition`、`lsp_references`、`lsp_rename` |
| `test` | 单测、冒烟、API/UI/性能验证 | `run_tests`、`write_unit_tests`、`smoke_test`、`api_test`、`run_ui_flow` |
| `debug` | 日志、调试器、性能与内存 | `attach_debugger`、`step_debug`、`log_query`、`memory_snapshot`、`dump_battery` |
| `other` | Git、Web、MCP、Skill、记忆和治理 | `git_diff`、`web_fetch`、`use_skill`、`spawn_agents`、`schedule_create`、`license_check` |

任务分组同时用于限额、成本统计、权限展示和命令面板，不应在前端维护第二套映射。

## 3. 实现模块

`src-tauri/src/agent/tools/` 按职责拆分：

- `mod.rs`：`TOOL_SPECS`、分组、schema 和总分发；
- `protocol.rs`：文本工具标记解析；
- `contracts.rs`：工具 schema/契约辅助；
- `pipeline.rs` / `guards.rs`：pre/post hook 和审批/预算/安全护栏；
- `errors.rs`：结构化错误与建议；
- `fs_tools.rs` / `explore_tools.rs`：文件读写、搜索、代码扫描；
- `cmd_tools.rs` / `build_tools.rs` / `test_tools.rs`：命令、构建和测试；
- `device_tools.rs` / `debug_tools.rs` / `ui_tools.rs`：设备、调试与 UI 自动化；
- `project_tools.rs` / `compose_tools.rs`：工程分析和组合工作流；
- `git_tools.rs` / `web_tools.rs`：Git 与网络；
- `memory_tools.rs` / `skill_tools.rs` / `meta_tools.rs`：记忆、Skill、Agent 元能力；
- `doc_tools.rs` / `media_tools.rs`：文档与多模态；
- `quality_tools.rs`：质量工具 facade；具体实现拆到 metrics/security/runtime/media 四个文件；
- `schedule_tools.rs`：会话内提醒。

外部模块应通过 facade 或总分发访问质量工具，不直接耦合 `quality_*` 内部文件。

## 4. 已落地能力

### 4.1 文件与编辑

- 项目根路径约束和 canonical path 校验；
- `.gitignore` 感知的目录、glob、grep 与代码库搜索；
- `write_file`、`edit_file`、`multi_edit`、copy/move/delete、Diff 预览和 dry-run；
- 会话级撤销快照；
- `.env*`、密钥/证书和已存在迁移 SQL 的不变式保护；
- 大文件分段读取、长注释折叠和超大输出落盘。

### 4.2 HarmonyOS 工程与构建

- Stage 工程创建、HAP/HAR/HSP 模块识别和 workspace 扫描；
- hvigor/ohpm 调用、通用工程构建、构建错误解析和依赖诊断；
- HAP 大小分析、版本 size diff、签名检查/诊断和 OTA `.pkg` 打包；
- HarmonyOS SDK 对齐、API 兼容扫描和跨版本 API diff。

### 4.3 设备、调试与 UI

- hdc 服务、无线设备、模拟器、应用安装/启动/停止/卸载；
- shell、设备文件、截图、录屏、UI hierarchy、手势和 UI flow；
- hilog/runtime log/faultlog 查询与崩溃分类；
- debugger attach、step/next/continue/where 等调试动作；
- CPU/内存/电池/性能采样与内存快照 diff；
- 网络条件、Wi-Fi、飞行模式和权限设置。

### 4.4 代码理解与知识

- ArkTS LSP definition/references/rename/format/code action/completion/signature/hover/diagnostics/symbols；
- 符号索引、分级扫描、代码度量和变更审查；
- SDK API、HarmonyOS 官方文档、用户知识库和 ohpm landscape；
- BM25 + embedding 的混合检索、RRF、front-page 置顶和负反馈纠偏。

### 4.5 测试、质量与安全

- 单元测试生成/执行、flaky detect、smoke test、UI flow 和性能基准；
- OpenAPI 测试、mock 和健康检查；
- license check、vulnerability scan、代码混淆和 sandbox exec；
- 截图 diff、质量度量、日志聚合、trace replay 和指标导出；
- 输出脱敏、危险命令黑名单、工具缓存、工具健康检查与统计。

### 4.6 Agent 元能力

- `plan_task`、Todo、主动提问、诊断卡和进度更新；
- `spawn_agents`、tool filter、max depth、persona 和 Agent 消息板；
- 后台 job、完成消息注入和 kill-tree；
- 记忆保存/搜索、Reflexion、时间旅行和会话引用；
- MCP 服务发现/调用、Skill 调用、Web 搜索/抓取和会话提醒。

## 5. 工具执行的安全与可靠性

所有工具并非直接从模型字符串进入实现函数，而是经过统一执行链：

```text
模型工具调用
  → schema/参数解析
  → 项目路径与不变式
  → 任务预算/限额/危险命令
  → 权限等级与用户审批
  → execution step + tool lease + idempotency
  → 专用 OS 线程执行
  → 结构化结果/证据/补偿信息
  → Owner fencing 后提交终态
  → 审计、进度、缓存和大输出落盘
```

关键语义：

- L0 且无交互副作用的连续读取可最多 4 路并发；写工具是串行 barrier；
- 工具 future 在线程内 panic 时只失败当前调用；
- 调用方超时/取消而线程未退出会标记 stuck；
- 读取型工具可在崩溃后安全重试；
- 修改、命令和部署等副作用先验证实际效果，不能盲目 replay；
- 相同副作用以幂等键阻止重复执行；
- 旧 Tool Worker 的迟到结果不能覆盖新 Owner；
- 对外文本输出同时保留，结构化 V2 envelope 为验收和恢复提供机器可读证据。

详细执行内核见 `ARCHITECTURE.md`。

## 6. 历史增强批次

| 日期 | 变化 |
|---|---|
| 2026-08-14 | 初版形成 117 个工具，覆盖文件、构建、设备、知识和 Agent 基础循环 |
| 2026-08-16 | 工具扩展至 191，补齐日志查询、文档/音频、调试、内存、OTA、许可证和漏洞扫描，并拆分质量模块 |
| 2026-08-20 | 增至 198：`memorize`、`ui_focus`、`schedule_create/list/delete`，并加入时间旅行、混合检索与循环检测 |
| 2026-08-21 | 工具数不变；新增证据契约、持久调度、DAG、多 Worker、Tool Execution Kernel、专用执行线程和可靠性控制面 |
| 2026-08-22 | 增至 201：`workflow_template`、`team_share`、`reproduction_bundle`，随工作流治理、团队共享与复现包批次落地 |

工具数量只描述对外能力数；2026-08-21 的重点是让已有工具在崩溃、超时、多实例和副作用场景下可恢复，而不是继续增加工具名。

## 7. 明确暂缓项

以下外部服务集成不属于当前 201 工具，保持暂缓：

- Figma 导入；
- 飞书任务同步；
- Jira 同步。

原因是它们需要外部账号、token、权限模型和长期 API 兼容维护，且不影响鸿蒙本地开发闭环。若重新启动，应先定义连接凭据、权限边界、失败补偿和审计要求，而不是只增加一个网络调用工具。

## 8. 新增或修改工具的检查清单

1. 在 `TOOL_SPECS` 登记名称、参数、返回和副作用；
2. 添加 schema 与 dispatcher，确认文本协议和 native tools 都可见；
3. 登记 `TOOL_GROUP`、权限等级、timeout/retry/cost hint；
4. 明确 effect kind、幂等输入、产物和验证方式；
5. 接入路径、不变式、审批和输出脱敏；
6. 为成功、参数错误、权限拒绝、超时和 panic/恢复补测试；
7. 若新增数据库结构，只新增递增迁移，不修改既有迁移；
8. 更新 README/CHANGELOG；不要在本文复制一份会漂移的完整工具数组。

## 9. 验证口径

工具相关变更至少通过：

```bash
npm test
npm run lint
npm run build
cargo test --manifest-path src-tauri/Cargo.toml --locked
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets --locked
cargo test --manifest-path src-tauri/Cargo.toml --locked --test tool_worker_crash_e2e -- --test-threads=1
```

CI 还会执行 Agent reliability、Execution Kernel 和多进程 Worker crash gate。测试通过只能证明已覆盖的不变量未回归；涉及真实 SDK、设备、签名和网络 Provider 的工具仍需对应环境验收。
