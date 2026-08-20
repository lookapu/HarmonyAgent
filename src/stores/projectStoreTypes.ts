import type {
  Project,
  Conversation,
  NewConversationWorktree,
  ChatMessage,
  GitBranchInfo,
  FileTreeNode,
  ChatOptions,
  TodoItem,
  ProjectMemory,
  ToolStat,
  ToolTokenStat,
  MessageFeedback,
  MessageVersion,
  MemoryDraft,
  QueuedMessageInfo,
  RollbackInfo,
  ConversationCostStats,
  PendingConfirmation,
  TaskLedger,
  SnapshotInfo,
  RestoreSnapshotResult,
} from '../api/project'
import type { TaskRun } from '../api/cost'

/** 流式回复状态 */
export interface StreamingState {
  conversationId: string | null
  /** 后端任务运行代次；用于丢弃停止/重试后的旧流式与终态事件。 */
  runId: string | null
  content: string
  /** 思考过程（推理模型 reasoning 流，无则空串） */
  reasoning: string
  /** 兼容旧逻辑的完整错误文本（invoke reject 与结构化事件共用） */
  error: string | null
  /** 结构化错误详情（chat-error 事件，前端友好卡片展示） */
  errorDetail: ChatErrorDetail | null
  /** 任务开始时间戳（前端计时显示，ms） */
  startedAt: number | null
  /** 最近一次内容/思考增量时间戳（静默检测：长时间无增量时提示“模型思考中”） */
  lastDeltaAt: number | null
  /** 该会话运行中工具数（chat-tool-start +1 / chat-tool-done -1；看门狗巡检不误杀执行中任务） */
  toolRunning: number
}

/** 结构化友好错误（对应后端 errors::FriendlyError） */
export interface ChatErrorDetail {
  kind: string
  title: string
  reason: string
  suggestion: string
  retryable: boolean
  statusCode?: number | null
}

/** Agent 工具执行状态（实时卡片） */
export interface ToolRun {
  id: string
  tool: string
  args: string
  status: 'running' | 'done' | 'error'
  output: string
  /** 当前轮次 / 最大轮次（第几轮） */
  round?: number
  total?: number
  /** 风险等级（L0 只读 / L1 写入 / L2 危险） */
  level?: string
  /** 工具一句话说明（悬浮提示） */
  desc?: string
  /** 运行中的流式输出（agent:log 累积；完成后并入 output 展示） */
  liveOutput?: string
  /** 工具开始时间戳（ms，running 态计时用） */
  startedAt?: number
  /** 工具执行耗时（ms，完成后定值） */
  durationMs?: number
}

/** 终端面板条目：工具执行实时记录（右侧栏终端视图） */
export interface TerminalEntry {
  id: string
  tool: string
  args: string
  status: 'running' | 'done' | 'error'
  output: string
  /** 运行中的流式输出（agent:log 累积；完成后并入 output 展示） */
  liveOutput?: string
  startedAt: number
  durationMs?: number
}

/** 构建/部署流式日志行（来自后端 agent:log 事件） */
export interface BuildLogLine {
  /** 稳定自增 id，用作 React key，避免下标 key 在丢弃头部时整列重渲染 */
  id: number
  /** "stdout" | "stderr" | "system" */
  stream: string
  line: string
  ts: number
}

/** 子 Agent 执行状态（并行委派实时卡片） */
export interface AgentRun {
  id: string
  name: string
  model: string
  status: 'running' | 'done' | 'error'
  output: string
}

/** 任务进度清单步骤（计划卡）：Agent 计划列表 + 工具执行联动 */
export interface PlanStep {
  text: string
  status: 'pending' | 'running' | 'done' | 'error'
}

export interface TaskPlan {
  steps: PlanStep[]
  /** running=执行中 / done=正常结束 / error=出错或部分失败 */
  phase: 'running' | 'done' | 'error'
}

/** 会话任务账本展示状态（chat-ledger 事件按会话聚合；切回会话恢复展示） */
export interface TaskLedgerState {
  ledger: TaskLedger | null
  /** 任务是否已结束（true=最终状态：中断保留/完成清空；false=进行中每轮实时刷新） */
  finished: boolean
}

/** 工具权限审核请求（自动审核模式下待用户确认） */
export interface ToolApproval {
  requestId: string
  tool: string
  args: string
}

/** Agent 推送的诊断引导卡片（需用户在 IDE/系统中手动操作的问题） */
export interface DiagnoseCard {
  id: string
  requestId: string
  conversationId: string
  category: 'signing' | 'sdk' | 'dependency' | 'other'
  title: string
  message: string
  action: 'install_deps' | 'open_sdk_manager' | 'open_signing_config' | 'none'
  createdAt: number
}

/** 计划/审查模式待确认的任务计划 */
export interface PendingPlan {
  requestId: string
  conversationId: string
  plan: string
}

/** Agent 提问卡（ask_user 工具，待用户自由回答） */
export interface AskCard {
  requestId: string
  conversationId: string
  question: string
  options: string[]
}

/** 任务结束摘要（ChatGPT 式收尾统计）：耗时 + 工具调用数 + 修改文件数 + token 成本 */
export interface TaskSummary {
  status: 'completed' | 'incomplete'
  durationMs: number
  toolCount: number
  fileCount: number
  /** 本任务累计输入 token（后端持久化到结束消息的 tokens_in） */
  tokensIn: number
  /** 本任务累计输出 token（后端持久化到结束消息的 tokens_out） */
  tokensOut: number
}

/** 项目/文件树/分支切片 */
export interface ProjectSlice {
  projects: Project[]
  currentProject: Project | null
  gitBranches: GitBranchInfo | null
  fileTree: FileTreeNode | null
  indexBuilding: boolean
  /** 文件树懒加载缓存：目录相对路径 -> 该层子项列表 */
  dirCache: Record<string, FileTreeNode[]>
  loading: boolean
  refreshProjects: () => Promise<void>
  openProject: (id: string) => Promise<void>
  addProjectByPath: (path: string) => Promise<Project>
  confirmTrust: (id: string) => Promise<void>
  removeProject: (id: string) => Promise<void>
  refreshGitBranches: () => Promise<void>
  switchBranch: (branch: string) => Promise<string | null>
  loadFileTree: () => Promise<void>
  /** 懒加载：读取单层目录（带缓存），返回该层子项 */
  loadDirChildren: (path: string) => Promise<FileTreeNode[]>
  rebuildIndex: () => Promise<void>
  reset: () => void
}

/** 会话/消息/流式/审批/计划切片 */
export interface ChatSlice {
  conversations: Conversation[]
  currentConversation: Conversation | null
  messages: ChatMessage[]
  /** 会话消息分页：游标（messages[0]）之前是否还有更早消息未加载 */
  olderHasMore: boolean
  /** 正在加载更早的历史消息（向上翻页，防重入） */
  loadingOlder: boolean
  streaming: StreamingState
  /** 多会话并行流式分桶（真源）：conversationId → 流式状态；streaming 为当前会话的派生视图 */
  streamings: Record<string, StreamingState>
  toolRuns: ToolRun[]
  /** 终端面板：工具执行实时记录（新任务开始时清空，结束后保留供查看） */
  terminalEntries: TerminalEntry[]
  /** 构建/部署流式日志（agent:log 事件累积，按行；新任务开始时清空） */
  buildLogs: BuildLogLine[]
  agentRuns: AgentRun[]
  /** 任务进度清单（计划卡）：工具执行联动推进，任务结束后保留展示 */
  plan: TaskPlan | null
  /** 待用户审核的工具调用（自动审核模式） */
  toolApprovals: ToolApproval[]
  /** 各会话待确认项（按 conversationId 聚合）：会话列表角标 + 切回会话恢复弹窗 */
  pendingConfirmations: Record<string, PendingConfirmation[]>
  /** 各会话任务账本（Ledger 协议，按 conversationId 聚合）：实时刷新/中断保留/切回恢复展示 */
  taskLedgers: Record<string, TaskLedgerState>
  /** Agent 推送的诊断引导卡片（签名/SDK/依赖等需用户决策） */
  diagnoseCards: DiagnoseCard[]
  /** 计划/审查模式：待用户确认的任务计划 */
  pendingPlan: PendingPlan | null
  /** 已获用户批准的计划（执行中展示在对话流上方；新任务开始/切会话时清空） */
  approvedPlan: { conversationId: string; plan: string } | null
  /** 任务清单（todo_write 工具维护，任务结束后保留展示，新任务清空、切会话重载） */
  todos: TodoItem[]
  /** Agent 挂起的提问卡（ask_user 工具；5 分钟超时自动关闭） */
  askCard: AskCard | null
  /** 上次任务被停止且未完成（有工具成果无总结）；展示“继续任务”断点续跑按钮 */
  unfinishedConv: { conversationId: string } | null
  /** 会话排队中消息列表（流式运行中提交、任务结束后续跑） */
  queuedList: QueuedMessageInfo[]
  /** 会话搜索关键字（侧栏搜索框，后端 LIKE 匹配标题/首条消息） */
  conversationKeyword: string
  /** 关闭一张诊断卡片 */
  dismissDiagnoseCard: (id: string) => void
  /** 回复 Agent 提问：answer 为空串表示跳过 */
  resolveAskUser: (requestId: string, answer: string) => Promise<void>
  /** 回复计划审查：approved=true 批准执行；false 驳回并可附带修改意见 */
  resolvePlanReview: (requestId: string, approved: boolean, feedback?: string) => Promise<void>
  /** 回复审核结果：true=允许执行 / false=拒绝（可附理由反馈模型）；remember=本会话始终允许该工具 */
  resolveToolApproval: (requestId: string, approved: boolean, remember?: boolean, feedback?: string, scope?: 'session' | 'project') => Promise<void>
  /** 拉取项目内所有会话的待确认项（审批/计划/提问），刷新会话列表角标与恢复数据 */
  refreshPendingConfirmations: () => Promise<void>
  /** 清空终端面板记录 */
  clearTerminal: () => void
  /** 清空构建日志 */
  clearBuildLogs: () => void
  /** 会话搜索：更新关键字并刷新列表 */
  setConversationKeyword: (kw: string) => Promise<void>
  newConversation: (worktree?: NewConversationWorktree) => Promise<void>
  openConversation: (id: string) => Promise<void>
  /** 会话 Fork：从当前会话派生新会话（复制截至 untilMessageId 含该条的消息与事件；缺省全部），完成后切换到新会话 */
  forkCurrentConversation: (untilMessageId?: string) => Promise<void>
  /** 加载更早的历史消息（prepend 到 messages 头部），返回新增条数；无更多或已加载返回 0 */
  loadOlderMessages: (conversationId: string) => Promise<number>
  sendUserMessage: (content: string, options?: ChatOptions, references?: string[], images?: string[]) => Promise<void>
  /** 流式运行中提交消息进入排队：agentOwned=true 发送到 Agent；false 任务结束后自动续跑 */
  queueUserMessage: (content: string, agentOwned: boolean, references?: string[], images?: string[]) => Promise<void>
  /** 编辑已发送的用户消息（更新后刷新列表） */
  editMessage: (messageId: string, content: string) => Promise<void>
  /** 删除单条消息及其之后的所有消息（刷新列表） */
  removeMessage: (messageId: string) => Promise<void>
  stopGeneration: () => Promise<void>
  /** 停止当前正在执行的工具（不终止整个任务）：模型拿到中断反馈后继续生成结论 */
  stopCurrentTool: () => Promise<void>
  /** 重新生成：messageId 指定时从该 user 消息分支重生成（丢弃其后主线并归档旧回复） */
  regenerateLast: (options?: ChatOptions, messageId?: string) => Promise<void>
  /** 刷新排队中消息列表 */
  refreshQueued: (conversationId: string) => Promise<void>
  /** 移除排队中的一条消息（不再续跑） */
  removeQueued: (messageId: string) => Promise<void>
  renameConversation: (id: string, title: string) => Promise<void>
  deleteConversation: (id: string) => Promise<void>
  pinConversation: (id: string, pinned: boolean) => Promise<void>
  archiveConversation: (id: string, archived: boolean) => Promise<void>
  /** 任务回滚（dryRun=true 仅预览）：git 硬重置到任务起点前最后一次提交 */
  rollbackTask: (conversationId: string, dryRun: boolean) => Promise<RollbackInfo>
  /** 会话快照列表（时间轴，最新在前；当前可见末端对应 is_current=true） */
  snapshots: SnapshotInfo[]
  /** 快照列表加载中（防抖重复打开） */
  loadingSnapshots: boolean
  loadSnapshots: (conversationId: string) => Promise<void>
  /** 恢复会话到历史快照点（时间旅行）：归档后续消息、重现旧分支、账本写回 */
  restoreToSnapshot: (conversationId: string, snapshotId: string) => Promise<RestoreSnapshotResult>
}

/** 记忆/统计/反馈/版本切片 */
export interface MemorySlice {
  /** 项目记忆（右侧面板管理，注入 system_prompt） */
  memories: ProjectMemory[]
  /** 工具调用统计（按工具聚合） */
  toolStats: ToolStat[]
  /** 工具 token 消耗排行（[69]：request_logs.tool_name 按工具聚合） */
  toolTokenStats: ToolTokenStat[]
  loadToolTokenStats: () => Promise<void>
  /** 消息反馈：message_id -> 反馈记录 */
  feedbackMap: Record<string, MessageFeedback>
  /** 回复版本：user_message_id -> 旧版本列表（重新生成保留） */
  versionMap: Record<string, MessageVersion[]>
  /** 记忆总结草稿（待确认弹窗） */
  memoryDraft: MemoryDraft | null
  /** 记忆总结进行中 */
  summarizing: boolean
  /** 当前会话 token/成本累计（标题下展示） */
  tokenStats: ConversationCostStats | null
  /** 最近任务列表（概览面板展示，task_runs 表） */
  recentRuns: TaskRun[]
  /** 最近一次任务结束摘要（消息流收尾展示；切换会话/新任务时清空） */
  lastTaskSummary: TaskSummary | null
  /** 加载项目记忆列表 */
  loadMemories: () => Promise<void>
  /** 新增/更新记忆（saveMemory 后端自动区分 id 有无） */
  saveMemory: (input: { id?: string; category: string; title: string; content: string }) => Promise<void>
  /** 删除记忆 */
  deleteMemory: (id: string) => Promise<void>
  /** 启用/禁用记忆 */
  setMemoryEnabled: (id: string, enabled: boolean) => Promise<void>
  /** 加载工具调用统计 */
  loadToolStats: () => Promise<void>
  /** 加载会话全部反馈 */
  loadFeedback: (conversationId: string) => Promise<void>
  /** 点赞/点踩（feedback=neutral 取消） */
  rateMessage: (messageId: string, feedback: 'like' | 'dislike' | 'neutral', reason?: string) => Promise<void>
  /** 加载会话全部回复版本 */
  loadVersions: (conversationId: string) => Promise<void>
  /** 请求 LLM 生成记忆总结草稿（不落库，用户确认后保存） */
  summarizeMemory: (conversationId: string) => Promise<MemoryDraft | null>
  /** 会话消息导出为 Markdown 文本 */
  exportConversation: (format: 'md' | 'txt' | 'html') => string
  /** 加载最近任务列表 */
  loadRecentRuns: () => Promise<void>
  /** 加载会话 token/成本累计统计 */
  loadTokenStats: (conversationId: string) => Promise<void>
}

/** 组合状态：ProjectSlice + ChatSlice + MemorySlice */
export interface ProjectState extends ProjectSlice, ChatSlice, MemorySlice {}
