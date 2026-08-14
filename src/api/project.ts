import { invoke } from '@tauri-apps/api/core'

export interface Project {
  id: string
  name: string
  path: string
  kind: string
  trusted: boolean
  default_provider_id: string | null
  default_model_id: string | null
  index_state: string
  rules: string | null
  last_opened_at: number | null
  created_at: number
  /** 绑定的 worktree 目录（Agent 任务在此目录执行；null=主仓库） */
  worktree_path: string | null
  /** 工作区下识别到的鸿蒙子工程相对路径列表（JSON 字符串，正斜杠）；null 表示未扫描 */
  harmony_subprojects: string | null
  /** 工作区下识别到的各类子工程模块（JSON 字符串，WorkspaceModule[]） */
  workspace_modules: string | null
  /** 会话"鸿蒙主工程"（混合工作区中实际进行鸿蒙开发的子工程：相对项目根或绝对路径）；空=用项目根本身 */
  harmony_project_path: string | null
}

export interface Conversation {
  id: string
  project_id: string
  title: string
  provider_id: string | null
  model_id: string | null
  system_prompt_version: number | null
  is_pinned: boolean
  archived: boolean
  created_at: number
  updated_at: number
}

export interface ChatMessage {
  id: string
  conversation_id: string
  role: string
  content: string
  references_json: string | null
  model: string | null
  tokens_in: number | null
  tokens_out: number | null
  created_at: number
  /** 思考过程（DeepSeek 等推理模型的 reasoning_content，无则 null） */
  reasoning: string | null
  /** 排队状态：1=流式运行中提交、尚未提交给模型 */
  queued: number
  /** 挂起类型：1=发送到 Agent（任务内安全点并入）；0=普通排队（任务结束后自动续跑） */
  agent_owned: number
  /** 本回复修改的文件列表 JSON（相对路径数组，无则 null） */
  modified_files_json: string | null
  /** 本次任务耗时（ms，assistant 消息专用；回复上方展示用时） */
  duration_ms: number | null
}

export interface ProjectInspect {
  path: string
  name: string
  is_harmony: boolean
  file_count: number
  has_git: boolean
  already_added: boolean
  app_name: string | null
  bundle_name: string | null
}

export interface GitBranchInfo {
  has_git: boolean
  current: string | null
  branches: string[]
  error: string | null
}

/** 对话级模型设置（随消息发送，覆盖 Provider/模型默认值） */
export interface ChatOptions {
  model_id?: string
  use_proxy?: boolean
  temperature?: number
  top_p?: number
  max_tokens?: number
  /** 推理深度（OpenAI 兼容协议；low/medium/high，部分推理模型支持） */
  reasoning_effort?: string
  /** 子 Agent 默认模型记录 ID（跨 Provider；缺省跟随主模型） */
  sub_model_id?: string
  /** 子 Agent 最大并发数（缺省 3） */
  max_concurrency?: number
  /** 工具权限模式：ask=每次确认；auto=分级审核；allow_all=完全放任（缺省）；first_write=首次写文件确认、本任务后续放行 */
  tool_approval?: 'ask' | 'auto' | 'allow_all' | 'first_write'
  /** 计划/审查模式：true 时 Agent 先出任务计划，用户确认后才执行工具 */
  plan_mode?: boolean
  /** 排队消息提交方式：true=一起提交（任务结束后合并全部排队消息为一条）；缺省=逐个提交 */
  batch_queued?: boolean
  /** 协议端点选择（openai | anthropic | gemini；Provider 配置多端点时生效，如 DeepSeek） */
  protocol?: string
  /** 原生工具调用（function calling）：true 时 OpenAI 兼容协议注入 tools、解析原生 tool_calls（与文本标记并行）；缺省 false 保持纯文本标记协议 */
  native_tools?: boolean
}

/** 回复工具权限审核请求（自动审核模式下由确认弹窗调用）；remember=始终允许该工具，feedback=拒绝理由，scope=project 时持久化到项目白名单 */
export const resolveToolApproval = (
  requestId: string,
  approved: boolean,
  remember?: boolean,
  feedback?: string,
  scope?: 'session' | 'project',
) => invoke<void>('resolve_tool_approval', { requestId, approved, remember, feedback, scope })

/** 回复诊断引导卡片结果（完成操作或稍后关闭后调用，唤醒等待中的 Agent） */
export const resolveDiagnoseCard = (
  requestId: string,
  completed: boolean,
  note?: string,
) => invoke<void>('resolve_diagnose_card', { requestId, completed, note })

/** 回复计划审查结果（计划模式下由计划卡片调用） */
export const resolvePlanReview = (
  conversationId: string,
  requestId: string,
  approved: boolean,
  feedback?: string,
) =>
  invoke<void>('resolve_plan_review', {
    conversationId,
    requestId,
    approved,
    feedback,
  })

/** 任务清单条目（todo_write 工具维护，agent:todo 事件推送） */
export interface TodoItem {
  id: string
  content: string
  status: 'pending' | 'in_progress' | 'done'
}

/** 读取会话当前任务清单（切换会话/刷新后恢复展示） */
export const getTodos = (conversationId: string) =>
  invoke<TodoItem[]>('get_todos', { conversationId })

/** Agent 挂起的提问（ask_user 工具推送） */
export interface PendingAsk {
  conversation_id: string
  request_id: string
  question: string
  options: string[]
}

/** 查询会话内挂起的提问（切回会话时恢复提问卡） */
export const getAsk = (conversationId: string) =>
  invoke<PendingAsk | null>('get_ask', { conversationId })

/** 回复 Agent 的提问（answer 为空串表示跳过） */
export const resolveAskUser = (requestId: string, answer: string) =>
  invoke<void>('resolve_ask_user', { requestId, answer })

/** 会话上下文状态（输入区上下文可视条：消息数/摘要状态） */
export interface ConversationContextInfo {
  conversation_id: string
  message_count: number
  has_summary: boolean
  /** 历史消息估算 token（字符数/2 保守估算） */
  estimated_tokens: number
  /** 模型上下文窗口预算 */
  context_limit: number
  /** 本会话累计输入 token */
  total_tokens_in: number
  /** 本会话累计输出 token */
  total_tokens_out: number
  /** 本会话累计 assistant 回复耗时（ms） */
  total_duration_ms: number
  /** 事件日志条数（session_events 只追加审计日志） */
  event_count: number
}

export const getConversationContext = (conversationId: string) =>
  invoke<ConversationContextInfo>('conversation_context', { conversationId })

/** 会话事件日志条目（回放视图） */
export interface SessionEvent {
  id: number
  conversation_id: string
  seq: number
  event_type: string
  payload: Record<string, unknown>
  created_at: number
}

/** 事件 → 消息历史投影（回放视角） */
export interface DerivedMessage {
  role: 'user' | 'assistant' | 'tool'
  content: string
  tool_name: string | null
  created_at: number
}

/** 会话事件日志视图（读取侧：事件流 + 投影 + 总数） */
export interface SessionEventsView {
  events: SessionEvent[]
  messages: DerivedMessage[]
  total: number
}

export const getSessionEvents = (conversationId: string) =>
  invoke<SessionEventsView>('get_session_events', { conversationId })

export interface FileTreeNode {
  name: string
  path: string
  type: 'dir' | 'file'
  /** 文件字节数（目录无此字段） */
  size?: number
  children?: FileTreeNode[]
}

export const listProjects = () => invoke<Project[]>('list_projects')

export const inspectProject = (path: string) => invoke<ProjectInspect>('inspect_project', { path })

export const addProject = (path: string) => invoke<Project>('add_project', { path })

/** 工作区模块类型（与后端 ModuleKind 一一对应，小写） */
export type ModuleKind =
  | 'harmony' | 'vue' | 'react' | 'angular' | 'node'
  | 'java' | 'kotlin' | 'go' | 'python' | 'rust' | 'dotnet'
  | 'flutter' | 'android' | 'ios' | 'html' | 'php' | 'ruby' | 'cpp' | 'unknown'

/** 模块类型展示名（中文） */
export const MODULE_KIND_LABELS: Record<ModuleKind, string> = {
  harmony: 'HarmonyOS',
  vue: 'Vue',
  react: 'React',
  angular: 'Angular',
  node: 'Node.js',
  java: 'Java',
  kotlin: 'Kotlin',
  go: 'Go',
  python: 'Python',
  rust: 'Rust',
  dotnet: '.NET',
  flutter: 'Flutter',
  android: 'Android',
  ios: 'iOS',
  html: '静态站点',
  php: 'PHP',
  ruby: 'Ruby',
  cpp: 'C/C++',
  unknown: '未分类',
}

/** 可手动绑定的全部模块类型（下拉选项顺序） */
export const MODULE_KINDS: ModuleKind[] = [
  'harmony', 'vue', 'react', 'angular', 'node',
  'java', 'kotlin', 'go', 'python', 'rust', 'dotnet',
  'flutter', 'android', 'ios', 'html', 'php', 'ruby', 'cpp', 'unknown',
]

/** 一个工作区模块（子工程） */
export interface WorkspaceModule {
  rel_path: string
  kind: ModuleKind
  name: string
  /** 用户手动绑定的模块（重新扫描时保留，不被自动分类覆盖） */
  manual?: boolean
}

/** 扫描预览结果：模块 + 该子目录探测信息 */
export type ScannedModuleEntry = WorkspaceModule & { inspect: ProjectInspect }

/** 预览扫描：列出所选目录下识别到的所有模块（不落库） */
export const scanWorkspaceModules = (path: string) =>
  invoke<ScannedModuleEntry[]>('scan_workspace_modules', { path })

/** 重新扫描已添加项目的工作区模块（保留手动绑定项） */
export const rescanWorkspaceModules = (projectId: string) =>
  invoke<Project>('rescan_workspace_modules', { projectId })

/** 手动设置工作区模块列表（增删改、修改类型） */
export const setWorkspaceModules = (projectId: string, modules: WorkspaceModule[]) =>
  invoke<Project>('set_workspace_modules', { projectId, modules })
  
  /** 会话"鸿蒙主工程"解析结果 */
  export interface HarmonyRootInfo {
    /** 解析后的鸿蒙主工程根（绝对路径；未配置且无唯一候选时为项目根） */
    root: string
    /** 已配置的鸿蒙主工程（相对项目根或绝对路径）；null=未配置 */
    configured: string | null
    /** 候选鸿蒙子工程（绝对路径列表，不含项目根本身） */
    candidates: string[]
    /** 是否自动兜底（未配置但工作区仅一个鸿蒙模块） */
    auto: boolean
  }
  
  /** 查询项目的"鸿蒙主工程"解析结果 */
  export const getHarmonyRoot = (projectId: string) =>
    invoke<HarmonyRootInfo>('get_harmony_root', { projectId })
  
  /** 设置会话"鸿蒙主工程"（空串=清除，回退项目根本身）；返回更新后的项目 */
  export const setHarmonyProjectPath = (projectId: string, path: string) =>
    invoke<Project>('set_harmony_project_path', { projectId, path })

/** 解析 project.workspace_modules JSON 字符串为数组 */
export const parseWorkspaceModules = (json: string | null | undefined): WorkspaceModule[] => {
  if (!json) return []
  try {
    const v = JSON.parse(json)
    return Array.isArray(v) ? (v as WorkspaceModule[]) : []
  } catch {
    return []
  }
}

/** 兼容旧字段：从 workspace_modules 派生鸿蒙子工程相对路径 */
export const parseHarmonySubprojects = (project?: { workspace_modules?: string | null } | null): string[] => {
  if (!project?.workspace_modules) return []
  return parseWorkspaceModules(project.workspace_modules)
    .filter((m) => m.kind === 'harmony')
    .map((m) => m.rel_path)
}

/** 项目整体类型的展示信息（综合根 kind 与工作区模块动态推导，而非只看一次性写入的 kind） */
export interface ProjectTypeBadge {
  /** 展示文案 */
  label: string
  /** 模块类型；multi=多模块工作区，generic=普通目录 */
  kind: ModuleKind | 'multi' | 'generic'
}

/**
 * 根据项目根 kind 与已识别的工作区模块推导顶部/信息卡显示的类型。
 * 优先级：已扫描到的模块数量 > 根 kind。
 * 这样即使根目录本身是鸿蒙结构（kind=harmony），只要识别出多个子模块，
 * 仍显示为"多模块工作区"，符合实际情况。
 * - 多个子模块 → 多模块工作区
 * - 仅 1 个子模块 → 该子模块类型
 * - 0 个子模块 → 回退到根 kind（harmony→HarmonyOS，global→Global，其余→普通目录）
 */
export const deriveProjectType = (project?: {
  kind?: string | null
  workspace_modules?: string | null
} | null): ProjectTypeBadge => {
  if (project?.kind === 'global') {
    return { label: 'Global', kind: 'generic' }
  }
  const mods = parseWorkspaceModules(project?.workspace_modules)
  if (mods.length > 1) {
    return { label: `多模块工作区（${mods.length}）`, kind: 'multi' }
  }
  if (mods.length === 1) {
    const k = mods[0].kind
    return { label: MODULE_KIND_LABELS[k], kind: k }
  }
  if (project?.kind === 'harmony') {
    return { label: 'HarmonyOS', kind: 'harmony' }
  }
  return { label: '普通目录', kind: 'generic' }
}

/** 类型徽章配色（与 AddProjectDialog 保持一致） */
export const projectTypeBadgeClass = (kind: ProjectTypeBadge['kind']): string => {
  switch (kind) {
    case 'harmony': return 'bg-[#e6f7ef] text-[#1a9b5c] dark:bg-[#1a9b5c]/15 dark:text-[#4ade80]'
    case 'vue': return 'bg-[#e6fbf3] text-[#42b883] dark:bg-[#42b883]/15 dark:text-[#4ade80]'
    case 'react': return 'bg-[#e6f4fe] text-[#149eca] dark:bg-[#149eca]/15 dark:text-[#61dafb]'
    case 'angular': return 'bg-[#ffe9e9] text-[#dd0031] dark:bg-[#dd0031]/15 dark:text-[#ff6b6b]'
    case 'node': return 'bg-[#e9f9e3] text-[#5fa04e] dark:bg-[#5fa04e]/15 dark:text-[#86efac]'
    case 'java': case 'kotlin': return 'bg-[#fff3e0] text-[#e76f00] dark:bg-[#e76f00]/15 dark:text-[#fbbf24]'
    case 'go': return 'bg-[#e3f2fd] text-[#00add8] dark:bg-[#00add8]/15 dark:text-[#38bdf8]'
    case 'python': return 'bg-[#fef3c7] text-[#d97706] dark:bg-[#d97706]/15 dark:text-[#fbbf24]'
    case 'rust': return 'bg-[#fce7e7] text-[#ce422b] dark:bg-[#ce422b]/15 dark:text-[#f87171]'
    case 'dotnet': return 'bg-[#f3e8ff] text-[#7c3aed] dark:bg-[#7c3aed]/15 dark:text-[#c4b5fd]'
    case 'flutter': return 'bg-[#e7f0ff] text-[#02569b] dark:bg-[#02569b]/15 dark:text-[#60a5fa]'
    case 'android': return 'bg-[#e3f9e5] text-[#3ddc84] dark:bg-[#3ddc84]/15 dark:text-[#4ade80]'
    case 'ios': return 'bg-[var(--bg-hover)] text-[var(--text-secondary)]'
    case 'html': return 'bg-[#fff0e6] text-[#e34c26] dark:bg-[#e34c26]/15 dark:text-[#fb923c]'
    case 'php': return 'bg-[#f0e9ff] text-[#777bb4] dark:bg-[#777bb4]/15 dark:text-[#c4b5fd]'
    case 'ruby': return 'bg-[#ffe9ec] text-[#cc342d] dark:bg-[#cc342d]/15 dark:text-[#f87171]'
    case 'cpp': return 'bg-[#e6f0ff] text-[#00599c] dark:bg-[#00599c]/15 dark:text-[#60a5fa]'
    case 'multi': return 'bg-[var(--accent)]/15 text-[var(--accent)]'
    case 'unknown': return 'bg-[var(--bg-hover)] text-[var(--text-muted)]'
    default: return 'bg-[var(--warning)]/15 text-[var(--warning)]'
  }
}

export const trustProject = (id: string) => invoke<Project>('trust_project', { id })

export const deleteProject = (id: string) => invoke<void>('delete_project', { id })

export interface ScopedCounts { mcp: number; skills: number }
/** 各项目的项目级专属 MCP/技能数量 */
export const projectScopedCounts = () =>
  invoke<Record<string, ScopedCounts>>('project_scoped_counts')

export const listConversations = (projectId: string, includeArchived = false, keyword = '') =>
  invoke<Conversation[]>('list_conversations', { projectId, includeArchived, keyword })

export const createConversation = (projectId: string, title?: string) =>
  invoke<Conversation>('create_conversation', { projectId, title })

export const listMessages = (conversationId: string) =>
  invoke<ChatMessage[]>('list_messages', { conversationId })

/** 消息全文搜索命中 */
export interface MessageSearchHit {
  conversation_id: string
  conversation_title: string
  message_id: string
  role: string
  created_at: number
  snippet: string
  match_start: number
}

/** 在项目内（或指定会话）全文检索消息内容，返回命中片段 */
export const searchMessages = (projectId: string, query: string, conversationId?: string) =>
  invoke<MessageSearchHit[]>('search_messages', { projectId, query, conversationId: conversationId ?? null })

export const sendMessage = (conversationId: string, content: string) =>
  invoke<ChatMessage>('send_message', { conversationId, content })

export const streamChat = (
  conversationId: string,
  content: string,
  options?: ChatOptions,
  regenerate = false,
  references?: string[],
  images?: string[],
  regenerateUserId?: string,
) => invoke<void>('stream_chat', { conversationId, content, options, regenerate, references, images, regenerateUserId })

/** 运行中提交消息进入排队：agentOwned=true 由 Agent 安全点并入当前任务；false 任务结束后自动续跑 */
export const queueMessage = (conversationId: string, content: string, agentOwned: boolean, references?: string[], images?: string[]) =>
  invoke<ChatMessage>('queue_message', { conversationId, content, agentOwned, references, images })

/** 会话排队中消息（前端“排队中”条展示） */
export interface QueuedMessageInfo {
  id: string
  content: string
  agent_owned: boolean
  created_at: number
}

/** 查询会话排队中消息列表 */
export const listQueuedMessages = (conversationId: string) =>
  invoke<QueuedMessageInfo[]>('list_queued_messages', { conversationId })

/** 移除会话排队中的一条消息（不再续跑） */
export const removeQueuedMessage = (conversationId: string, messageId: string) =>
  invoke<void>('remove_queued_message', { conversationId, messageId })

/** 项目审批白名单条目 */
export interface WhitelistEntry {
  tool: string
  created_at: number
}

/** 查询项目的工具审批白名单 */
export const listToolWhitelist = (projectId: string) =>
  invoke<WhitelistEntry[]>('list_tool_whitelist', { projectId })

/** 移除项目审批白名单中的一条记录 */
export const removeToolWhitelist = (projectId: string, tool: string) =>
  invoke<void>('remove_tool_whitelist', { projectId, tool })

/** 编辑已发送的用户消息内容 */
export const updateMessage = (messageId: string, content: string) =>
  invoke<void>('update_message', { messageId, content })

/** 删除单条消息及其之后的所有消息 */
export const deleteMessage = (messageId: string) => invoke<number>('delete_message', { messageId })

/** 停止当前流式生成（后端在安全点退出，部分内容会入库） */
export const stopChat = (conversationId: string) => invoke<void>('stop_chat', { conversationId })

/** 停止当前正在执行的工具（不终止整个任务）：强杀子进程，模型拿到中断反馈后继续生成结论 */
export const stopTool = (conversationId: string) => invoke<void>('stop_tool', { conversationId })

export const renameConversation = (id: string, title: string) =>
  invoke<void>('rename_conversation', { id, title })

/** 置顶 / 取消置顶 */
export const pinConversation = (id: string, pinned: boolean) =>
  invoke<Conversation>('update_conversation', { id, isPinned: pinned, archived: null, modelId: null })

/** 归档/取消归档会话 */
export const archiveConversation = (id: string, archived: boolean) =>
  invoke<Conversation>('update_conversation', { id, isPinned: null, archived, modelId: null })

/** 会话绑定模型（空串清除绑定；上下文可视条按会话模型查 context_limit） */
export const setConversationModel = (id: string, modelId: string) =>
  invoke<Conversation>('update_conversation', { id, isPinned: null, archived: null, modelId })

export const deleteConversation = (id: string) =>
  invoke<void>('delete_conversation', { id })

export const getGitBranches = (projectId: string) =>
  invoke<GitBranchInfo>('get_git_branches', { projectId })

export const switchGitBranch = (projectId: string, branch: string) =>
  invoke<GitBranchInfo>('switch_git_branch', { projectId, branch })

export const buildProjectIndex = (projectId: string) =>
  invoke<FileTreeNode>('build_project_index', { projectId })

export const getProjectFileTree = (projectId: string) =>
  invoke<FileTreeNode | null>('get_project_file_tree', { projectId })

/** 读取单层目录内容（文件树懒加载：根目录 → 展开时逐级按需请求） */
export const listProjectDir = (projectId: string, path: string) =>
  invoke<FileTreeNode[]>('list_project_dir', { projectId, path })

/** 读取项目内文件文本内容（UTF-8，≤5MB），供预览面板使用 */
export const readProjectFile = (projectId: string, path: string) =>
  invoke<string>('read_project_file', { projectId, path })

/* ============ 项目记忆（Memory） ============ */

export interface ProjectMemory {
  id: string
  project_id: string
  /** general|code|build|deploy|decision|pitfall */
  category: string
  title: string
  content: string
  enabled: boolean
  created_at: number
  updated_at: number
}

export const listMemories = (projectId: string) =>
  invoke<ProjectMemory[]>('list_memories', { projectId })

/** 保存记忆（id 为空 = 新增，否则更新） */
export const saveMemory = (input: { id?: string; project_id: string; category: string; title: string; content: string }) =>
  invoke<ProjectMemory>('save_memory', { input })

export const deleteMemory = (id: string) => invoke<void>('delete_memory', { id })

export const setMemoryEnabled = (id: string, enabled: boolean) =>
  invoke<void>('set_memory_enabled', { id, enabled })

/* ============ 工具调用统计（Evaluation） ============ */

export interface ToolStat {
  tool_name: string
  call_count: number
  success_count: number
  fail_count: number
  avg_duration_ms: number | null
  last_called_at: number | null
}

export const listToolStats = (projectId: string) =>
  invoke<ToolStat[]>('list_tool_stats', { projectId })

/* ============ 消息反馈（点赞/点踩） ============ */

export interface MessageFeedback {
  id: string
  message_id: string
  conversation_id: string
  /** like | dislike */
  feedback: string
  reason: string | null
  comment: string | null
  created_at: number
}

/** 保存反馈（feedback=neutral 时删除已有反馈；dislike 可带 reason/comment） */
export const saveMessageFeedback = (input: {
  messageId: string
  conversationId: string
  feedback: 'like' | 'dislike' | 'neutral'
  reason?: string
  comment?: string
}) => invoke<MessageFeedback | null>('save_message_feedback', { input })

export const listMessageFeedback = (conversationId: string) =>
  invoke<MessageFeedback[]>('list_message_feedback', { conversationId })

/* ============ 回复版本（重新生成保留旧版 + diff） ============ */

export interface MessageVersion {
  id: string
  conversation_id: string
  user_message_id: string
  content: string
  reasoning: string | null
  model: string | null
  created_at: number
}

export const listMessageVersions = (conversationId: string) =>
  invoke<MessageVersion[]>('list_message_versions', { conversationId })

/* ============ 记忆自动总结（草稿） ============ */

export interface MemoryDraft {
  title: string
  category: string
  content: string
}

/** 由 LLM 提取会话要点，返回待确认的记忆草稿（不落库） */
export const summarizeMemory = (conversationId: string) =>
  invoke<MemoryDraft>('summarize_memory', { conversationId })

/** 手动压缩会话历史：用经济模型把较早历史总结为结构化摘要，返回新摘要 */
export const compactConversation = (conversationId: string, keep?: number) =>
  invoke<string>('compact_conversation', { conversationId, keep })

/* ============ 会话 token/成本统计 + 任务回滚 + Rules 指令 ============ */

/** 会话内 token/成本累计（标题下展示） */
export interface ConversationCostStats {
  total_in: number
  total_out: number
  cost_cny: number
  messages_count: number
}

export const getConversationCostStats = (conversationId: string) =>
  invoke<ConversationCostStats>('conversation_cost_stats', { conversationId })

/** 任务回滚目标信息（dryRun=true 时仅预览） */
export interface RollbackInfo {
  commit: string
  commit_date: string
  changed: number
  untracked: number
  is_repo: boolean
}

export const rollbackConversation = (conversationId: string, dryRun: boolean) =>
  invoke<RollbackInfo>('rollback_conversation', { conversationId, dryRun })

/** 读取全局指令（未配置返回空串） */
export const getGlobalRules = () => invoke<string>('get_global_rules')

/** 保存全局指令（传空串即清空） */
export const setGlobalRules = (rules: string) => invoke<void>('set_global_rules', { rules })

/** 保存项目级指令（覆盖写入 projects.rules；传空串即清空） */
export const updateProjectRules = (projectId: string, rules: string) =>
  invoke<void>('update_project_rules', { projectId, rules })
