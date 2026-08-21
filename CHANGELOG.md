# DevEco Switch · CHANGELOG

> 面向 HarmonyOS / OpenHarmony 开发者的桌面 AI 编程 IDE。
> 本文件按版本倒序记录用户可见的变更、迁移要点与回滚指引。

---

## Unreleased — 证据驱动治理与双层执行内核（2026-08-21）

定位：把“模型能调用很多工具”升级为“任务和工具都可持久调度、可验收、可恢复、可观测”。可靠性与治理批次新增迁移 `057`—`062`、`069`；当前继续推进长会话 Context V2，数据库迁移总数达到 **69**。本批不新增对外工具，`TOOL_SPECS` 仍为 **198**。

### HarmonyOS 工程语义模型

- 新增版本化 `HarmonySemanticModel` 单一解析真源，统一表示应用、产品、嵌套模块、HAP/HSP/HAR 产物类型、Ability、ExtensionAbility 和 OHPM 依赖边。
- 部署所需的 bundle、入口模块、API Level、签名状态和 HAP 输出目录改为从统一模型派生；工程能力面板复用同一模块与依赖口径，不再只扫描根下一层目录。
- 语义模型 schema 升级为 v2：结构化记录根/模块清单来源与解析错误，兼容 OHPM v1/v3 及 targetName 锁文件，并在依赖边同时保留声明约束、锁定版本和锁文件来源。
- 语义模型 schema 升级为 v3：全模块聚合 main pages、router map、权限 usedScene、SystemCapability 检查和 ArkTS/TS 跨模块 import，并生成带清单或源码位置的工程关系边；旧页面摘要改由该图派生。

### 长会话 Context V2（M1 基础）

- Agent 工具参数在统一执行入口增加 schema 级预检：返回 JSON 语法、对象形态、缺失必填项和未知字段的纠错建议；所有参数均不静默改写，令牌、证书、签名与设备标识等敏感字段会显式禁止自动修正。
- 阶段工具选择器升级为证据驱动排序：在能力包先验上结合近 90 天成功率、平均耗时、预计结果 token 成本、副作用等级，以及当前 HarmonyOS 工程、Git 仓库和设备可用性；每轮只暴露得分最高的 32 个工具并记录可解释排名。
- 新增统一执行循环状态机，将目标契约、可验证计划、阶段最小工具集、真实执行证据、独立验证和最终验收收敛到同一快照；阶段变化持久记录为 `workflow.stage` 事件并在每轮重新注入，写入成功不能跳过验证门禁。
- 文件变更会按真实成功轨迹自动生成验证计划：ArkTS/ETS 选择格式化、lint、测试、Hvigor 构建和 diff；通用代码选择格式化、静态检查、测试、构建和 diff；文档改动至少核对 diff。失败写入不会产生虚假验证范围。
- 部署、Ability 启动、Git 提交/推送/合并、数据库迁移、密钥与知识库写入、HTTP 非只读请求新增写后读确认矩阵；部署和 Git 验收必须绑定时间顺序正确的后续状态读取，写入工具自己的成功文本不再自证完成。
- `compose` 多步工具流升级为可恢复逻辑事务：成功步骤写 Durable checkpoint，主步骤失败可走 fallback 降级，整体失败按逆序执行显式补偿，并列出未补偿副作用供人工恢复；禁止嵌套组合事务，未处理失败不再返回伪成功。
- 新增分层上下文模型：任务快照、来源化事实、产物引用、摘要覆盖游标和显式 token 预算。
- 新事实与旧事实冲突时保留历史版本并标记失效；Context 摘要不再被设计为文件、Git、工具或设备状态的唯一真源。
- 新增 Context 投影检查点和失效 epoch，并兼容读取现有会话摘要与任务账本。
- 聊天循环每轮从 Durable Run、执行步骤和来源化事实重建结构化上下文；摘要按消息/事件游标双写检查点，读取失败自动回退旧路径。
- 构建、Git、设备工具结果和产物自动进入 Context 投影；文件修改、分支切换、项目标识变化与设备副作用会使相关旧事实失效。
- Workspace 上下文状态条可展开查看当前目标、分层 token 预算、摘要覆盖游标、事实及产物来源。
- 热上下文新增最近消息、当前错误、活跃文件和待用户确认项；审批、计划审查与 Agent 提问均持久化请求、Owner、超时和终态，重启时只收敛已失联 Run，绝不自动批准。
- 自动与手动压缩后执行摘要—事实对账，附加机器生成的权威事实块；摘要与失败构建/测试、未完成 Run 或待审批状态冲突时记录纠偏审计。
- 新增 120 条消息压缩、SQLite 关闭重开和事实冲突换代回归测试。
- 项目长期记忆升级为 Context V2 项目层，补充架构/构建命令/模块职责/用户偏好分类，以及来源、可信度、版本、确认、固定和显式失效条件。
- 分支、项目身份、文件路径和设备副作用会按记忆声明的条件精准失效旧知识，并在记忆面板保留来源、版本与失效原因供解释。
- 关键消息、人工决策、活跃文件和验收条件可持久固定为权威上下文，跨压缩保留并参与摘要事实对账；原消息置顶入口同步 Context V2。
- 上下文达到主动压缩阈值、超限重试或摘要事实冲突时提供明确通知；恢复核验继续通过进度与错误状态显式反馈。
- 重启后从 Durable Run、步骤、Context 快照和事件游标恢复任务；恢复计划先核验文件、Git、产物、设备及外部状态，持久队列新增安全暂停、继续和取消控制并记录审计。
- 恢复任务时支持增量追加、明确移除和整体替换目标要求；目标契约差异进入事件与审计，旧目标下未完成且不再适用的计划项自动取消，“暂不推送”等否定表达不再误生成验收要求。
- 会话可从消息、检查点、构建失败或 Git 提交锚点创建持久分支；合并严格限制为固定决策、验收条件、产物引用和来源化验证事实，不拼接消息或摘要。
- 子 Agent 委派升级为协议 V2：限定上下文引用、工具范围与嵌套深度，明确不复制父会话全文；返回值统一为带验收、产物、证据、阻塞项和错误的 `SubAgentResultV2`。
- 完成长会话 M2 自动化验收：120 条消息重开恢复、四小时等效 checkpoint/lease 恢复、目标变更、来源追溯与副作用防重放均形成可重复测试证据。
- 工具结果统一为可扩展的 `ToolResultV2`：所有注册工具稳定输出状态、修改、产物、验证、恢复、建议与错误信封，并兼容旧 V2 记录及未知未来字段。
- 工具执行契约补齐副作用、幂等、超时、取消、重试、审批与恢复元数据；Tool Worker 超时改由契约驱动，未知 MCP 工具采用始终审批的保守写入策略。
- 成功与失败的长工具输出统一外部化为受保留策略管理的产物，模型只接收有界头尾摘要和读取引用；`ToolResultV2` 同步记录产物路径。
- 文本与 JSON 脱敏收敛到统一入口，覆盖 token、证书/私钥、签名材料、敏感环境变量、连接口令和设备唯一标识；MCP 错误、长输出产物、工具审计与人工交互不再存在旁路。
- 验证器和恢复动作进入工具契约真源：验证证据不再依赖结果层硬编码，所有副作用工具均声明快照恢复、Git 补偿提交、重新部署、核验后补偿或人工恢复策略。
- 新增项目理解、编译修复、功能开发、重构、构建部署、设备诊断和 Git 交付 7 个能力包；系统提示与原生 tool schema 共用有界选择器，每包声明最小工具集、顺序、停止条件和验收。
- 每轮模型请求根据持久工具证据在 explore/modify/verify/deliver/recover 阶段间切换，动态注入最多 32 个阶段工具；Git 推送仅在验证通过且目标明确要求交付后开放。
- 可靠性面板新增工具治理清单：按窗口识别高失败率与真实长期未使用工具，并列出保守的功能重叠候选及修复、隐藏、合并审查建议。
- 修复 `062_tool_execution_threads.sql` 未登记到统一迁移清单的问题，确保已有用户升级时真实应用 Tool Worker 线程字段。

### 目标契约与证据验收

- 用户目标编译为结构化 `GoalContract`，识别修改、验证、构建、测试、部署、commit 和 push 等必需条件。
- 工具结果转为结构化证据，记录产物、验证范围、错误、补偿策略、指标和 evidence digest。
- 模型只能申请完成；运行内核依据真实工具轨迹裁决。修改后的验证必须发生在最后一次写操作之后，缺证据会自动进入补救循环。
- 达到补救预算仍未满足契约时，Run 收敛为 `interrupted/continuation_required`，不再把自然语言完成声明当作成功。

### Durable Run、调度队列与 DAG

- `agent_runs` 扩展目标契约、动态预算、租约、恢复信息与质量快照；Run 终态不可逆。
- 新增持久化 `agent_task_queue`，支持优先级、claim、退避重试、checkpoint、resume token、并发键和 tenant。
- 新增 Agent DAG 节点/边：主任务和子 Agent 记录依赖条件、失败策略、独立尝试与验收结果；根验收合并子节点证据。
- 新增 execution step 协调与副作用感知恢复：读取可安全重试，写入/命令/部署先验证效果，无法判定时要求人工确认。

### 多进程 Agent Worker

- 每个桌面进程登记唯一 Worker、PID、主机、容量和心跳；启动第二实例不会中断仍健康的第一实例任务。
- 队列 claim 生成 lease token 与递增 epoch，checkpoint、续租和终态写入执行 Owner fencing，旧 Worker 的迟到写入被拒绝。
- 心跳扫描仅回收真正过期或失联 Owner；新增真实进程崩溃 E2E 覆盖认领、进程退出、租约过期和接管恢复。

### Tool Execution Kernel

- `tool_runs` 增加协议版本、结构化结果、幂等键、执行 Worker、租约、尝试、验证状态、恢复次数与 outcome commit 时间。
- 副作用工具采用 prepared → running/verifying → committed 语义；同 Run 的重复副作用按幂等键阻止，迟到结果按 lease fencing 丢弃。
- 实际工具 future 迁到命名专用 OS 线程执行；线程 panic 由 `catch_unwind` 隔离，不拖垮主进程。
- 调用方超时/取消但线程仍运行时标记 stuck，后台同时扫描租约过期调用；控制面新增 `stuck_tools` 指标和 Worker 线程身份。
- 增加卡死线程、不可取消迟到结果、输出洪泛和真实孤儿进程隔离测试；Unix 进程树清理先允许包装器回收已终止子进程，再兜底强杀，避免遗留僵尸 PID。
- 工具质量指标新增成功率、参数错误率、超时率、重试率、取消延迟和平均耗时，并在最终验收后区分直接贡献与“成功但未推进验收”的调用。
- 工具 SLO 新增副作用重复、能力包外错选和无效成功上限；可靠性面板可按工具、能力包、模型、项目和协议/应用版本比较成功率、贡献率与耗时。
- 新增工具协议版本目录和生产者版本维度：V1 历史记录保持只读兼容，V2 保留未知未来字段，后续不兼容变更必须使用新 schema 版本和显式迁移。
- `ToolResultV2` 增加向后兼容的影响说明，失败结果统一给出原因、真实状态影响、已完成部分和恢复下一步；阶段门禁新增 12 个高频工具故障协议矩阵及典型任务裁剪后可完成性测试。
- 新增工具线程 panic、进程崩溃、副作用恢复与重复执行防护 E2E。

### 可靠性控制面与质量门禁

- 新增 SLO policy、告警、审计事件、配额和评测历史；成本页展示验收率、质量分、恢复率、结构化证据覆盖率、队列/DAG、Agent Worker、Tool Worker 和卡死工具。
- CI 在 macOS/Windows 上新增 reliability、Execution Kernel、多进程 Worker crash 和 Tool Worker crash E2E gate。

### 文档校准

- 重写架构文档，以 Rust 后端 Agent 主循环和双层执行内核替换已过时的“前端 TS 编排”方案。
- README 代码规模更新为 198 工具、29 个 Agent 顶层模块、29 个工具文件、33 个命令模块、36 个服务模块、281 个 IPC 入口、68 个迁移和 14 个页面。
- 明确能力批次版本与应用 manifest 版本的口径差异；本批不修改应用发布版本，`package.json`、Cargo 和 Tauri manifest 仍为 `2.0.0`。

### 修复

- 统一首选 HAP 输出目录与递归 fallback 的产物排序：`-signed.hap` 优先于较新的未签名包，避免部署阶段误选不可直接安装的 unsigned 产物；新增回归测试。
- 管理侧栏移除硬编码 `v0.1.0`，改为通过 Tauri `getVersion()` 显示当前 manifest 版本，并保留 `2.0.0` 启动 fallback。

### 迁移

| 编号 | 内容 |
|---|---|
| `057_agent_governance.sql` | 目标契约、补救、Run 租约和质量快照 |
| `058_reliability_control_plane.sql` | 结构化证据、调度队列、DAG、评测 |
| `059_execution_kernel_v2.sql` | 队列协议、工具协议 V2、SLO/告警/审计/配额 |
| `060_multi_worker_runtime.sql` | Agent Worker、lease token、claim epoch、尝试账本 |
| `061_tool_execution_kernel_v2.sql` | Tool Worker、执行租约、验证/恢复与尝试账本 |
| `062_tool_execution_threads.sql` | 工具线程身份与 stuck 计数 |
| `063_conversation_context_v2.sql` | 分层上下文、来源化事实、产物引用与摘要游标 |
| `064_pending_interactions.sql` | 审批、计划审查、Agent 提问的持久生命周期 |
| `065_context_reconciliation.sql` | 摘要与结构化事实的冲突检测及纠偏审计 |
| `066_structured_project_memories.sql` | 项目记忆来源、版本、确认、固定与条件失效 |
| `067_context_pins.sql` | 用户固定消息、决策、文件和验收条件 |
| `068_conversation_branches.sql` | 会话分支血缘与结构化合并清单 |

---

## v2.2 — 八仓库盘点落地：混合检索 + 时间旅行 + 定时提醒 + 跨会话引用（2026-08-20）

定位：对 8 个参考仓库（deepseek-harness / qwen-code / Qwen-Agent / langgraph / OpenHands 等）做全量盘点后的能力落地——检索、会话管理、任务编排、工具集各补一批高价值能力，工具集 **193 → 198**。

### 🔍 检索与记忆升级（desA，对齐 Qwen-Agent）

- **BM25 重排**：新增 `utils/tokenizer.rs`（中文 2-4 字滑窗 n-gram + 英文整词 + 停用词过滤）与 `utils/relevance.rs` Okapi BM25 索引（k1=1.2 / b=0.75，与 rank_bm25 一致）；`keyword_search` / 记忆检索结果从 SQL 字典序改为 **BM25 相关性重排**（标题双份注入近似位置权重 + 时间衰减 + 类别加权）。
- **front_page 置顶**：记忆注入预算充足时，最近更新的 2 条记忆无条件置顶（对齐 Qwen-Agent front_page_search），预算不足自动跳过。
- **RRF 融合**：embedding 向量检索与 BM25 关键词检索双路 RRF 融合（对齐 Qwen-Agent hybrid_search 的混合检索三件套）。
- **pitfall 加权前置**：构建错误修复任务中 build 类历史记忆加权前置，Agent 动手前先看到本工程踩过的同类坑。

### 🧭 会话时间旅行（对齐 langgraph checkpoint）

- **快照自动保存**（migration `051_conversation_snapshots.sql`）：每轮工具执行后保存状态锚点（可见消息 rowid + 账本 + 模型输出摘要），每会话上限 50 条，首轮无执行痕迹不保存。
- **双向恢复**：`restore_snapshot` 归档锚点后的消息（hidden，旧分支保留可回溯）、重现锚点前的归档段、账本写回快照时刻（续跑继承该点执行轨迹）；任务运行中拒绝恢复（防写消息竞态）。
- **前端时间线**：更多菜单 →「会话时间线」弹窗，快照点列表（标签/时间/工具数/当前标记），「回到此处」warn 确认后恢复并刷新消息/账本/审计留痕（`task.timeline`）。

### ⏰ 定时提醒（对齐 deepseek-harness schedule）

- 新工具 `schedule_create`（after / at / every 三类，错误码含 invalid_prompt / invalid_selector / not_future / frequency_too_high）/ `schedule_list` / `schedule_delete`；every 锚点推进（错过不枚举历史周期）。
- 新服务 `services/reminders.rs` + migration `052_reminders_feedback_terms.sql`（`message_reminders` 表）；lib.rs setup 30s 轮询派发到期提醒 → 会话队列注入（`inject_message`，session-local 不中断当前轮次）+ 桌面通知。

### 📊 消息反馈纠偏（A2）

- 点踩（dislike）消息内容高频词（词频 ≥2 取前 5）写入 `feedback_terms` 词袋；记忆注入前加载负反馈词袋，命中 ≥2 个不同词的记忆剔除不注入、命中 1 个的排到末尾——用户不期望的内容不再反复出现在上下文。

### 🛡 不变式守卫（A5）

- 新增 `agent/invariants.rs` 注册表（`Invariant { name, check }` + 静态数组 + `check_write` 统一入口），3 条不变式：`.env*` 前缀文件、8 种密钥/证书后缀（`.key/.pem/.pfx/.p12/.keystore` 等）、已存在的 `migrations/*.sql`（已执行迁移不可修改，新建允许）；`fs_tools::is_protected_file` 收拢为委托注册表，含 2 个测试。

### 🔗 跨会话引用（B6）

- `references_json` 支持 `conv:<id>` 前缀：历史重放时注入会话标题 + 摘要（`messages.summary` 非空优先，回退最后一条 assistant 内容，单会话 2000 字符 / 总 8000 上限）；前端 @ 面板追加会话候选（同项目、排除当前、标题模糊匹配、chat 图标），选中即把标题 + 最近内容插入草稿，与消息引用（Quote）同构。

### 🛠 流式健壮性加固

- **无产出静默超时**：连接保持但 60s 解析不到有效内容 → 保留已收内容自动续写（与截断续写同链路）。
- **产出前中断冻结重放**：流在输出任何内容前中断 → 冻结请求原样重发（≤5 次，对齐 DeepSeek-Reasonix 机制，模型无需重新思考、prompt 缓存不失效）。
- **工具循环检测**（对齐 qwen-code LoopDetectionService 轻量版）：连续相同调用（name+args）/ 连续同名调用（参数抖动）/ 每轮工具总数软硬上限，命中注入纠正提示，最多打断两次后收尾。
- **行动承诺假完成纠正**：模型宣布开始开发/仅输出方案计划但无任何工具标记时，注入纠正提示要求立即执行（上限防死循环）。
- **reasoning_content 多轮合规**：DeepSeek 推理模型携带 tools 的请求完整回传思考链（缺失导致 400/思考链断裂）；V4 thinking 模式 content 数组块解析（text 块进正文 / thinking 块归推理）；仅带工具调用的 assistant 消息回传 reasoning（纯文本回答不回传、不占输入预算）。
- **run_command 输出超限落盘**：响应超限时全文落盘 + 头尾采样 + `store_overflow` 路径标记，Agent 可按需读回完整输出。

### 🧰 其他

- `ui_focus` 工具（对齐 OpenHands canvas_ui_control）：Agent 产出后驱动 UI 聚焦（切换右侧面板 / 打开文件预览，L0 权限）。
- `memorize` 工具 + `replay_memories`（对齐 Qwen-Agent MemoAssistant）：从历史消息重放 memorize 调用重建键值状态，每轮作为 system 注入。
- 文件树面板：展开但缓存缺失时自动重新加载（刷新后已展开目录免手动再点）。
- logger 测试隔离修复（pid 复用残留文件导致偶发断言失败）。

### ✅ 验证

- `cargo check`：0 error / 0 warning
- `cargo test --lib`：**446 passed / 0 failed**（新增 reminders 2 + invariants 2 + 检索/协议若干）
- 前端 `tsc --noEmit`：通过

### 🔄 迁移要点

- 新增迁移 `051_conversation_snapshots.sql`、`052_reminders_feedback_terms.sql`（已执行库自动应用，无破坏性变更）。
- 工具总数 193 → **198**（+memorize / ui_focus / schedule_create / schedule_list / schedule_delete）；`TOOL_SPECS` 数量以 `src-tauri/src/agent/tools/mod.rs` 为准。
- `inject_references` 签名新增 `conn` 参数（conv: 会话摘要查询）；内部调用点已同步。

---

## v2.1 — 对话流转加固 + 极简留白 UI（2026-08-19）

定位：围绕"对话能否正常流转"做一次全面体检与修复，解决停止/删除/审批/错误态等边界场景的状态不一致，并把对话区视觉改为极简留白风格。

### 🐛 对话流转修复（后端）

- **停止语义修复**：用户点停止后，不再自动续跑排队消息（`stream_chat_body` 在 `stats.stopped` 时终止队列消费），避免"点了停止，过会儿 AI 又自己开始干活"。
- **删除运行中会话**：`delete_conversation` 改为先停止 + abort 后台任务（新增 `TaskRegistry::abort_conversation`）+ 释放项目锁，再删库，消除孤儿任务、继续写文件和项目锁长期占用问题。删除时同步清理 `tool_limits` / `task_guard` 进程内状态，修复内存随会话数单调增长。
- **审批/计划审查中停止**：新增 `InterceptKind::Cancelled`、`ApprovalOutcome::Cancelled`、`PlanReview.cancelled`，工具审批/计划审查等待期间点停止，现在按"停止"收尾（`chat-stopped`），而非被当成"拒绝"导致任务继续跑一轮或显示为正常完成。串行与批处理工具路径均已覆盖。
- **任务看门狗**：`TaskRegistry` 统一登记所有 `stream_chat` 任务，8 分钟无心跳 / 40 秒停止未生效时强制 abort 并 emit `chat-error`；`stream_once` 内按阶段（发送→首字节→流式→解析）高频 touch。
- **新增迁移 `050_task_ledger.sql`**：持久化 `task_runs.target_text / target_passed / target_evidence`。
- `chat-done` 事件新增 `user_message_id` 字段，供前端替换乐观占位。

### 🐛 对话流转修复（前端）

- **错误态与流式残影共存**：出错时清 `conversationId` / `startedAt`，打字光标/三点动画立即消失，只保留已生成内容 + 错误卡。
- **乐观 user 消息 ID 不替换**：`chat-done` 用真实 `user_message_id` 替换 `local-` 占位，当前会话周期内的编辑/删除/分支重生成/Fork 立即生效。
- **停止兜底计时器误杀新任务**：`stopGeneration` 的 60s 兜底用 `startedAt` 代次 token 校验，停止后立即重发不再被旧计时器置错。
- **看门狗误杀后台审批会话**：改为按 `pendingConfirmations[convId]` 判断（含后台会话），而非仅当前会话视图数组。
- 乐观消息 ID 加随机后缀避免跨会话同秒碰撞；排队失败弹错误通知；`chat-done` 按完成会话自身 `project_id` 刷新列表；新增 `conversation-deleted` 事件监听（多端/LAN 删除时同步清理并切换会话）。

### 🎨 对话区极简留白样式

- 消息头模型/耗时/token/消息ID 徽章**默认隐藏，悬浮显示**；assistant 头像去紫色渐变改朴素圆点；用户气泡去彩边/阴影改中性背景。
- 工具卡 / 子 Agent / 计划卡 / 账本卡 / 任务过程条统一为**纯文字行 + 折叠**：去掉彩色背景、左侧竖条、图标底色块、阴影和完成脉冲。
- 思考块（ThinkingBlock）改左侧细竖线；错误卡弱化为中性边框；CSS 中 `.task-*` 类去背景/竖条/阴影。

### ✅ 验证

- `cargo check`：0 error / 0 warning
- `cargo test --lib`：**418 passed / 0 failed**（含 ask/guards/pipeline）
- 前端 `tsc --noEmit`：通过

### 🔄 迁移要点

- 新增迁移 `050_task_ledger.sql`（已执行库自动应用，无破坏性变更）。
- `delete_conversation` 命令由同步改为 `async`，签名新增 `app/cancel/lock/registry` 状态参数；LAN 服务改用同步内部函数 `delete_conversation_sync`，HTTP 行为不变（删除仍会级联清理运行中任务）。

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

- 工具总数以 `src-tauri/src/agent/tools/mod.rs` 中 `TOOL_SPECS` 数组长度为准（当前 198）。
- 任务分组以 `TASK_GROUPS` 常量为准（当前 8 个：`build` / `fix` / `explore` / `deploy` / `refactor` / `test` / `debug` / `other`）。
- `quality_tools::*` 通过 facade 暴露，**禁止**直接 import 4 个子文件（`quality_metrics` 等）—— 内部模块，外部耦合面随 facade 走。
- 任何对工具的"按行数切分"禁止。**必须按方法完整切片**，签名 + 函数体在同一文件内。脚本辅助可见 `scripts/legacy/_split_quality.py`。
- CHANGELOG 任何变更在 commit message 里写 `docs(changelog): <一句话>`，不要直接编辑本文件然后 commit `docs:`。
