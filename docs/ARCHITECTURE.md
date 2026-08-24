# DevEco Switch 架构说明

> 本文描述 2026-08-21 `main` 分支的实际实现。历史规划与能力盘点分别见 `CHANGELOG.md`、`TOOL_ENHANCEMENTS.md` 和 `HARNESS_ENHANCEMENTS.md`。

简体中文 | [English](ARCHITECTURE.en.md)

## 1. 产品定位

DevEco Switch 是面向 HarmonyOS/OpenHarmony 开发者的本地桌面 Agent 工作台。用户选中工程并用自然语言下达目标，Agent 负责读取和修改代码、构建、测试、部署、读取设备日志并验证结果。

它坚持三条产品边界：

1. **任务模式优先**：对话是操作主线，计划、工具、Diff、日志、审批与验收都在任务上下文中呈现。
2. **不替代专业 IDE**：提供文件树、只读预览和必要的简单编辑，但不建设多标签编辑器、完整终端或全量 IDE 插件体系。
3. **鸿蒙闭环优先**：通用 Agent 能力服务于 HarmonyOS 工程理解、hvigor/ohpm 构建、hdc 设备控制和 SDK/API 兼容性分析。

项目最初是 Provider 管理器，现在的核心已经是 Rust 后端 Agent 执行内核；Provider、代理、成本和健康检查是其基础设施，而不是产品终点。

## 2. 架构总览

```text
┌──────────────────────────────────────────────────────────────┐
│ React 19 + TypeScript + Zustand                              │
│ 项目/会话、流式消息、计划/审批/工具卡、文件与设备面板、管理页 │
└────────────────────────────┬─────────────────────────────────┘
                             │ Tauri invoke / event
┌────────────────────────────▼─────────────────────────────────┐
│ Rust / Tauri 2                                               │
│                                                              │
│ commands/chat.rs                                             │
│   上下文组装 → 模型流式请求 → 工具解析 → 执行循环 → 验收      │
│                                                              │
│ Agent Execution Kernel                                      │
│   Run 状态机 / 执行步骤 / 调度队列 / DAG / 恢复 / 治理       │
│                                                              │
│ Tool Execution Kernel                                       │
│   201 工具 / 审批流水线 / 专用线程 / 租约 / fencing / 幂等   │
│                                                              │
│ Services                                                     │
│   Provider/代理/熔断/成本、鸿蒙环境、知识库、MCP、LAN         │
└────────────────────────────┬─────────────────────────────────┘
                             │
┌────────────────────────────▼─────────────────────────────────┐
│ SQLite（77 个迁移） + 本地文件/钥匙串 + 外部工具链           │
│ HarmonyOS SDK / hvigor / ohpm / hdc / ArkTS LSP              │
└──────────────────────────────────────────────────────────────┘
```

当前代码规模口径：

| 项目 | 实际值 |
|---|---:|
| Agent 对外工具 | 201 |
| `agent/` 顶层模块（不含 `mod.rs`） | 36 |
| `agent/tools/` Rust 文件（含 `mod.rs`） | 30 |
| `commands/` 命令模块（不含 `mod.rs`） | 38 |
| `services/` 服务模块（不含 `mod.rs`） | 56 |
| Tauri IPC 注册入口 | 298 |
| 数据库迁移 | 77 |
| React 页面 | 16 |

以上计数会随代码演进变化；工具数以 `TOOL_SPECS`、IPC 入口以 `lib.rs` 的 `generate_handler!`、迁移数以 `src-tauri/migrations/` 为准。

## 3. 前端

### 3.1 页面与路由

`src/App.tsx` 使用 React Router。根路由 `/` 是 Agent Workspace（`Home.tsx`），其余 13 个页面负责 LAN、Provider、运行时版本、配置、限额、成本与可靠性、代理、MCP、Skill、知识库、HarmonyOS API、健康检查和 ohpm 生态。

所有页面按路由懒加载，避免 Recharts、Markdown/KaTeX 和设备诊断代码阻塞首屏。

### 3.2 状态模型

`projectStore.ts` 组合三个 Zustand slice：

- `projectSlice`：项目、工作区模块、文件树和 Git 分支；
- `chatSlice`：会话、消息、流式分桶、工具运行、审批、计划和待确认事项；
- `memorySlice`：记忆、反馈、统计和消息版本。

流式状态以 `conversation_id` 分桶，而不是只保存“当前会话”。用户切换项目或会话后，后台任务的增量、工具事件和终态仍写入对应桶。

### 3.3 IPC 与事件

前端 API 模块只封装 Tauri `invoke`。长任务使用事件流更新 UI，主要事件包括正文/推理增量、工具开始与结束、子 Agent、计划审查、审批、任务账本、治理状态、完成、停止和错误。

SQLite 与 Rust 状态机是任务真实状态来源；前端计时器和看门狗仅用于展示与兜底，不能覆盖后端终态。

## 4. 对话与 Agent 主循环

Agent 编排实际位于 `src-tauri/src/commands/chat.rs`，不是前端。

一次 `stream_chat` 的主流程为：

1. 按会话注册唯一任务和 AbortHandle，拒绝同会话并发写入；
2. 写入用户消息，创建 Run、根 DAG 节点和持久队列记录；
3. 编译目标契约与动态执行预算；
4. 组装系统提示、工程规则、历史摘要、记忆、诊断、Skill、MCP、引用和任务账本；
5. 选择 Provider/模型并发起 OpenAI、Anthropic 或 Gemini 协议的流式请求；
6. 解析原生 function calling 或文本工具标记；
7. 执行审批、预算、工具调用、重试和结果持久化；
8. 将工具结果注入下一轮，直到模型收尾或触发停止/预算/错误；
9. 用真实工具证据验收目标；缺证据时自动补救，满足契约后才能进入 `completed`；
10. 先收敛 Durable Run 终态，再写任务统计并向前端发送完成事件。

主循环的重要防护包括：

- 首字节前断流冻结重放、输出后中断续写、空响应重试；
- 上下文达到阈值时滚动摘要，保留最近消息；
- 连续相同工具、连续同名工具和总工具数循环检测；
- 连续失败后重新规划；
- 只读工具最多 4 路并发，写工具作为串行 barrier；
- 工具轮次与时长预算按目标复杂度动态计算，仅在持续取得证据时扩容；
- 模型的“已修复/已验证/已完成”声明不能代替工具证据。

### 4.1 长会话 Context V2

`agent/context.rs` 将长会话拆成热消息、任务状态、项目事实和历史归档四层。它是可重建投影，不替代 `messages`、事件、Durable Run、工具结果或工作区真实状态。

- `TaskSnapshotV2` 每轮从最新 Run、目标契约和 execution step 重建；旧会话兼容读取任务账本。
- 摘要记录覆盖的消息 rowid 与事件 seq；Context 检查点保留最近 80 个版本。
- 构建、Git、设备结果和工具产物转为来源化事实或引用，保存来源、digest、可信度、版本和作用域。
- 事实变化时旧版本显式失效；文件修改、分支切换、项目标识变化和设备副作用会使相关事实失效并递增 epoch。
- token 窗口先预留模型输出，再分配给系统、任务、项目、归档和最近消息；Workspace 可查看预算、摘要游标和事实来源。
- Context V2 读取或写入失败时聊天继续走兼容路径，原始消息、Run 和事件仍可用于恢复。

详细数据映射和裁决优先级见 `CONTEXT_V2.md`。

## 5. 目标契约与证据验收

`agent/acceptance.rs` 将用户目标编译为 `GoalContract`。当前可识别的验收类型包括：

- 修改真实落地；
- 修改后的独立验证；
- 构建；
- 测试；
- 部署；
- Git commit；
- Git push。

工具运行会转为结构化证据。对于写操作，验证必须发生在最后一次修改之后；普通 `read_file` 只有覆盖被修改的目标才算验证，构建、测试和 Git diff/status 可作为全局验证器。

验收未通过时，运行内核会给模型补救提示并继续工具循环；达到补救预算仍缺证据时，Run 标记为 `interrupted/continuation_required`，不会伪装成成功。

## 6. Durable Agent Runtime

### 6.1 Run 与事件

`agent/runtime.rs` 管理 `agent_runs` 和 `run_events`。Run 保存目标、状态、阶段、尝试次数、事件序号、父 Run、恢复计划、目标契约、租约、验收结果和质量快照。

有效状态包括 `queued`、`running`、`waiting_approval`、`waiting_user`、`verifying` 以及终态 `completed`、`failed`、`cancelled`、`interrupted`。终态不可逆，迟到的看门狗或旧 Worker 不能把已完成任务改回失败。

### 6.2 执行步骤与恢复

`coordinator.rs` 把工具调用持久化为 execution step，记录 prepared/started/finished 和幂等键。`recovery.rs` 根据副作用域决定恢复动作：

- 纯读取可安全重试；
- 文件修改、命令、部署等副作用需要先验证效果；
- 无法证明是否已生效的操作要求人工确认；
- 已有可信结果的步骤直接复用，避免重复执行。

会话快照是用户可见的“时间旅行”；Run/step checkpoint 是执行内核的故障恢复，两者用途不同。

## 7. 调度、DAG 与多 Worker

### 7.1 持久队列

`agent/scheduler.rs` 管理 `agent_task_queue`：优先级、最大尝试次数、退避时间、并发键、checkpoint、resume token、Worker Owner 和租约都持久化。

Worker 每 5 秒写心跳并回收过期 Owner。认领任务会生成 lease token 和递增 epoch；之后的 checkpoint、续租和终态写入都校验 Owner，形成 fencing，防止旧进程的迟到结果覆盖新 Owner。

### 7.2 DAG

`agent/dag.rs` 将主 Run 和子 Agent 表示为节点，边支持依赖条件和 required 标志。节点具备独立的尝试次数、失败策略、下一次尝试时间、输出摘要和验收结果。

根任务验收会合并子节点证据；子 Agent 的自然语言结论不能绕过根契约。

### 7.3 多进程恢复

每个桌面进程注册唯一 `agent_workers` 记录。进程退出时标记 stopped；异常退出后，其他实例只回收心跳过期且租约失效的任务，不会在第二个实例启动时抢走仍然健康的任务。

## 8. Tool Execution Kernel

### 8.1 工具注册与协议

`agent/tools/mod.rs` 的 `TOOL_SPECS` 是 201 个对外工具的权威清单，包含名称、说明和副作用标记。工具既支持文本标记协议，也支持 OpenAI 兼容的原生 function calling；MCP 与 Skill 工具在运行时动态注入。

工具按 build/fix/explore/deploy/refactor/test/debug/other 八个任务域进行限额和统计。

### 8.2 执行流水线

工具执行经过 pre/post hooks：预算、危险命令、路径边界、敏感文件不变式、权限审批、任务进度、审计和大输出落盘。安全边界位于 Rust，不能由前端或模型绕过。

### 8.3 专用线程与崩溃隔离

`agent/tool_runtime.rs` 为每次工具调用创建执行租约，并把实际调用放到命名 OS 线程中。线程身份登记到 `tool_execution_workers`；panic 由 `catch_unwind` 隔离，只结束该执行线程，不拖垮桌面进程。

调用方超时或取消后，仍未退出的线程会标记为 stuck；后台扫描同时检测租约过期调用。`stuck_count` 和 `stuck_tools` 暴露到可靠性控制面。

### 8.4 幂等与副作用恢复

工具调用使用稳定幂等键和 lease token。结果提交校验当前 Worker Owner，迟到结果会被 fencing 丢弃。对于崩溃时处于 prepared/running 的副作用工具，内核先进入 verifying，再依据结构化产物和恢复策略决定复用、重试或人工确认。

## 9. 结构化结果、治理与可观测性

`structured_result.rs` 将传统文本工具输出包装为 V2 envelope，包含：

- 产物路径和类型；
- 验证类型与范围；
- 错误分类、错误码和是否可重试；
- 补偿/恢复策略；
- 耗时、输出规模等指标；
- 稳定 evidence digest。

`governance.rs` 根据目标复杂度生成动态工具轮次、最长时长、补救次数、租约和模型回退策略，并在终态生成质量分。

`enterprise.rs` 提供本地 tenant 的 SLO、告警、审计和配额累计；`evals.rs` 运行 16 个执行内核可靠性场景和 10 个鸿蒙固定任务场景，并把逐场景 expected/actual 与 schema v1 执行快照（模型/工具/SDK/设备/Token/成本/证据摘要）写入评测历史；`versioning.rs` 汇总数据库、工具协议、Skill/工作流规范、知识索引与评测 schema 的当前版本和兼容承诺。成本页通过 `commands/reliability.rs` 展示：

- Run 状态和验收率；
- 调度队列、恢复任务和 DAG 节点；
- Agent Worker 与 Tool Worker；
- 工具卡死、失败和恢复统计；
- SLO、告警、审计与最近评测。

## 10. 鸿蒙能力层

鸿蒙能力分布在 `services/harmony*.rs`、`agent/tools/build_tools.rs`、`device_tools.rs`、`debug_tools.rs` 和 `project_tools.rs`：

- 探测 DevEco Studio、HarmonyOS SDK、command-line-tools、JDK、Node 和 Git；
- 把工程清单、ArkTS/ArkUI、API 导入及构建/崩溃日志合并为带置信度和相对来源证据的鸿蒙指纹，供工程理解和能力包选择复用；
- 扫描 HAP/HAR/HSP 模块、bundleName、API 版本、页面和签名配置；
- 调用 hvigor/ohpm 构建并解析错误与产物；
- 通过 hdc 管理设备、安装、启动、截图、日志、性能和文件；
- 启动 ArkTS language server，提供 definition/references/symbols/hover/diagnostics；
- 查询内置 SDK API、官方文档、版本 diff 和兼容性；
- 浏览 ohpm landscape 并提供依赖建议。

详细规则见 `HARMONY_INTEGRATION.md`；指纹边界和固定评测见 `FIXED_EVALUATION_SUITE.md`。

## 11. Provider、代理与模型协议

Provider 和 model 保存于 SQLite，API key 通过系统钥匙串管理。会话可以选择模型、协议、采样参数、推理强度、是否走代理和是否启用原生工具调用。

本地代理负责请求转发、熔断、自动 failover、重试、成本和请求日志。多开时由 `ProxyLock` 保证只有一个实例持有代理端口，其他实例共享它。

模型层支持 OpenAI、Anthropic 和 Gemini 的请求/流式响应格式；工具协议、reasoning content 和图片输入按具体协议转换。

## 12. 数据与存储

SQLite 使用 WAL 和外键约束，迁移在启动时顺序执行。当前 77 个迁移覆盖：

- Provider、模型、代理、成本和请求日志；
- 项目、会话、消息、引用、标签、反馈和版本；
- 工具运行、事件、任务账本、快照和提醒；
- Skill、MCP、知识库、API 文档、ohpm landscape；
- Durable Run、execution step、恢复计划；
- 调度队列、DAG、Worker、尝试账本；
- SLO、告警、审计、配额和工具执行 Worker。
- 固定评测结果、版本化执行快照、环境/资源元数据和最终证据摘要。
- 长会话 Context V2 状态、来源化事实、产物引用和摘要检查点。
- 待审批、计划审查和 Agent 提问的持久生命周期及失联恢复依据。
- 模型摘要与结构化事实的压缩后对账、冲突码和纠偏审计。

已发布迁移不可修改；数据库不变式会拒绝改写已存在的 `migrations/*.sql`，新结构必须新增递增编号迁移。

其他本地数据包括日志、symbol cache、spill 大输出、内置运行时和种子知识库。敏感 Provider key 不保存到普通配置或 SQLite 明文字段。

## 13. MCP、Skill 与 LAN

- **MCP**：已实现服务器 CRUD、导入导出、测试、长驻 stdio 客户端、工具发现和调用转发。Agent 只加载与当前项目精确绑定且已授权的实例；全局配置仅作模板，工具/目录/网络/凭据白名单在发现与调用两处复验，连接配置变化会使授权失效。详细边界见 [MCP 项目授权与作用域](MCP_PROJECT_AUTHORIZATION.md)。
- **Skill**：支持本地 Skill 管理、GitHub 导入、启停、克隆和使用统计，启用内容按项目注入 Agent 上下文。
- **LAN**：内置 Hyper 服务和独立原生 Web UI，提供 token 鉴权、项目/会话管理、消息发送、SSE 更新和项目内只读文件访问。LAN 不暴露任意写文件、命令或设备控制接口。

## 14. 启动与退出

Tauri setup 依次完成：

1. 注册工具护栏；
2. 初始化日志、SQLite、迁移和全局 DB；
3. 启动 Run/Tool Worker 心跳与过期恢复扫描；
4. 导入种子 API 知识库并按需刷新 ohpm 数据；
5. 启动提醒调度；
6. 初始化托盘、快捷键、MCP manager 与内置 Node/JDK/Git；
7. 探测 HarmonyOS 工具链；
8. 按配置自动启动本地代理和 LAN 服务。

退出时停止当前 Worker、回收 MCP 子进程，并由锁持有者停止代理和 LAN 服务。

## 15. 质量门禁

`.github/workflows/quality.yml` 在 macOS 和 Windows 上执行：

- `npm test`、ESLint、前端生产构建；
- `cargo test --locked` 与 Clippy；
- Agent reliability gate；
- 固定评测 CI 基线门禁（`ci_baseline_gate`）：保存/恢复可跨机器比较的基线，阻止任务完成率、评测覆盖或关键延迟显著回退，主分支保存基线、PR 只比较；
- Execution Kernel 模块测试；
- 多进程 Worker 崩溃恢复 E2E；
- 工具 Worker 崩溃与副作用恢复 E2E。

可靠性设计的基本原则是：**模型输出不是事实，持久状态不是装饰，副作用不能盲目重放，完成必须有证据。**

## 16. 当前结构性风险

以下是实际代码风险，不是未实现功能：

1. `commands/chat.rs` 同时承担协议、上下文、工具循环、恢复和持久化，文件过大，后续应按不破坏状态机边界的方式拆分；
2. `pages/Home.tsx` 仍然庞大，虽已拆出多组 chat components，但布局和交互状态仍高度集中；
3. `agent/tools/mod.rs` 同时承担 201 个 schema 与总分发，新增工具时必须同步验证注册、权限、分组和结构化结果；
4. README/CHANGELOG 中的能力批次版本与应用 manifest `2.0.0` 不是同一口径，发布前应统一正式版本策略；
5. 内置 runtime/resources 不随 Git 分发，干净克隆只能运行不依赖这些资源的测试和精简构建。

维护时应优先保持终态不可逆、Owner fencing、迁移只增不改、工具证据可追溯和前后端事件幂等这五个不变量。
