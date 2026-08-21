# Agent Harness 增强计划

> 来源：参考《Harness 工程实战指南》与《AI Agent 工具整合指南（2026 版）》两篇外部资料，
> 结合 DevEco Switch 现有实现（工具编排 / 沙箱审批 / 防打转 / 记忆注入 / 上下文管理）梳理出的可落地改进项。
> 状态说明：`✅ 已完成` `🔄 进行中` `⏳ 待开发`

---

## 一、现状对照（已对齐，无需改动）

| 外部资料主张 | 项目现有实现 | 结论 |
|---|---|---|
| 工具描述写"什么不做/副作用/返回格式" | `agent/tools/mod.rs` 的 `TOOL_SPECS` 含描述与副作用声明 | 已对齐 |
| 错误信息带 suggestion 让 LLM 自我修复 | `diagnose_tool_error` 失败输出追加【诊断建议】 | 已对齐 |
| 危险命令黑名单 + 路径校验 + 输出截断 | `is_dangerous_command` + `resolve_in_project` 越界拦截 + 3000 字符截断 | 已对齐 |
| 重试三件套：区分可恢复/不可恢复错误 | `is_retryable_err` 白名单 + 指数退避（`retry_with_backoff`） | 已对齐 |
| 并行 Agent 抢资源要加锁 | `GATED_TOOLS` 信号量（build/deploy 全局互斥） | 已对齐 |
| 子 Agent 用隔离模式，主 Agent 只看结果 | `spawn_agents` 独立上下文 + 结果汇总 | 已对齐 |
| 上下文按压力触发压缩 | 动态历史预算 + 85% 阈值滚动摘要 + ContextOverflow 降级 | 已对齐（见改进项 1/12） |
| 简单任务用便宜模型（动态路由） | `model_router::pick_economy_model`（子 Agent 已用） | 已对齐（见改进项 2 扩展） |

---

## 二、改进项

### 1. 上下文超限升级：结构化滚动摘要（替代硬裁剪）✅

**目标**：ContextOverflow 时不再直接砍半丢弃历史，先用经济模型把被裁剪部分压成结构化摘要注入，保住关键决策信息。

**现状**：`commands/chat.rs` 中 `history_limit = history_limit / 2` 直接裁剪，无摘要；早期对话的决策、失败教训全部丢失。

**方案**：
- 触发 ContextOverflow 且历史条数 > `MIN_HISTORY_KEEP` 时：
  1. 取将被裁剪的最旧部分（约一半）历史消息
  2. 用经济模型（`pick_economy_model`，无更便宜时回退主模型）非流式生成结构化摘要：
     - 已完成的关键决策（3-5 条）
     - 当前任务状态
     - 待解决问题 / 失败教训
     - 重要工具调用结果（只留工具名和结论）
  3. 摘要存入 `context_summary`，之后每轮组装消息时以 system 消息注入（`## 历史摘要（早期对话）`）
  4. 历史裁剪减半后重试
- 摘要失败（网络/解析错误）时**降级为原硬裁剪**，不阻塞主流程
- 已在循环中注入过摘要时再次超限：结合旧摘要 + 新裁剪部分重新生成（增量更新）

**验收**：长对话触发超限后，早期关键决策（如"已确认使用 xxx 构建命令"）在摘要中可查；降级路径不受影响。

### 2. 非核心推理统一走经济模型 ✅

**目标**：摘要、记忆提取等"非核心推理"用同 Provider 更便宜的模型，主模型只做任务推理。

**现状**：`summarize_memory` 与改进项 1 的摘要均用主模型；经济模型路由仅用于子 Agent。

**方案**：
- `summarize_memory`：取模型后调用 `pick_economy_model`，命中则替换（回退默认模型）
- 改进项 1 的滚动摘要直接使用经济模型

**验收**：使用双模型配置（如主模型 + 便宜模型）时，记忆提取与滚动摘要按经济模型计费；无便宜模型时行为不变。

### 3. 项目记忆注入加相关性排序 ✅

**目标**：记忆注入不再固定按更新时间倒序全量塞入，与当前任务相关的记忆排前面。

**现状**：`SELECT ... ORDER BY updated_at DESC LIMIT 30`，全部记忆一视同仁。

**方案**：
- 从当前用户消息提取关键词（2+ 字符片段，过滤停用词，最多 8 个）
- 按关键词在 title/content 中的命中次数计分，`ORDER BY score DESC, updated_at DESC`
- 无关键词（消息为空/过短）时回退原逻辑
- 保留现有注入护栏（每条 200 字符、总 8000 字符）

**验收**：提及"构建"的任务时，build 类记忆排在最前；无关键词时行为不变。

### 4. 连续失败增加 replan 档（重试 / 改策略 / 终止 三档）✅

**目标**：连续多次工具失败且非打转时，给模型一次"重新规划"机会，而不是直接放弃。

**现状**：可恢复错误自动重试；防打转拦截同参数重复调用；但"换参数还是失败"的场景直接结束。

**方案**：
- 工具循环内跟踪 `consecutive_failures`
- 连续失败 ≥2 且本任务未给过 replan 提示时，下一轮注入：
  `（系统提示：连续多次工具执行失败，请停止当前路径，重新规划整体方案——换工具、换思路或缩小目标；若已无可行路径请直接总结。本轮仍可调用工具。）`
- 成功后计数清零；replan 只给一次，仍失败走现有终止逻辑

**验收**：构建失败 → 换方案（如改用诊断命令）可继续推进；原防打转/终止逻辑不受影响。

### 5. 敏感文件写入保护（环境约束 > Prompt 约束）✅

**目标**：`write_file` / `edit_file` 对敏感文件代码级拦截，不再只靠 prompt 约束。

**现状**：命令有黑名单，但文件写入工具无敏感清单。

**方案**：`agent/invariants.rs` 注册不变式，并由 `agent/tools/fs_tools.rs` 的写入路径统一调用：
- 文件名以 `.env` 开头（`.env` / `.env.local` 等）→ 拒绝
- 扩展名 `.key` / `.pem` / `.pfx` / `.p12` / `.keystore` → 拒绝
- 路径含 `migrations` 且为已存在的 `.sql` 文件 → 拒绝（已执行迁移不可修改，须新建递增编号文件；**新建文件允许**）

**验收**：Agent 尝试改 `.env` 或已执行迁移 SQL 时收到明确拒绝原因；新建迁移文件不受影响。

### 6. 任务进度清单（progress.txt 思路）✅

**目标**：复杂任务（build → deploy → logcat 调试链）展示"计划步骤 + 完成勾选"，停止后可看到干到哪一步。

**现状**：`task_runs` 只记录结果统计（tool_rounds / retry / tokens / 耗时），无计划清单。

**方案**（前端为主，二期）：
- Agent 输出计划列表（Markdown 有序列表）时，前端在任务区渲染为可勾选清单
- 与工具执行卡片联动：步骤对应工具调用成功后自动勾选
- 停止/出错时展示"已完成 N/M 步"

**验收**：多步任务中用户可实时看到进度；任务停止后进度保留。

### 7. 执行循环流水线化（工具护栏钩子化）✅

**目标**：工具执行前的“预算/黑名单/审批”与执行后的“任务记录/大输出落盘”从 chat.rs 主循环内联代码中解耦，形成可组合、可扩展的钩子流水线（对齐 deepseek-harness 的 pipeline 架构优势）。

**现状**：预算检查、黑名单预检、三级审批（约 100 行）与 record_tool 内联在 chat.rs 主循环/子任务循环，新增任何护栏都要改主循环。

**方案**（已完成）：
- `agent/tools/pipeline.rs`：`PipelineRegistry` 静态注册表 + `ToolInvocation` 快照（工具名/参数/项目/会话/审批模式/执行上下文）+ `run_pre_hooks`/`run_post_hooks`（锁内快照克隆、锁外 await，避免跨 await 持锁）
- `agent/tools/guards.rs`：注册 5 个钩子 —— `pre_budget`（任务预算）、`pre_blacklist`（危险命令/文件黑名单）、`pre_approval`（L0/L1/L2 三级 + 项目白名单 + first_write 记忆）、`post_guard`（任务进展记录 + 提示注入）、`post_spill`（>20K 字符输出落盘 .deveco-agent/spill/）
- `InterceptKind` 枚举让调用方按拦截类型收尾：Budget/Blacklist → 请求最终总结后终止；Approval/Generic → 直接终止
- chat.rs 主循环瘦身约 240 行，子任务循环同样接入；`ensure_registered()` 在 lib.rs setup 幂等注册

**验收**：审批/预算/黑名单行为与改造前一致（pipeline 拦截与观察语义有单测）；新增护栏只需注册钩子，不再触碰主循环。

### 8. 后台任务协议（run_in_background + jobs）✅

**目标**：长命令（构建/安装依赖/长测试）不必阻塞模型规划——后台启动立即返回 job_id，完成时结果自动反馈。

**方案**（已完成）：
- `agent/jobs.rs`：进程生命周期托管（tokio spawn + 行级 smart_decode 收集 + 尾部 512KB 缓冲 + 超时/终止/会话清理时 kill_tree 强杀进程树）；每会话并发上限 4 个
- `run_command` 新增 `run_in_background: true` 参数；新增 `job_list` / `job_output` / `job_kill` 三工具
- 完成时 `inject_message` 注入会话队列（上限 10 条，模型下一轮请求自动看到）+ 前端 `chat-job-done` 事件（终端面板记录 + 桌面通知）
- 会话删除/重置时 `drop_conversation_jobs` 联动清理

**验收**：模型启动后台构建后可继续执行其他步骤；任务完成提示自动出现在下一轮对话。

### 9. 工具并发调度（只读并行池 + 模型序提交）✅

**目标**（对齐 deepseek-harness maxParallelToolCalls）：一轮内连续只读工具（L0）并行执行，写工具串行 barrier，减少多步读文件的串行等待。

**方案**（已完成）：
- `MAX_TOOL_CONCURRENCY=4` 有界并行池：主循环把连续 L0 工具收集到 pending 批次，满 4 或遇写工具时 `run_tool_batch` 按 chunks + join_all 并行派发，结果保序
- `is_concurrency_safe`：L0 + 非交互类工具（ask_user/审批/进度类除外）才入池
- 关键简化：主循环工具结果不走 messages 而是 tool_runs（下一轮请求组装时统一注入）→ 批次函数全不可变借用，无需共享 &mut
- 拦截语义并发降级对齐 dsh“drain 已启动调用”：批次内拦截工具不执行，已派发工具照常完成，收集后 Budget/Blacklist 请求最终总结再终止；批次间检查用户停止

**验收**：多文件读取轮次耗时显著下降；工具结果顺序与模型标记序一致；拦截/停止语义与串行路径一致。

### 10. LLM 录制/重放（llm-replay）✅

**目标**（对齐 dsh llm-replay）：无 key 回归 agent 行为——录制真实响应后重放，测试不依赖网络/余额。

**方案**（已完成）：
- `services/llm_replay.rs`：环境变量 `DEVS_LLM_REPLAY=record:dir|replay:dir`；指纹 = model + 消息序列 SHA-256 前 8 字节 hex（消息含工具结果逐轮不同 → 轮级精确匹配）
- 录制：流式响应存原始 SSE 文本流（含 reasoning delta），重放经 `replay_sse_response` 包装为 reqwest::Response 走完整解析路径；非流式（子 Agent）存最终文本；仅正常结束路径落盘
- 重放 fail-closed：未命中报错且不重试，避免回归测试静默打到真实 API；同 key 重复录制取最新一条

**验收**：record 模式跑真实任务后，replay 模式同对话/同模型可无网络复现全部轮次。

### 11. jobs 状态机升级（stopping 态 + 幂等收尾）✅

**目标**（对齐 dsh job 生命周期）：kill 与正常收尾竞争时不再互相覆盖状态。

**方案**（已完成）：
- `JobStatus` 三态（Running/Stopping/Finished）：kill 先 mark_stopping 再杀进程树，超时分支同样先标记
- `finish_job` 幂等（Finished 后不重复写）；最终摘要从 job 记录读取，防 kill 摘要被退出码覆盖
- `job_list` 展示 “⏹ 停止中”；JobInfo 保留 finished 字段兼容前端

**验收**：kill 后状态短暂为 stopping、最终 finished；kill 与 wait 并发收尾不丢摘要。

### 12. 自动压缩触发（context 压力阈值）✅

**目标**（对齐 dsh pressure 触发）：上下文压力达阈值自动滚动摘要，不靠超限报错被动触发。

**结论**：**已存在，无需改动**——主循环已实现 85% 预算阈值自动 `summarize_rolling_history` + 保留最近 N 条 + chat-compact 事件 + compact_keep 持久化，且 ContextOverflow 仍可自动恢复（见改进项 1）。

### 13. subagent 委派约束（toolFilter + maxDepth + persona）✅

**目标**（对齐 dsh 委派三件套）：子 Agent 不再全量继承工具——越权/嵌套滥用由代码级约束。

**方案**（已完成）：
- `SubAgentLimits` 三件套：tool_filter（工具白名单）/ max_depth（可再委派层数）/ persona（角色约束），spawn_agents 顶层与 agents[] 逐任务均可指定，深度缺省继承调用方剩余-1
- 双重生效：系统提示注入（模型自省）+ 子 Agent 工具循环执行前过滤（白名单外工具跳过并注入说明继续）；嵌套 spawn 深度为 0 时直接拒绝
- `ToolCtx.spawn_remaining` 沿委派链递减（主 Agent=1）；run_spawn_agents 入口检查深度上限

**验收**：子 Agent 调用白名单外工具被跳过；嵌套委派超限被拒；persona 出现在子 Agent 系统提示。

### 14. 会话时间旅行（快照回溯，对齐 langgraph checkpoint）✅

**目标**：用户可回到任务任意历史决策点重新引导，而不是只能删消息重来。

**方案**（已完成，2026-08-20）：
- migration `051_conversation_snapshots.sql`：每轮工具执行后自动保存快照（可见消息 rowid 锚点 + 账本 + 模型输出摘要），每会话上限 50 条，首轮无执行痕迹不保存
- `restore_snapshot` 双向恢复：rowid > 锚点的可见消息归档（hidden=1，旧分支保留可回溯）、rowid ≤ 锚点的归档消息重新可见、账本写回快照时刻（续跑/继续任务继承该点执行轨迹）；任务运行中拒绝恢复（防与主循环写消息竞态）
- 前端「会话时间线」弹窗：快照列表（标签/时间/工具数/当前标记）→「回到此处」warn 确认 → 恢复后刷新消息/账本/审计留痕

**验收**：长任务中回到 10 轮前的快照点，后续消息归档隐藏、旧分支重现、续跑从该点轨迹继续。

### 15. 工具循环检测 + 产出前中断冻结重放（对齐 qwen-code / DeepSeek-Reasonix）✅

**目标**：模型打转与流式 0 产出两类"看不见的卡死"由代码级护栏兜底。

**方案**（已完成，2026-08-20）：
- 循环检测（对齐 qwen-code LoopDetectionService 轻量版）：连续相同调用（name+args）/ 连续同名调用（参数抖动）/ 每轮工具总数软硬上限；命中注入纠正提示，最多打断两次后直接收尾（防"纠正-循环-再纠正"空转）
- 冻结重放（对齐 DeepSeek-Reasonix）：流在输出任何内容前中断 → 冻结请求原样重发 ≤5 次（模型无需重新思考、prompt 缓存不失效），超过后走续写收尾
- 无产出静默超时：连接保持但 60s 解析不到有效内容 → 保留已收内容自动续写（与截断续写同链路）；大输出/长响应期间产出持续刷新不会误触发
- 行动承诺假完成纠正：模型宣布开始开发/仅输出方案但无任何工具标记时注入纠正提示（上限防死循环）

**验收**：同一工具连调 3 次以上收到纠正提示；0 产出断流自动重发且不重复计费思考；纯文本回答不回传 reasoning。

### 16. 记忆注入纠偏（负反馈词袋，2026-08-20）✅

**目标**：用户点踩过的"不期望内容"不再反复出现在上下文（改进项 3 相关性排序的反馈闭环）。

**方案**（已完成）：
- 点踩（dislike）消息内容高频词（词频 ≥2 取前 5）写入 `feedback_terms` 词袋（migration 052）
- 记忆注入前加载本项目负反馈词袋：命中 ≥2 个不同词的记忆剔除不注入、命中 1 个的排到末尾（弱相关降权）
- 检索侧同步升级：BM25 重排（tokenizer + Bm25Index，替代 SQL 字典序）+ front_page 最近更新置顶 + pitfall 加权前置（构建场景 boost）

**验收**：点踩含"轮询"的方案后，含轮询关键词的同类记忆不再出现在上下文或沉底。

### 17. 目标契约与证据验收（2026-08-21）✅

**目标**：把“模型说完成了”与“任务真实完成”分离。

**实现**：`acceptance.rs` 从原始目标编译修改、验证、构建、测试、部署、commit、push 等条件；工具轨迹转为带 digest 的证据。写操作之后必须出现覆盖对应产物的读取或全局构建/测试/Git 验证。缺证据自动补救，耗尽补救预算后保持未完成。

### 18. Durable Run 与执行步骤（2026-08-21）✅

**目标**：WebView 刷新、调用 future 被释放或进程异常后，任务状态仍可判断和恢复。

**实现**：`agent_runs`、`run_events` 和 `execution_steps` 持久化状态、阶段、事件游标、prepared/started/finished、幂等键、checkpoint、验收和质量快照；终态不可逆，正常完成清理可重放 delta，异常终态保留诊断现场。

### 19. 持久化调度与 Agent DAG（2026-08-21）✅

**目标**：复杂任务不再只依赖进程内循环，主/子 Agent 的依赖、重试和完成条件可查询。

**实现**：`agent_task_queue` 支持优先级、claim、退避、并发键、resume token 与 checkpoint；`agent_dag_nodes/edges` 记录父子节点、依赖条件、required、failure policy 和独立验收，根任务聚合子节点证据。

### 20. 多进程 Worker 租约与 fencing（2026-08-21）✅

**目标**：多开和进程崩溃时避免双重执行及旧 Owner 覆盖新结果。

**实现**：每个桌面进程登记 Worker 心跳；任务 claim 生成 lease token 和递增 epoch，checkpoint/续租/终态写入均校验 Owner。仅心跳与租约同时过期时回收，并有真实进程崩溃 E2E。

### 21. Tool Execution Kernel 与副作用恢复（2026-08-21）✅

**目标**：工具 Worker 崩溃后，读操作可恢复，写操作不被盲目重放。

**实现**：每次工具调用保存 Worker、lease、attempt、verification state 和 outcome commit；同 Run 副作用以稳定幂等键去重。读取可安全 replay，写入/命令/部署先 verify effect，无法判断时转人工确认；迟到工具结果由 fencing 拒绝。

### 22. 专用 OS 执行线程与卡死归因（2026-08-21）✅

**目标**：工具 panic 或永久阻塞不拖垮 Tokio 主任务池和桌面进程，并能在控制面定位。

**实现**：工具 future 在命名 OS 线程中 `block_on`，panic 由 `catch_unwind` 转为结构化失败；线程 ID/名称登记到 Tool Worker。调用方超时/取消后仍运行的调用标记 stuck，后台租约扫描兜底，`stuck_tools` 进入可靠性面板。

### 23. SLO、评测与可靠性控制面（2026-08-21）✅

**目标**：可靠性不只依靠单元测试，而是有运行指标、故障场景和 CI 门禁。

**实现**：新增 SLO policy、告警、审计、配额、质量分和评测历史；成本页显示验收率、恢复率、证据覆盖率、队列/DAG、Agent/Tool Worker 与卡死指标；CI 增加 reliability、执行内核、进程崩溃和工具线程崩溃 gate。

---

## 三、实施顺序

| 批次 | 内容 | 状态 |
|---|---|---|
| 第一批（后端，低风险） | 1 结构化滚动摘要 + 2 经济模型复用 | ✅ |
| 第二批（后端，中风险） | 3 记忆相关性 + 4 replan 档 + 5 敏感文件保护 | ✅ |
| 第三批（前端） | 6 任务进度清单 | ✅ |
| 第四批（执行循环改造） | 7 流水线钩子 + 8 后台任务协议 | ✅ |
| 第五批（dsh 借鉴） | 9 并发调度 + 10 llm-replay + 11 jobs 状态机 + 12 压缩阈值（已存在）+ 13 subagent 约束 | ✅ |
| 第六批（八仓库盘点） | 14 会话时间旅行 + 15 循环检测/冻结重放 + 16 记忆纠偏（+ 检索 BM25/front_page/RRF、定时提醒、跨会话引用，详见 CHANGELOG v2.2） | ✅ |
| 第七批（商业可靠性内核） | 17 目标契约 + 18 Durable Run + 19 调度/DAG + 20 多 Worker + 21 工具恢复 + 22 专用线程 + 23 SLO/评测 | ✅ |
