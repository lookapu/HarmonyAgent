# DevEco Switch · CHANGELOG

> 面向 HarmonyOS / OpenHarmony 开发者的桌面 AI 编程 IDE。
> 本文件按版本倒序记录用户可见的变更、迁移要点与回滚指引。

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
