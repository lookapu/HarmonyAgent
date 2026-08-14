import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { useTranslation } from 'react-i18next'
import { listen } from '@tauri-apps/api/event'
import { watch } from '@tauri-apps/plugin-fs'
import { open as shellOpen } from '@tauri-apps/plugin-shell'
import { useProjectStore, type ToolRun } from '../stores/projectStore'
import { useThemeStore } from '../stores/themeStore'
import {
  inspectProject,
  listConversations,
  type ProjectInspect,
  type ChatOptions,
  type ChatMessage,
  type MemoryDraft,
  getGlobalRules,
  setGlobalRules,
  updateProjectRules,
  compactConversation,
  getConversationContext,
  type ConversationContextInfo,
  searchMessages,
  type MessageSearchHit,
  rescanWorkspaceModules,
  setWorkspaceModules,
  parseWorkspaceModules,
  MODULE_KINDS,
  MODULE_KIND_LABELS,
  deriveProjectType,
  projectTypeBadgeClass,
  type WorkspaceModule,
  type ModuleKind,
  listToolWhitelist,
  removeToolWhitelist,
  type WhitelistEntry,
  resolveDiagnoseCard,
  setConversationModel,
  projectScopedCounts,
  getHarmonyRoot,
} from '../api/project'
import { sendNotification } from '../api/desktop'
import { listProviders, listProviderModels, type ProviderModel } from '../api/provider'
import { openTerminal } from '../api/terminal'
import { getHarmonyEnv } from '../api/harmonyEnv'
import { saveKnowledgeFromText } from '../api/knowledge'
import {
  runOhpmInstall,
  type AnalyzedBuildError,
} from '../api/harmonyAnalyze'
import { warmupSymbolIndex } from '../api/symbols'
import Icon, { type IconName } from '../icons/Icon'
import AddProjectDialog from '../components/AddProjectDialog'
import TrustDialog from '../components/TrustDialog'
import FileTreePanel from '../components/FileTreePanel'
import GitPanel from '../components/GitPanel'
import Markdown from '../components/Markdown'
import { toPinyinInitials, toPinyinFull } from '../utils/pinyin'
import {
  ThinkingBlock,
  ThumbUpIcon,
  ThumbDownIcon,
  ModifiedFilesCard,
  StreamingMessage,
  ErrorCard,
  EmptyState,
  ChatEmptyState,
} from '../chat/components/messageBlocks'
import { BranchSelector, ModelSettingsPopover, PlanCard, TaskOpsBadge } from '../chat/components/plan'
import { ToolRunGroup } from '../chat/components/toolRuns'
import { FeedbackDialog, VersionDiffDialog, MemoryDraftDialog, EditMessageDialog, RulesDialog } from '../chat/components/dialogs'
import { OverviewRow, OverviewGitSummary, MemoriesPanel, ToolStatsPanel, PreviewPanel, TerminalPanel } from '../chat/components/panels'
import { DevicesPanel, AnalyzePanel, SymbolsPanel } from '../chat/components/devicePanels'
import { fmtElapsed, restoreSelectionRange, sanitizeToolMarkers } from '../chat/chatUtils'

/** 斜杠快捷指令清单：输入 / 触发，插入预置 prompt；action 触发额外行为（plan=开启计划模式，compact=手动压缩历史） */
function getSlashCommands(t: (key: string) => string): { id: string; icon: IconName; title: string; prompt: string; action?: 'plan' | 'compact' }[] {
  return [
    { id: 'build', icon: 'bolt', title: t('home.slashBuild'), prompt: t('home.quickBuildPrompt') },
    { id: 'deploy', icon: 'devices', title: t('home.slashDeploy'), prompt: t('home.quickDeployPrompt') },
    { id: 'page', icon: 'add-circle', title: t('home.slashPage'), prompt: t('home.quickPagePrompt') },
    { id: 'explain', icon: 'search', title: t('home.slashExplain'), prompt: t('home.slashExplainPrompt') },
    { id: 'review', icon: 'check', title: t('home.slashReview'), prompt: t('home.slashReviewPrompt') },
    { id: 'fix', icon: 'refresh', title: t('home.slashFix'), prompt: t('home.slashFixPrompt') },
    { id: 'test', icon: 'package', title: t('home.slashTest'), prompt: t('home.slashTestPrompt') },
    { id: 'refactor', icon: 'spark', title: t('home.slashRefactor'), prompt: t('home.slashRefactorPrompt') },
    { id: 'plan', icon: 'lightbulb', title: t('home.slashPlan'), prompt: t('home.slashPlanPrompt'), action: 'plan' },
    { id: 'continue', icon: 'arrow-down', title: t('home.slashContinue'), prompt: t('home.continuePrompt') },
    { id: 'summary', icon: 'receipt', title: t('home.slashSummary'), prompt: t('home.slashSummaryPrompt') },
    { id: 'compact', icon: 'archive', title: t('home.slashCompact'), prompt: '', action: 'compact' },
  ]
}

/** 图片压缩：长边缩到 1568（与 OpenAI 高细节档同级，界面文字仍清晰可读），JPEG 质量 0.9；
 *  小图/GIF 原样返回；带透明的 PNG（设计稿常见）保持 PNG 输出；任何异常兜底返回原图。 */
function compressImage(dataUrl: string): Promise<string> {
  return new Promise((resolve) => {
    if (dataUrl.startsWith('data:image/gif')) { resolve(dataUrl); return }
    const img = new Image()
    img.onload = () => {
      try {
        const w = img.naturalWidth
        const h = img.naturalHeight
        // 小图（长边已达标且体积小）直接原样，避免 canvas 转换精度损失
        if (Math.max(w, h) <= 1568 && dataUrl.length < 300000) { resolve(dataUrl); return }
        const scale = Math.min(1, 1568 / Math.max(w, h))
        const cw = Math.max(1, Math.round(w * scale))
        const ch = Math.max(1, Math.round(h * scale))
        const canvas = document.createElement('canvas')
        canvas.width = cw
        canvas.height = ch
        const ctx = canvas.getContext('2d')
        if (!ctx) { resolve(dataUrl); return }
        ctx.drawImage(img, 0, 0, cw, ch)
        // 透明像素检测：带透明的 PNG 保持 PNG，否则 JPEG 更省体积
        let hasAlpha = false
        if (dataUrl.includes('image/png')) {
          const d = ctx.getImageData(0, 0, cw, ch).data
          for (let i = 3; i < d.length; i += 4) {
            if (d[i] < 255) { hasAlpha = true; break }
          }
        }
        resolve(canvas.toDataURL(hasAlpha ? 'image/png' : 'image/jpeg', 0.9))
      } catch {
        resolve(dataUrl)
      }
    }
    img.onerror = () => resolve(dataUrl)
    img.src = dataUrl
  })
}

export default function Home() {
  const { t } = useTranslation()
  const navigate = useNavigate()

  const {
    projects,
    currentProject,
    conversations,
    currentConversation,
    messages,
    streaming,
    toolRuns,
    agentRuns,
    plan,
    toolApprovals,
    resolveToolApproval,
    diagnoseCards,
    dismissDiagnoseCard,
    pendingPlan,
    resolvePlanReview,
    approvedPlan,
    unfinishedConv,
    todos,
    askCard,
    resolveAskUser,
    refreshProjects,
    openProject,
    addProjectByPath,
    confirmTrust,
    removeProject,
    newConversation,
    openConversation,
    sendUserMessage,
    stopGeneration,
    stopCurrentTool,
    regenerateLast,
    renameConversation,
    deleteConversation,
    pinConversation,
    archiveConversation,
    gitBranches,
    switchBranch,
    fileTree,
    indexBuilding,
    dirCache,
    loadDirChildren,
    rebuildIndex,
    memories,
    toolStats,
    saveMemory,
    deleteMemory,
    setMemoryEnabled,
    loadMemories,
    loadToolStats,
    rateMessage,
    summarizing,
    queueUserMessage,
    editMessage,
    removeMessage,
    tokenStats,
    rollbackTask,
    setConversationKeyword,
    recentRuns,
    loadRecentRuns,
    terminalEntries,
    clearTerminal,
    buildLogs,
    clearBuildLogs,
    lastTaskSummary,
    queuedList,
    refreshQueued,
    removeQueued,
  } = useProjectStore()
  const theme = useThemeStore((s) => s.theme)
  const toggleTheme = useThemeStore((s) => s.toggle)

  const [showAddDialog, setShowAddDialog] = useState(false)
  const [pendingTrust, setPendingTrust] = useState<{ projectId: string; inspect: ProjectInspect } | null>(null)
  const [trustBusy, setTrustBusy] = useState(false)
  const [showRightPanel, setShowRightPanel] = useState(
    () => localStorage.getItem('deveco-switch-right-panel') !== 'collapsed',
  )
  const [rightTab, setRightTab] = useState<'overview' | 'files' | 'memories' | 'stats' | 'git' | 'preview' | 'devices' | 'symbols' | 'terminal' | 'analyze'>('overview')
  const [sidebarCollapsed, setSidebarCollapsed] = useState(
    () => localStorage.getItem('deveco-switch-sidebar-collapsed') === '1',
  )
  // 侧栏宽度（可拖拽调宽，记忆上次调整）
  const [sidebarWidth, setSidebarWidth] = useState(() => {
    const v = Number(localStorage.getItem('deveco-switch-sidebar-width'))
    return Number.isFinite(v) && v >= 180 && v <= 420 ? v : 256
  })
  const [rightWidth, setRightWidth] = useState(() => {
    const v = Number(localStorage.getItem('deveco-switch-right-width'))
    const max = Math.min(900, Math.floor(window.innerWidth * 0.65))
    return Number.isFinite(v) && v >= 240 && v <= max ? v : 288
  })
  // 右侧栏过窄时 Tab 仅显示图标、隐藏文字，避免文字竖排（阈值：全文字 Tab 需约 420px）
  const rightCompact = rightWidth < 400
  const [draft, setDraft] = useState('')
  // 会话搜索（侧栏搜索框；Ctrl+K 聚焦）
  const [searchText, setSearchText] = useState('')
  const searchInputRef = useRef<HTMLInputElement>(null)
  // 消息全文搜索：conv=按会话标题/首条消息；msg=按消息正文
  const [searchMode, setSearchMode] = useState<'conv' | 'msg'>('conv')
  const [msgHits, setMsgHits] = useState<MessageSearchHit[]>([])
  const [msgSearching, setMsgSearching] = useState(false)
  // 消息搜索命中后高亮某条消息（3 秒后自动清除）
  const [highlightMsgId, setHighlightMsgId] = useState<string | null>(null)
  // 斜杠快捷指令：输入 / 弹出候选（构建/部署/新建页面/解释代码/审查等）
  const [slashCandidates, setSlashCandidates] = useState<{ id: string; icon: IconName; title: string; prompt: string }[] | null>(null)
  const [slashIdx, setSlashIdx] = useState(0)
  // @ 引用：候选面板 + 已选引用列表（发送时随 references 落库注入文件内容）
  const [refCandidates, setRefCandidates] = useState<{ path: string; name: string }[] | null>(null)
  const [refQuery, setRefQuery] = useState('')
  const [refIdx, setRefIdx] = useState(0)
  const [references, setReferences] = useState<string[]>([])
  // 多模态图片（粘贴/拖入，data URL；发送时随消息上传，后端按协议内联）
  const [pickedImages, setPickedImages] = useState<string[]>([])
  // Rules 编辑弹窗（全局指令 + 项目级 rules，均注入 system_prompt）
  const [showRulesDialog, setShowRulesDialog] = useState(false)
  const [rulesTab, setRulesTab] = useState<'global' | 'project'>('global')
  const [rulesGlobalText, setRulesGlobalText] = useState('')
  const [rulesProjectText, setRulesProjectText] = useState('')
  const [rulesSaving, setRulesSaving] = useState(false)
  // 任务回滚（git 硬重置到任务起点前最后一次提交）
  const [rollbackBusy, setRollbackBusy] = useState(false)
  // 任务已运行时长（秒，每秒刷新；会话列表/右侧面板展示，静默期也能看到任务在走）
  const [taskElapsed, setTaskElapsed] = useState(0)
  // 任务过程徽章展开态：流式“已处理 N 个操作中”与完成后回看共用
  const [opsOpen, setOpsOpen] = useState(false)
  // 静默时长（秒）：流式期间距最近一次内容/思考增量的秒数，超阈值提示“模型思考中”
  const [silentSeconds, setSilentSeconds] = useState(0)
  // 右侧栏 Web 预览：待打开地址 + 当前 iframe 地址
  const [previewUrl, setPreviewUrl] = useState(() => localStorage.getItem('deveco-switch-preview-url') || 'http://localhost:5173')
  const [previewSrc, setPreviewSrc] = useState('')
  const [inputHeight, setInputHeight] = useState(96)
  const [renamingId, setRenamingId] = useState<string | null>(null)
  const [renamingText, setRenamingText] = useState('')
  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null)
  const [showArchived, setShowArchived] = useState(false)
  const [showSettingsMenu, setShowSettingsMenu] = useState(false)
  // 构建/部署由失败转成功时后端推送的"修复经验候选"（toast + 保存弹窗）
  const [knowledgeCandidate, setKnowledgeCandidate] = useState<{
    title: string
    error_text: string
    fix: string
  } | null>(null)
  const [candidateSaving, setCandidateSaving] = useState(false)
  // 运行时异常（部署后监听捕获）：弹卡片供用户一键让 Agent 修复
  const [runtimeAnomaly, setRuntimeAnomaly] = useState<{
    category: string
    summary: string
    detail: string
  } | null>(null)
  // 部署后运行日志监听中（后端成功启动监听时置 true，用于 UI 指示）
  const [runtimeWatching, setRuntimeWatching] = useState(false)
  // 鸿蒙工具链健康状态：ok=齐全 / warn=部分缺失 / bad=关键工具缺失，用于设置菜单红点提示
  const [envHealth, setEnvHealth] = useState<'ok' | 'warn' | 'bad' | null>(null)
  // 各项目的项目级专属配置数量（MCP/技能），用于项目列表徽标
  const [scopedCounts, setScopedCounts] = useState<Record<string, { mcp: number; skills: number }>>({})
  const refreshScopedCounts = useCallback(() => {
    projectScopedCounts().then(setScopedCounts).catch(() => {})
  }, [])
  useEffect(() => { refreshScopedCounts() }, [refreshScopedCounts])
  useEffect(() => {
    let cancelled = false
    getHarmonyEnv().then((env) => {
      if (cancelled) return
      const hasHdc = !!env.hdc_path
      const hasOhpm = !!env.ohpm_path
      const hasSdk = !!env.sdk_root && (env.sdk_versions?.length ?? 0) > 0
      setEnvHealth(hasHdc && hasOhpm && hasSdk ? 'ok' : hasHdc ? 'warn' : 'bad')
    }).catch(() => !cancelled && setEnvHealth('bad'))
    return () => { cancelled = true }
  }, [])
  const [showModelSettings, setShowModelSettings] = useState(false)
  // 划词菜单（复制/解释/翻译/搜索/引用回复）
  const [selectionMenu, setSelectionMenu] = useState<{ x: number; y: number; text: string } | null>(null)
  // 划词选区快照：点击菜单按钮时浏览器可能清除选区高亮，操作前用快照恢复，保证用户能看到选中内容
  const selectionRangeRef = useRef<Range | null>(null)
  // 选区快照配套：完整文本与所在容器。React 渲染重建消息 DOM 后 live Range 端点节点被替换、
  // 端点自动归一化收缩，恢复时按文本在容器内重新定位端点（见 restoreSelectionRange）
  const selectionTextRef = useRef('')
  const selectionContainerRef = useRef<Node | null>(null)
  // 划词完整快照（selectionchange 拖拽期间保存）：WebView2 内核在 mouseup 事件分派中段会把
  // 跨表格拖拽的选区端点归一化到表格前，bubble 阶段读到的选区已截断，而拖拽过程中选区始终
  // 完整；selectionchange 持续保存"更长"的快照（只增不减），天然抗截断，
  // 供菜单文本、复制内容和延迟恢复高亮使用
  const captureRangeRef = useRef<Range | null>(null)
  const captureTextRef = useRef('')
  // 拖拽进行中标记：mouseup 后内核会提交截断选区并触发 selectionchange，用标记阻止其覆盖完整快照
  const dragActiveRef = useRef(false)
  // 点踩原因弹窗
  const [feedbackDialog, setFeedbackDialog] = useState<{ messageId: string } | null>(null)
  // 回复版本 diff 弹窗
  const [versionDialog, setVersionDialog] = useState<{ userMessageId: string; current: string } | null>(null)
  // 记忆总结确认弹窗
  const [memoryDraft, setMemoryDraft] = useState<MemoryDraft | null>(null)
  // 导出菜单
  const [showExportMenu, setShowExportMenu] = useState(false)
  // 顶栏"更多操作"折叠菜单（小屏友好：回滚/导出/记忆/压缩/终端收进此处）
  const [showMoreMenu, setShowMoreMenu] = useState(false)
  // 正在朗读的消息 id
  const [speakingId, setSpeakingId] = useState<string | null>(null)
  // 编辑消息弹窗目标（仅 user 消息可编辑）
  const [editTarget, setEditTarget] = useState<ChatMessage | null>(null)
  // 删除消息二次确认（第一次点击进入确认态，3 秒内再点执行）
  const [confirmDeleteMsgId, setConfirmDeleteMsgId] = useState<string | null>(null)
  // 拖拽调宽中的侧栏（拖拽时禁用宽度过渡动画，避免拖尾）
  const [resizing, setResizing] = useState<'sidebar' | 'right' | null>(null)
  const [modelCatalog, setModelCatalog] = useState<{ providerName: string; models: ProviderModel[] }[]>([])
  const [modelOptions, setModelOptions] = useState<ChatOptions>(() => {
    try {
      return JSON.parse(localStorage.getItem('deveco-switch-chat-options') || '{}')
    } catch {
      return {}
    }
  })
  const [planFeedback, setPlanFeedback] = useState('')
  // 工具审批弹窗：选择记忆范围（空=仅本次；session=本会话免审；project=本项目持久化免审）；拒绝理由反馈给模型
  const [approvalScope, setApprovalScope] = useState<'' | 'session' | 'project'>('')
  const [approvalFeedback, setApprovalFeedback] = useState('')
  useEffect(() => {
    // 审批队列切换时重置选择与理由（每个工具独立决策）
    setApprovalScope('')
    setApprovalFeedback('')
  }, [toolApprovals[0]?.requestId])
  // 工具风险分级展示：L0 只读=绿 / L1 写入=橙 / L2 危险=红
  const approvalRisk = (tool: string): { label: string; cls: string } => {
    if (/^(bash|exec|run_command|delete_|remove_|spawn_agents|git_push|publish|deploy|format_)/.test(tool)) {
      return { label: 'L2 高风险', cls: 'bg-[var(--danger)]/15 text-[var(--danger)]' }
    }
    if (/^(write_|edit_|apply_|create_|move_|install|build|run_|preview|update_|replace_|patch)/.test(tool)) {
      return { label: 'L1 写入', cls: 'bg-[var(--warning)]/15 text-[var(--warning)]' }
    }
    return { label: 'L0 只读', cls: 'bg-[var(--success)]/15 text-[var(--success)]' }
  }
  // 计划卡片：可直接编辑计划正文；editing 切换查看/编辑，planDraft 保存编辑内容
  const [planEditing, setPlanEditing] = useState(false)
  const [planDraft, setPlanDraft] = useState('')
  useEffect(() => {
    if (pendingPlan) {
      setPlanDraft(pendingPlan.plan)
      setPlanEditing(false)
      setPlanFeedback('')
    }
  }, [pendingPlan?.requestId])
  // Agent 提问卡：新问题到来时重置回答输入
  const [askAnswer, setAskAnswer] = useState('')
  useEffect(() => {
    if (askCard) setAskAnswer('')
  }, [askCard?.requestId])
  // 上下文可视条：消息数 + 摘要状态 + token 预算占用（切换会话/收到新消息后刷新）
  const [ctxInfo, setCtxInfo] = useState<ConversationContextInfo | null>(null)
  useEffect(() => {
    if (!currentConversation) {
      setCtxInfo(null)
      return
    }
    let cancelled = false
    getConversationContext(currentConversation.id)
      .then((info) => !cancelled && setCtxInfo(info))
      .catch(() => {})
    return () => {
      cancelled = true
    }
  }, [currentConversation?.id, messages.length, modelOptions.model_id])
  // 会话跟随模型：切换会话时恢复该会话绑定的模型（未绑定的会话保持当前全局选择）
  useEffect(() => {
    if (currentConversation?.model_id) {
      setModelOptions((prev) => {
        if (prev.model_id === currentConversation.model_id) return prev
        const next = { ...prev, model_id: currentConversation.model_id ?? undefined }
        localStorage.setItem('deveco-switch-chat-options', JSON.stringify(next))
        return next
      })
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [currentConversation?.id])
  // 项目目录监视：外部工具（IDE/编辑器/其他进程）修改文件时，节流刷新文件树与 Git 面板，
  // 让界面实时感知项目变化（Agent 执行工具产生的修改同样感知；构建产物目录已过滤）
  useEffect(() => {
    if (!currentProject?.path) return
    let cancelled = false
    let unwatch: (() => void) | undefined
    let lastRefresh = 0
    const IGNORE_SEG = ['.git', 'node_modules', 'build', 'oh_modules', '.hvigor', 'target', 'dist', '.idea', '.preview']
    const shouldIgnore = (p: string) => {
      const lower = p.toLowerCase()
      return IGNORE_SEG.some((s) => lower.includes(`/${s}/`) || lower.includes(`\\${s}\\`))
    }
    watch(
      currentProject.path,
      (event) => {
        if (cancelled) return
        const paths = event.paths ?? []
        // 全部变更都在忽略目录内（如仅构建产物变化）→ 不刷新
        if (paths.length > 0 && paths.every(shouldIgnore)) return
        // 节流：2 秒内最多刷新一次（合并高频变更，如 git 操作/批量写文件）
        const now = Date.now()
        if (now - lastRefresh < 2000) return
        lastRefresh = now
        useProjectStore.getState().rebuildIndex().catch(() => {})
        useProjectStore.getState().refreshGitBranches().catch(() => {})
      },
      { recursive: true, delayMs: 800 },
    )
      .then((stop) => {
        if (cancelled) stop()
        else unwatch = stop
      })
      .catch(() => {})
    return () => {
      cancelled = true
      unwatch?.()
    }
  }, [currentProject?.id])
  // 排队中消息列表：消息/任务状态变化时刷新（任务结束后排队消息被消费清空）
  const [queuedOpen, setQueuedOpen] = useState(false)
  useEffect(() => {
    if (!currentConversation) return
    refreshQueued(currentConversation.id)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [currentConversation?.id, messages.length, streaming.conversationId])
  // 项目审批白名单管理弹窗（查看/移除已永久放行的工具）
  const [whitelistOpen, setWhitelistOpen] = useState(false)
  const [whitelist, setWhitelist] = useState<WhitelistEntry[]>([])
  const openWhitelistDialog = async () => {
    if (!currentProject) return
    setWhitelistOpen(true)
    try {
      setWhitelist(await listToolWhitelist(currentProject.id))
    } catch {
      setWhitelist([])
    }
  }
  const modelSettingsRef = useRef<HTMLDivElement>(null)
  const exportMenuRef = useRef<HTMLDivElement>(null)
  const moreMenuRef = useRef<HTMLDivElement>(null)
  const bottomRef = useRef<HTMLDivElement>(null)
  const inputRef = useRef<HTMLTextAreaElement>(null)
  const dragRef = useRef<{ startY: number; startH: number } | null>(null)
  const sidebarDragRef = useRef<{ startX: number; startW: number } | null>(null)
  const rightDragRef = useRef<{ startX: number; startW: number } | null>(null)
  const settingsRef = useRef<HTMLDivElement>(null)

  // 渲染分组：连续 tool 消息合并为工具折叠组（历史工具记录一行展示，点击展开全部）；
  // 其余消息保持原序，并附带回复归属的 userMessageId（版本分组键）；日期变化处插入分隔线
  const renderItems = useMemo(() => {
    type Item =
      | { kind: 'msg'; message: ChatMessage; userMessageId: string }
      | { kind: 'tools'; key: string; runs: ToolRun[] }
      | { kind: 'divider'; key: string; label: string }
    const items: Item[] = []
    let lastDayKey = ''
    const dayKeyOf = (ts: number) => {
      const d = new Date(ts * 1000)
      return `${d.getFullYear()}-${d.getMonth()}-${d.getDate()}`
    }
    const dayLabel = (ts: number) => {
      const d = new Date(ts * 1000)
      const today = new Date()
      const s = (x: Date) => new Date(x.getFullYear(), x.getMonth(), x.getDate()).getTime()
      const diff = Math.round((s(today) - s(d)) / 86400000)
      if (diff <= 0) return t('home.dayToday')
      if (diff === 1) return t('home.dayYesterday')
      return d.toLocaleDateString()
    }
    messages.forEach((m, idx) => {
      // 日期变化：插入分隔线（跨天分组，历史会话快速定位）
      const dk = dayKeyOf(m.created_at)
      if (dk !== lastDayKey) {
        lastDayKey = dk
        items.push({ kind: 'divider', key: `div-${dk}`, label: dayLabel(m.created_at) })
      }
      if (m.role === 'tool') {
        // tool 消息入库格式："工具名\n输出" → 转 ToolRun 并入当前组
        const [toolName, ...rest] = m.content.split('\n')
        const output = rest.join('\n')
        const run: ToolRun = {
          id: `hist-${m.id}`,
          tool: toolName || 'tool',
          args: '',
          status: output.trimStart().startsWith('执行失败') ? 'error' : 'done',
          output,
        }
        const last = items[items.length - 1]
        if (last && last.kind === 'tools') {
          last.runs.push(run)
        } else {
          items.push({ kind: 'tools', key: m.id, runs: [run] })
        }
        return
      }
      // 回复归属：向前找最近一条 user 消息（版本分组键）
      let userMessageId = ''
      for (let i = idx; i >= 0; i--) {
        if (messages[i].role === 'user') {
          userMessageId = messages[i].id
          break
        }
      }
      items.push({ kind: 'msg', message: m, userMessageId })
    })
    // 旧数据兼容：历史版本中 tool 消息时间戳晚于正文（工具入库在正文之后），
    // 把位于 assistant 正文之后的工具组前移到正文之前（工具先执行后输出的自然顺序）
    const reordered: Item[] = []
    for (const item of items) {
      if (item.kind === 'tools' && reordered.length > 0) {
        const prev = reordered[reordered.length - 1]
        if (prev.kind === 'msg' && prev.message.role === 'assistant') {
          reordered[reordered.length - 1] = item
          reordered.push(prev)
          continue
        }
      }
      reordered.push(item)
    }
    return reordered
  }, [messages, t])

  // 启动加载项目列表
  useEffect(() => {
    refreshProjects().catch(() => {})
  }, [refreshProjects])

  // 托盘菜单 / 全局快捷键：新建对话 & 打开设置
  useEffect(() => {
    let cancelled = false
    const unlisteners: Array<() => void> = []
    listen('tray-new-chat', () => {
      void newConversation()
      inputRef.current?.focus()
    }).then((u) => !cancelled && unlisteners.push(u)).catch(() => {})
    listen('tray-open-settings', () => {
      setShowSettingsMenu(true)
    }).then((u) => !cancelled && unlisteners.push(u)).catch(() => {})
    // 运行时异常：部署后监听捕获到应用 error/崩溃，弹卡片供一键修复
    listen<{ category: string; summary: string; detail: string }>('runtime-anomaly', (e) => {
      const p = e.payload as { category: string; summary: string; detail: string }
      setRuntimeAnomaly(p)
    }).then((u) => !cancelled && unlisteners.push(u)).catch(() => {})
    // 后端开启/停止运行日志监听时更新 UI 指示
    listen<{ watching: boolean }>('runtime-watching', (e) => {
      setRuntimeWatching(!!(e.payload as { watching?: boolean })?.watching)
    }).then((u) => !cancelled && unlisteners.push(u)).catch(() => {})
    listen<{ source: string; project_path: string; title: string; error_text: string; fix: string }>('knowledge-candidate', (e) => {
      const payload = e.payload as { title: string; error_text: string; fix: string }
      setKnowledgeCandidate(payload)
    }).then((u) => !cancelled && unlisteners.push(u)).catch(() => {})
    return () => {
      cancelled = true
      unlisteners.forEach((u) => u())
    }
  }, [newConversation])

  // 运行时异常一键修复：把异常信息拼成指令，自动发送给 Agent 触发自主修复闭环
  const fixRuntimeAnomaly = async () => {
    if (!runtimeAnomaly || !currentProject) return
    const a = runtimeAnomaly
    setRuntimeAnomaly(null)
    const prompt = `应用运行时检测到异常（${a.category}），请自行修复：\n${a.summary}\n\n相关日志：\n${a.detail}\n\n请用 read_runtime_logs 读取完整运行日志，定位源码后修复，然后重新构建并部署验证。`
    if (!currentConversation) {
      await newConversation()
    }
    await sendUserMessage(prompt, modelOptions)
  }

  // 保存"修复经验候选"：把刚解决的错误沉淀为知识库条目（自动提取触发关键词）
  const saveKnowledgeCandidate = async () => {
    if (!knowledgeCandidate) return
    setCandidateSaving(true)
    try {
      const pid = currentProject?.id ?? null
      await saveKnowledgeFromText(
        {
          title: knowledgeCandidate.title || undefined,
          error_text: knowledgeCandidate.error_text,
          fix: knowledgeCandidate.fix,
        },
        pid,
      )
      setKnowledgeCandidate(null)
    } catch (e) {
      alert(String(e))
    } finally {
      setCandidateSaving(false)
    }
  }

  // 压缩完成事件（手动 /compact 或运行中自动压缩）：刷新上下文可视条。
  // 压缩不删消息，消息数不变时 ctxInfo 的 effect 依赖不会触发，须由事件驱动
  useEffect(() => {
    let cancelled = false
    let dispose: (() => void) | undefined
    listen<{ conversation_id: string; keep: number }>('chat-compact', (e) => {
      const conv = useProjectStore.getState().currentConversation
      if (cancelled || !conv || e.payload.conversation_id !== conv.id) return
      getConversationContext(conv.id)
        .then((info) => !cancelled && setCtxInfo(info))
        .catch(() => {})
    })
      .then((u) => {
        if (!cancelled) dispose = u
      })
      .catch(() => {})
    return () => {
      cancelled = true
      dispose?.()
    }
  }, [])

  // 设置菜单外部点击关闭
  useEffect(() => {
    const handler = (e: MouseEvent) => {
      if (settingsRef.current && !settingsRef.current.contains(e.target as Node)) {
        setShowSettingsMenu(false)
      }
    }
    document.addEventListener('mousedown', handler)
    return () => document.removeEventListener('mousedown', handler)
  }, [])

  // 模型设置面板外部点击关闭
  useEffect(() => {
    const handler = (e: MouseEvent) => {
      if (modelSettingsRef.current && !modelSettingsRef.current.contains(e.target as Node)) {
        setShowModelSettings(false)
      }
    }
    document.addEventListener('mousedown', handler)
    return () => document.removeEventListener('mousedown', handler)
  }, [])

  // 导出菜单 / 更多菜单外部点击关闭
  useEffect(() => {
    const handler = (e: MouseEvent) => {
      if (exportMenuRef.current && !exportMenuRef.current.contains(e.target as Node)) {
        setShowExportMenu(false)
      }
      if (moreMenuRef.current && !moreMenuRef.current.contains(e.target as Node)) {
        setShowMoreMenu(false)
      }
    }
    document.addEventListener('mousedown', handler)
    return () => document.removeEventListener('mousedown', handler)
  }, [])

  // 加载全部 Provider 与模型（用于对话框模型选择）
  useEffect(() => {
    listProviders()
      .then(async (ps) => {
        const entries = await Promise.all(
          ps.map(async (p) => ({
            providerName: p.name,
            models: await listProviderModels(p.id).catch(() => [] as ProviderModel[]),
          })),
        )
        setModelCatalog(entries)
      })
      .catch(() => {})
  }, [])

  // 自动打开最近项目（仅首次加载后且未选中）
  useEffect(() => {
    if (projects.length > 0 && !currentProject) {
      openProject(projects[0].id).catch(() => {})
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [projects.length])

  // 新消息滚动到底部
  // 智能贴底：流式输出期间，仅当用户已在底部附近时才自动跟随；用户上滑查看历史时不打断
  const scrollRef = useRef<HTMLDivElement>(null)
  const stickToBottomRef = useRef(true)
  const [showScrollBottom, setShowScrollBottom] = useState(false)
  // 未读数按对话维度统计（conversationId → count）：滚离底部期间该对话新消息到达时累加，回到底部/切换对话清零。
  // 跨会话持久化到 localStorage，应用重启后对话列表仍保留未读标记。
  const [unreadMap, setUnreadMap] = useState<Record<string, number>>(() => {
    try {
      const raw = localStorage.getItem('deveco-unread-map')
      return raw ? JSON.parse(raw) : {}
    } catch {
      return {}
    }
  })
  const persistUnreadMap = useRef<number | null>(null)
  useEffect(() => {
    if (persistUnreadMap.current) cancelIdleCallback(persistUnreadMap.current)
    persistUnreadMap.current = requestIdleCallback(() => {
      try {
        localStorage.setItem('deveco-unread-map', JSON.stringify(unreadMap))
      } catch {
        // 忽略写入失败
      }
    }, { timeout: 2000 })
  }, [unreadMap])
  const currentConvId = currentConversation?.id
  const unreadCount = currentConvId ? unreadMap[currentConvId] ?? 0 : 0
  const scrollRafRef = useRef<number | null>(null)

  const isNearBottom = () => {
    const el = scrollRef.current
    if (!el) return true
    const threshold = 120
    return el.scrollHeight - el.scrollTop - el.clientHeight < threshold
  }

  const scrollToBottom = (smooth = true) => {
    const el = scrollRef.current
    if (!el) return
    el.scrollTo({ top: el.scrollHeight, behavior: smooth ? 'smooth' : 'auto' })
    stickToBottomRef.current = true
    setShowScrollBottom(false)
    if (currentConvId) {
      setUnreadMap((m) => {
        if (!m[currentConvId]) return m
        const next = { ...m }
        delete next[currentConvId]
        return next
      })
    }
  }

  const handleScroll = () => {
    const near = isNearBottom()
    stickToBottomRef.current = near
    setShowScrollBottom(!near)
  }

  /** rAF 节流的贴底滚动：流式内容高频更新时每帧最多滚动一次，避免卡顿 */
  const rafScrollToBottom = () => {
    if (scrollRafRef.current != null) return
    scrollRafRef.current = requestAnimationFrame(() => {
      scrollRafRef.current = null
      const el = scrollRef.current
      if (el) el.scrollTo({ top: el.scrollHeight, behavior: 'auto' })
    })
  }

  useEffect(() => {
    // 新消息入库（条数变化）时，如果之前贴底则跟随；否则给当前对话累计未读数
    if (stickToBottomRef.current) {
      scrollToBottom(true)
    } else if (currentConvId) {
      setUnreadMap((m) => ({ ...m, [currentConvId]: (m[currentConvId] ?? 0) + 1 }))
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [messages.length, currentConvId])

  // 流式内容增长（含思考过程/工具运行数变化）时，贴底状态下持续跟随（rAF 节流）
  const streamingLen = streaming.content.length + streaming.reasoning.length
  const streamingActive = streaming.conversationId === currentConversation?.id
  useEffect(() => {
    if (stickToBottomRef.current && streamingActive) {
      rafScrollToBottom()
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [streamingLen, streamingActive, toolRuns.length, agentRuns.length])

  // 组件卸载时清理 rAF
  useEffect(() => {
    return () => {
      if (scrollRafRef.current != null) cancelAnimationFrame(scrollRafRef.current)
    }
  }, [])

  // 切换对话时：重置贴底状态并清除该对话的未读数，下次内容加载后自动滚到底部
  useEffect(() => {
    stickToBottomRef.current = true
    setShowScrollBottom(false)
    if (currentConvId) {
      setUnreadMap((m) => {
        if (!m[currentConvId]) return m
        const next = { ...m }
        delete next[currentConvId]
        return next
      })
    }
    // 等消息渲染后再滚一次
    const t = setTimeout(() => scrollToBottom(false), 60)
    return () => clearTimeout(t)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [currentConvId])

  const handleAddConfirm = async (path: string) => {
    const project = await addProjectByPath(path)
    const inspect = await inspectProject(path)
    setPendingTrust({ projectId: project.id, inspect })
  }

  const handleTrust = async () => {
    if (!pendingTrust) return
    setTrustBusy(true)
    try {
      await confirmTrust(pendingTrust.projectId)
      setPendingTrust(null)
      await openProject(pendingTrust.projectId)
      // 信任并打开后，后台异步扫描工作区模块（大目录递归遍历可能耗时，
      // 不阻塞 UI；完成后刷新项目列表即可更新类型标签与模块卡）
      const pid = pendingTrust.projectId
      void (async () => {
        try {
          await rescanWorkspaceModules(pid)
          await refreshProjects()
        } catch {
          // 扫描失败静默：用户可在项目概览手动重新扫描
        }
      })()
    } finally {
      setTrustBusy(false)
    }
  }

  const handleReject = async () => {
    if (!pendingTrust) return
    setTrustBusy(true)
    try {
      await removeProject(pendingTrust.projectId)
      setPendingTrust(null)
    } finally {
      setTrustBusy(false)
    }
  }

  // 文件树引用：把文件路径插入输入框并加入引用列表（发送时 references 落库注入文件内容）
  const handleReference = (path: string) => {
    setDraft((d) => (d ? `${d} @${path} ` : `@${path} `))
    setReferences((r) => (r.includes(path) ? r : [...r, path]))
    inputRef.current?.focus()
  }

  /** 当前会话是否正在流式生成（派生值：供下方处理函数与 effect 使用，声明需在使用前） */
  const isStreaming = streaming.conversationId === currentConversation?.id

  /** 构建错误一键修复：将结构化错误摘要注入对话输入框并聚焦，让用户直接交给 Agent 修复 */
  const handleFixBuildErrors = (errors: AnalyzedBuildError[]) => {
    const lines = errors.map((e) => {
      const loc = e.file ? `${e.file}${e.line ? `:${e.line}` : ''}${e.column ? `:${e.column}` : ''}` : '未知位置'
      return `- ${loc} [${e.kind}] ${e.message}`
    })
    const paths = errors.map((e) => e.file).filter((f): f is string => !!f)
    setReferences((r) => {
      const next = [...r]
      for (const p of paths) {
        if (!next.includes(p)) next.push(p)
      }
      return next
    })
    setDraft((d) =>
      `${d ? d.trimEnd() + '\n\n' : ''}请分析并修复以下鸿蒙构建错误（${errors.length} 处）：\n${lines.join('\n')}\n\n修复后请重新构建验证。`,
    )
    inputRef.current?.focus()
  }

  /** 失败工具一键重试：注入指令让 Agent 重新执行该工具（失败输出头尾截断后附给模型参考） */
  const retryTool = (run: ToolRun) => {
    // 头尾保留：命令/环境信息多在头部，错误原因多在尾部，中段省略
    const out = run.output
    const head = 500
    const tailN = 800
    const tail =
      out.length <= head + tailN ? out : `${out.slice(0, head)}\n…(中段省略)…\n${out.slice(-tailN)}`
    const text =
      `请重试刚才失败的 ${run.tool} 工具。上次失败信息如下：\n${tail}\n\n` +
      '请先分析失败原因（参数/环境/前置步骤），修正后重新执行该工具，并继续后续步骤。'
    if (isStreaming) {
      void queueUserMessage(text, true)
    } else {
      void sendUserMessage(text, modelOptions)
    }
  }

  /** 构建错误自动修复闭环：直接向 Agent 发送修复任务（自动改代码 + 重新构建验证），无需手动发送 */
  const handleAutoFixErrors = async (errors: AnalyzedBuildError[]) => {
    if (errors.length === 0) return
    if (!currentProject) return
    if (!currentConversation) {
      await newConversation()
    }
    const lines = errors.map((e) => {
      const loc = e.file ? `${e.file}${e.line ? `:${e.line}` : ''}${e.column ? `:${e.column}` : ''}` : '未知位置'
      return `- ${loc} [${e.category}/${e.kind}] ${e.message}`
    })
    // 按根因分类统计，并前置针对性修复策略，提升首次修复命中率
    const cats = new Map<string, number>()
    for (const e of errors) cats.set(e.category, (cats.get(e.category) ?? 0) + 1)
    const strategy: Record<string, string> = {
      dependency: '依赖类：先核对 oh-package.json5 声明，缺失则执行 ohpm install；import 路径错误则修正引用',
      signing: '签名类：检查 build-profile.json5 的 signingConfigs；这类通常需用户在 DevEco 配置证书，不要盲目改代码',
      sdk: 'SDK 类：用 check_sdk_alignment 核对 compatibleSdkVersion 与已装 SDK，缺失则提示用户安装对应 SDK',
      api_level: 'API 级别类：该 API 高于工程 compatibleSdkVersion，改用低版本等价 API 或合理提升 compatibleSdkVersion',
      type: '类型类：阅读对应行号上下文，按 ArkTS 类型系统修正类型不匹配/空值/泛型，不要用 any 糊弄',
      syntax: '语法类：定位语法错误（括号/装饰器/import 语句），按 ArkTS 规范修正',
      resource: '资源类：检查 $r() 引用的资源名是否在 resources 下存在，缺失则补充或修正引用',
      ohpm: 'ohpm 工具链类：检查 ohpm 是否可用、版本是否匹配，必要时提示用户重装',
    }
    const catHints = [...cats.entries()]
      .map(([c, n]) => `  - ${c}（${n} 处）：${strategy[c] ?? '按错误信息定位修复'}`)
      .join('\n')
    const paths = errors.map((e) => e.file).filter((f): f is string => !!f)
    const text = [
      `请自动修复以下鸿蒙构建错误（共 ${errors.length} 处，已按根因分类）：`,
      ...lines,
      '',
      '根因分类与优先策略：',
      catHints,
      '',
      '修复闭环要求：',
      '0. 先参考上方注入的项目记忆（本工程历史构建错误与修复经验），同类问题优先复用已验证的解法，避免重复踩坑',
      '1. 按上述分类策略先处理对应根因，再阅读相关文件与错误上下文',
      '2. 用 edit_file/write_file 修改代码；遵循鸿蒙 ArkTS 规范（@kit import、API 级别约束），不确定 API 时先 search_sdk_api / search_harmony_docs 查证',
      '3. 依赖问题优先 ohpm_install；SDK 版本问题优先 check_sdk_alignment；签名/证书类问题不要改代码，调用 show_diagnose_card 弹出引导卡片并向用户说明需在 DevEco 配置',
      '4. 调用构建工具重新构建，确认构建通过后再结束；若仍有错误继续修复，但不要用相同方式重复失败的构建',
    ].join('\n')
    setDraft('')
    setReferences([])
    // 有进行中任务时排队并入，否则直接发送（sendUserMessage 内部读取最新会话状态）
    const busy = !!streaming.conversationId
    // 自动修复默认使用 first_write 审批：首次写文件前确认，本任务后续写操作免审，
    // 平衡自动化效率与代码安全；若用户已设为更严格的 ask 则尊重用户设置
    const fixOptions: ChatOptions =
      modelOptions.tool_approval === 'ask'
        ? modelOptions
        : { ...modelOptions, tool_approval: 'first_write' }
    if (busy) {
      void queueUserMessage(text, false, paths.length ? paths : undefined)
    } else {
      void sendUserMessage(text, fixOptions, paths.length ? paths : undefined)
    }
  }

  // Agent 任务结束（流式从运行到停止）时，若工程分析面板打开则自动刷新错误列表
  const [analyzeRefreshTick, setAnalyzeRefreshTick] = useState(0)
  const wasStreamingRef = useRef(false)
  useEffect(() => {
    if (wasStreamingRef.current && !isStreaming) {
      setAnalyzeRefreshTick((v) => v + 1)
    }
    wasStreamingRef.current = isStreaming
  }, [isStreaming])

  /**
   * 文件选区引用：把选中的代码片段作为 fenced code block 插入输入框，
   * 头部标注文件路径与行号范围，便于模型精确理解上下文。同时把该文件加入引用列表。
   */
  const handleReferenceSelection = (payload: { path: string; startLine: number; endLine: number; snippet: string }) => {
    const { path, startLine, endLine, snippet } = payload
    const range = startLine === endLine ? `L${startLine}` : `L${startLine}-L${endLine}`
    const block = `\n${path}#${range}\n\`\`\`\n${snippet}\n\`\`\`\n`
    setDraft((d) => (d ? `${d}${block}` : block))
    setReferences((r) => (r.includes(path) ? r : [...r, path]))
    inputRef.current?.focus()
  }

  /**
   * 代码块文件路径/行号点击：派发全局事件，由文件树面板打开预览并定位到行；
   * 同时将该文件加入 @ 引用，便于继续追问。rawPath 形如 "src/foo.ts:42"。
   */
  const openCodeFile = (rawPath: string) => {
    const lineMatch = rawPath.match(/:(\d+)$/)
    const line = lineMatch ? parseInt(lineMatch[1], 10) : undefined
    const path = rawPath.replace(/:\d+$/, '')
    handleReference(path)
    setRightTab('files')
    window.dispatchEvent(
      new CustomEvent('deveco:open-file', { detail: { path, line } }),
    )
  }

  /** 收集引用列表：显式 @ 选择 + 输入框内残留的 @path 文本（容错手动输入） */
  const collectRefs = (): string[] => {
    const fromText = Array.from(draft.matchAll(/@([^\s@]+)/g), (m) => m[1])
    return Array.from(new Set([...references, ...fromText]))
  }

  /** 图片文件 → data URL（粘贴/拖入共用；单图超 8MB 跳过，最多 4 张；发送前压缩控制 token 与供应商限制） */
  const addImageFiles = (files: FileList | File[]) => {
    for (const f of Array.from(files)) {
      if (!f.type.startsWith('image/') || f.size > 8 * 1024 * 1024) continue
      const reader = new FileReader()
      reader.onload = () => {
        void compressImage(String(reader.result)).then((url) => {
          setPickedImages((cur) => (cur.length >= 4 ? cur : [...cur, url]))
        })
      }
      reader.readAsDataURL(f)
    }
  }

  const removePickedImage = (idx: number) => {
    setPickedImages((cur) => cur.filter((_, i) => i !== idx))
  }

  /** @ 引用候选：@ 后输入路径片段（无空格/换行）时弹候选面板 */
  const handleDraftChange = (v: string) => {
    setDraft(v)
    // 斜杠指令：行首 / 触发，取 / 后到空格/换行的片段做过滤
    const slashMatch = v.match(/(?:^|\n)\/([^\s/]*)$/)
    if (slashMatch) {
      const q = slashMatch[1].toLowerCase()
      const all = getSlashCommands(t)
      const filtered = all.filter((c) => c.id.includes(q) || c.title.toLowerCase().includes(q)).slice(0, 8)
      setSlashCandidates(filtered)
      setSlashIdx(0)
    } else {
      setSlashCandidates(null)
    }
    const atIdx = v.lastIndexOf('@')
    if (atIdx >= 0 && (atIdx === 0 || !/[\w\u4e00-\u9fa5]/.test(v[atIdx - 1]))) {
      const rest = v.slice(atIdx + 1)
      if (rest && !rest.includes(' ') && !rest.includes('\n')) {
        const q = rest.toLowerCase()
        setRefQuery(q)
        const list = refPool
          .map((f) => {
            const score = fuzzyScore(f.path, q)
            if (score <= 0) return null
            // MRU 加权：最近引用过的路径额外加分（越近越高，最多 +200）
            const mruIdx = mruRefs.indexOf(f.path)
            const mruBonus = mruIdx >= 0 ? Math.max(0, 200 - mruIdx * 10) : 0
            return { f, rank: score + mruBonus }
          })
          .filter((x): x is { f: { path: string; name: string }; rank: number } => x !== null)
          .sort((a, b) => b.rank - a.rank)
          .slice(0, 8)
          .map((x) => x.f)
        setRefCandidates(list)
        setRefIdx(0)
        return
      }
    }
    setRefCandidates(null)
  }

  /** /compact 斜杠指令：手动压缩会话历史（较早消息总结为摘要，保留最近 10 条） */
  const compactHistory = async () => {
    if (!currentConversation) return
    try {
      const summary = await compactConversation(currentConversation.id, 10)
      sendNotification(t('home.compactDone'), summary.slice(0, 300))
      await useProjectStore.getState().openConversation(currentConversation.id).catch(() => {})
    } catch (e) {
      sendNotification(t('home.compactFail'), String(e), 'error')
    }
  }

  /** 选中斜杠指令：替换行首 /query 为完整 prompt；action 触发额外行为 */
  const pickSlash = (cmd: { id: string; prompt: string; action?: 'plan' | 'compact' }) => {
    if (cmd.prompt) {
      setDraft((d) => d.replace(/(^|\n)\/[^\s/]*$/, `$1${cmd.prompt} `))
    } else {
      setDraft((d) => d.replace(/(^|\n)\/[^\s/]*$/, ''))
    }
    setSlashCandidates(null)
    if (cmd.action === 'plan') {
      // /plan：开启计划模式（Agent 先出计划，批准后才执行工具），并提示用户
      updateModelOptions({ ...modelOptions, plan_mode: true })
    }
    if (cmd.action === 'compact') {
      // /compact：手动压缩历史为摘要（后台完成后提示）
      void compactHistory()
    }
    inputRef.current?.focus()
  }

  /** 选中候选：替换 @query 为 @path，加入引用列表 */
  const pickReference = (path: string) => {
    const atIdx = draft.lastIndexOf('@')
    if (atIdx < 0) return
    setDraft(draft.slice(0, atIdx) + `@${path} `)
    setReferences((r) => (r.includes(path) ? r : [...r, path]))
    setRefCandidates(null)
    recordMruRef(path)
    inputRef.current?.focus()
  }

  const handleSend = async () => {
    const text = draft.trim()
    if (!text) return
    if (!currentProject) return
    if (!currentConversation) {
      await newConversation()
    }
    const refs = collectRefs()
    const imgs = pickedImages
    // 先清空输入框：invoke 要等整个 Agent 任务结束后才 resolve，若 await 发送则任务期间输入框不会清空
    setDraft('')
    setReferences([])
    setPickedImages([])
    setRefCandidates(null)
    setSlashCandidates(null)
    if (isStreaming) {
      void queueUserMessage(text, false, refs.length ? refs : undefined, imgs.length ? imgs : undefined)
    } else {
      void sendUserMessage(text, modelOptions, refs.length ? refs : undefined, imgs.length ? imgs : undefined)
    }
  }

  /** 发送到 Agent：流式运行时提交为挂起消息，由 Agent 在任务内安全点并入当前任务 */
  const handleSendToAgent = async () => {
    const text = draft.trim()
    if (!text || !currentProject || !currentConversation || !isStreaming) return
    const refs = collectRefs()
    const imgs = pickedImages
    // 同 handleSend：先清空输入框，不 await 排队接口
    setDraft('')
    setReferences([])
    setPickedImages([])
    setRefCandidates(null)
    void queueUserMessage(text, true, refs.length ? refs : undefined, imgs.length ? imgs : undefined)
  }

  /** 删除消息（二次确认：第一次点击进入确认态，3 秒内再点执行；级联删除其后所有消息） */
  const handleDeleteMessage = async (msg: ChatMessage) => {
    if (confirmDeleteMsgId !== msg.id) {
      setConfirmDeleteMsgId(msg.id)
      setTimeout(() => setConfirmDeleteMsgId((cur) => (cur === msg.id ? null : cur)), 3000)
      return
    }
    setConfirmDeleteMsgId(null)
    await removeMessage(msg.id)
  }

  /** 更新对话级模型设置（持久化到 localStorage，随消息发送；同时绑定到当前会话使上下文预算实时生效） */
  const updateModelOptions = (next: ChatOptions) => {
    setModelOptions(next)
    localStorage.setItem('deveco-switch-chat-options', JSON.stringify(next))
    if (currentConversation) {
      // 后端写入完成后再刷新可视条（避免竞态读到旧 model_id 的 context_limit）
      void setConversationModel(currentConversation.id, next.model_id ?? '')
        .then(() =>
          getConversationContext(currentConversation.id)
            .then(setCtxInfo)
            .catch(() => {}),
        )
        .catch(() => {})
    }
  }

  /** 快捷操作：填入提示词并聚焦输入框 */
  const fillDraft = (text: string) => {
    setDraft(text)
    inputRef.current?.focus()
  }

  // @ 引用候选池：已加载文件树缓存（根层 + 展开过的目录）中的所有文件
  const refPool = useMemo(() => {
    const pool: { path: string; name: string }[] = []
    const seen = new Set<string>()
    for (const items of Object.values(dirCache)) {
      for (const it of items) {
        if (it.type !== 'file' || seen.has(it.path)) continue
        seen.add(it.path)
        pool.push({ path: it.path, name: it.name })
      }
    }
    return pool
  }, [dirCache])

  // @ 引用最近使用（MRU）：pickReference 时记录，按路径记忆最多 30 条，用于候选排序加权
  const [mruRefs, setMruRefs] = useState<string[]>(() => {
    try {
      const raw = localStorage.getItem('deveco-ref-mru')
      return raw ? JSON.parse(raw) : []
    } catch {
      return []
    }
  })
  const recordMruRef = (path: string) => {
    setMruRefs((prev) => {
      const next = [path, ...prev.filter((p) => p !== path)].slice(0, 30)
      try {
        localStorage.setItem('deveco-ref-mru', JSON.stringify(next))
      } catch {
        // 忽略写入失败
      }
      return next
    })
  }

  /**
   * 简单模糊评分：子序列匹配得基础分，连续匹配/起始匹配/路径末段匹配加分。
   * 返回 0 表示不匹配；分数越高越靠前。
   */
  const fuzzyScore = (path: string, query: string): number => {
    if (!query) return 1
    const p = path.toLowerCase()
    const q = query.toLowerCase()
    if (p.includes(q)) {
      // 子串匹配：路径末段（文件名）匹配权重更高
      const base = p.length - q.length
      const lastSeg = p.split('/').pop() ?? p
      const segBonus = lastSeg.startsWith(q) ? 100 : lastSeg.includes(q) ? 60 : 0
      return 1000 - base + segBonus
    }
    // 拼音全拼匹配：中文文件名可按完整拼音搜索（如 "首页" → "shouye"），权重高于首字母
    const pyFull = toPinyinFull(path)
    if (pyFull.includes(q)) {
      const lastSeg = pyFull.split('/').pop() ?? pyFull
      const segBonus = lastSeg.startsWith(q) ? 85 : lastSeg.includes(q) ? 45 : 0
      return 850 - pyFull.length + segBonus
    }
    // 拼音首字母匹配：中文文件名可按拼音首字母搜索（如 "首页" → "sy"）
    const py = toPinyinInitials(path)
    if (py.includes(q)) {
      const lastSeg = py.split('/').pop() ?? py
      const segBonus = lastSeg.startsWith(q) ? 90 : lastSeg.includes(q) ? 50 : 0
      // 拼音匹配权重略低于直接子串，但高于子序列
      return 800 - py.length + segBonus
    }
    // 子序列匹配（逐字符）
    let qi = 0
    let streak = 0
    let bestStreak = 0
    let score = 0
    for (let i = 0; i < p.length && qi < q.length; i++) {
      if (p[i] === q[qi]) {
        score += 1 + streak
        streak++
        bestStreak = Math.max(bestStreak, streak)
        qi++
      } else {
        streak = 0
      }
    }
    if (qi < q.length) return 0
    return score + bestStreak * 2
  }

  /** 会话搜索：本地输入即时响应，300ms 防抖后请求后端 LIKE 过滤；消息模式走全文检索 */
  useEffect(() => {
    const h = setTimeout(() => {
      if (searchMode === 'msg') {
        const pid = currentProject?.id
        const kw = searchText.trim()
        if (!pid || kw.length < 2) {
          setMsgHits([])
          setMsgSearching(false)
          return
        }
        setMsgSearching(true)
        searchMessages(pid, kw)
          .then((hits) => setMsgHits(hits))
          .catch(() => setMsgHits([]))
          .finally(() => setMsgSearching(false))
      } else {
        setMsgHits([])
        void setConversationKeyword(searchText)
      }
    }, 300)
    return () => clearTimeout(h)
  }, [searchText, searchMode, setConversationKeyword, currentProject?.id])

  /** 点击消息搜索命中：打开对应会话并高亮目标消息 */
  const openMessageHit = async (hit: MessageSearchHit) => {
    setSearchText('')
    setMsgHits([])
    await openConversation(hit.conversation_id)
    setHighlightMsgId(hit.message_id)
    setTimeout(() => {
      const el = document.querySelector(`[data-msg-id="${hit.message_id}"]`) as HTMLElement | null
      el?.scrollIntoView({ behavior: 'smooth', block: 'center' })
    }, 120)
    setTimeout(() => setHighlightMsgId(null), 3000)
  }

  /** 高亮消息片段中的命中关键字 */
  const highlightSnippet = (text: string) => {
    const kw = searchText.trim()
    if (!kw) return text
    const lower = text.toLowerCase()
    const kwLower = kw.toLowerCase()
    const idx = lower.indexOf(kwLower)
    if (idx < 0) return text
    return (
      <>
        {text.slice(0, idx)}
        <mark className="bg-[var(--warning)]/30 text-[var(--text-primary)] rounded px-0.5">{text.slice(idx, idx + kw.length)}</mark>
        {text.slice(idx + kw.length)}
      </>
    )
  }

  // 快捷键：Ctrl+K 聚焦会话搜索；Ctrl+Shift+B 构建 / Ctrl+Shift+D 部署 / Ctrl+Shift+N 新会话
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const mod = e.ctrlKey || e.metaKey
      if (mod && e.shiftKey) {
        const k = e.key.toLowerCase()
        if (k === 'b') {
          e.preventDefault()
          setDraft(t('home.quickBuildPrompt'))
          inputRef.current?.focus()
        } else if (k === 'd') {
          e.preventDefault()
          setDraft(t('home.quickDeployPrompt'))
          inputRef.current?.focus()
        } else if (k === 'n') {
          e.preventDefault()
          void newConversation()
        }
      } else if (mod && e.key.toLowerCase() === 'k') {
        e.preventDefault()
        searchInputRef.current?.focus()
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [newConversation, t])

  /** Web 预览：右侧栏内嵌加载 http/https 地址（iframe），失败时提示 */
  const handleOpenPreview = () => {
    const url = previewUrl.trim()
    if (!url) return
    localStorage.setItem('deveco-switch-preview-url', url)
    setPreviewSrc(url)
    setRightTab('preview')
  }

  /** 符号索引预热：项目切换/启动后在后台线程池预热（磁盘缓存命中 + 增量校正），
   *  让符号面板与首轮对话构建工程概要时秒出结果；静默执行不阻塞界面。 */
  useEffect(() => {
    if (!currentProject || currentProject.kind === 'global') return
    const pid = currentProject.id
    warmupSymbolIndex(pid).catch(() => {})
  }, [currentProject?.id])

  /** 自动补扫：旧项目（添加时未做工作区扫描）或新添加项目模块为空时，
   *  在后台异步扫描一次，不阻塞界面；扫描完成后刷新列表以更新类型标签与模块卡。 */
  useEffect(() => {
    if (!currentProject || currentProject.kind === 'global') return
    const isEmpty =
      !currentProject.workspace_modules ||
      currentProject.workspace_modules === '[]' ||
      currentProject.workspace_modules === ''
    if (!isEmpty) return
    let cancelled = false
    void (async () => {
      try {
        await rescanWorkspaceModules(currentProject.id)
        if (!cancelled) {
          await refreshProjects()
          // 通知工程分析面板重新解析候选、模块卡刷新主工程徽标
          setModuleScanTick((n) => n + 1)
        }
      } catch {
        // 静默失败：用户可在概览手动重新扫描
      }
    })()
    return () => {
      cancelled = true
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [currentProject?.id])

  /** 打开项目根目录系统终端（cmd 窗口），供用户手动执行命令 */
  const handleOpenTerminal = async () => {
    if (!currentProject) return
    try {
      await openTerminal(currentProject.path)
    } catch (e) {
      alert(`${t('home.openTerminalFail')}: ${String(e)}`)
    }
  }

  /** 诊断卡片操作：install_deps→切到工程分析面板执行 ohpm install；其他→打开 DevEco 或提示。
   *  操作完成后唤醒等待中的 Agent，使其根据结果重新构建验证 */
  const handleDiagnoseAction = async (card: { requestId: string; action: string; conversationId: string; id: string }) => {
    let completed = false
    let note = ''
    if (card.action === 'install_deps' && currentProject) {
      setRightTab('analyze')
      try {
        const log = await runOhpmInstall(currentProject.path)
        completed = true
        note = '依赖安装完成'
        useProjectStore.setState((s) => ({
          terminalEntries: [
            ...s.terminalEntries,
            { id: `diag-${Date.now()}`, tool: 'ohpm install', args: currentProject.path, status: 'done' as const, output: log, startedAt: Date.now(), durationMs: 0 },
          ],
        }))
      } catch (e) {
        note = `依赖安装失败：${String(e)}`
        alert(String(e))
      }
    } else if (card.action === 'open_sdk_manager' || card.action === 'open_signing_config') {
      // 这些需在 DevEco Studio 操作：打开工程目录并提示
      if (currentProject) await handleOpenTerminal()
      alert(t('home.diagnoseOpenDevEco'))
      completed = false
      note = '已打开工程目录，需用户在 DevEco Studio 中完成操作'
    } else {
      completed = true
    }
    dismissDiagnoseCard(card.id)
    void resolveDiagnoseCard(card.requestId, completed, note).catch(() => {})
  }

  /** 诊断卡片“稍后”：关闭卡片并告知 Agent 用户暂未操作，不阻塞任务结束 */
  const handleDiagnoseDismiss = (card: { requestId: string; id: string }) => {
    dismissDiagnoseCard(card.id)
    void resolveDiagnoseCard(card.requestId, false, '用户选择稍后处理').catch(() => {})
  }

  /** Agent 提问卡：提交回答（空回答=跳过）；后端唤醒挂起的 ask_user 工具继续任务 */
  const handleAskSubmit = () => {
    if (!askCard) return
    void resolveAskUser(askCard.requestId, askAnswer.trim())
    setAskAnswer('')
  }
  const handleAskSkip = () => {
    if (!askCard) return
    void resolveAskUser(askCard.requestId, '')
    setAskAnswer('')
  }

  /** 重新扫描工作区下的各类型模块（项目结构变化后手动刷新，保留手动绑定项） */
  const [rescanning, setRescanning] = useState(false)
  // 模块卡筛选：全部 / 鸿蒙 / 前端 / 后端 / 其它（混合工作区模块较多时按类别聚焦）
  const [moduleFilter, setModuleFilter] = useState<'all' | 'harmony' | 'frontend' | 'backend' | 'other'>('all')
  // 解析后的鸿蒙主工程根（模块卡“主工程”徽标）：跟随项目切换刷新
  const [mainRootAbs, setMainRootAbs] = useState<string | null>(null)
  // 模块扫描完成信号：右侧重扫/自动补扫成功后 +1，驱动下拉框候选与主工程徽标联动刷新
  const [moduleScanTick, setModuleScanTick] = useState(0)
  useEffect(() => {
    setMainRootAbs(null)
    if (!currentProject) return
    let cancelled = false
    getHarmonyRoot(currentProject.id)
      .then((r) => {
        if (!cancelled) setMainRootAbs(r.root)
      })
      .catch(() => {})
    return () => {
      cancelled = true
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [currentProject?.id, moduleScanTick])
  const handleRescanModules = async () => {
    if (!currentProject || rescanning) return
    setRescanning(true)
    try {
      await rescanWorkspaceModules(currentProject.id)
      await refreshProjects()
      // 通知工程分析面板重新解析候选、模块卡刷新主工程徽标
      setModuleScanTick((n) => n + 1)
    } catch (e) {
      alert(String(e))
    } finally {
      setRescanning(false)
    }
  }

  /** 手动编辑工作区模块（修改类型/增删） */
  const [editingModules, setEditingModules] = useState(false)
  const [moduleDraft, setModuleDraft] = useState<WorkspaceModule[]>([])
  const [savingModules, setSavingModules] = useState(false)
  const startEditModules = () => {
    if (!currentProject) return
    setModuleDraft(parseWorkspaceModules(currentProject.workspace_modules))
    setEditingModules(true)
  }
  const cancelEditModules = () => {
    setEditingModules(false)
    setModuleDraft([])
  }
  const addModuleRow = () => {
    setModuleDraft((d) => [...d, { rel_path: '', kind: 'unknown', name: '', manual: true }])
  }
  const updateModuleRow = (idx: number, patch: Partial<WorkspaceModule>) => {
    setModuleDraft((d) => d.map((m, i) => (i === idx ? { ...m, ...patch } : m)))
  }
  const removeModuleRow = (idx: number) => {
    setModuleDraft((d) => d.filter((_, i) => i !== idx))
  }
  const saveModules = async () => {
    if (!currentProject || savingModules) return
    // 规范化：路径非空，name 缺省取末级目录名
    const cleaned: WorkspaceModule[] = moduleDraft
      .map((m) => {
        const rel = m.rel_path.replace(/\\/g, '/').replace(/^\.\//, '').replace(/\/+$/, '').trim()
        const name = m.name?.trim() || rel.split('/').filter(Boolean).pop() || rel
        return { rel_path: rel, kind: m.kind, name, manual: true }
      })
      .filter((m) => m.rel_path && m.rel_path !== '.')
    setSavingModules(true)
    try {
      await setWorkspaceModules(currentProject.id, cleaned)
      await refreshProjects()
      setEditingModules(false)
    } catch (e) {
      alert(String(e))
    } finally {
      setSavingModules(false)
    }
  }

  /** Rules 弹窗：加载全局指令 + 项目级 rules（读取后端 settings / projects.rules） */
  const openRulesDialog = async () => {
    setRulesTab('global')
    setShowRulesDialog(true)
    try {
      const g = await getGlobalRules()
      setRulesGlobalText(g)
    } catch {
      setRulesGlobalText('')
    }
    setRulesProjectText(currentProject?.rules ?? '')
  }

  const saveRules = async () => {
    if (!currentProject) return
    setRulesSaving(true)
    try {
      await setGlobalRules(rulesGlobalText)
      await updateProjectRules(currentProject.id, rulesProjectText)
      await refreshProjects()
      setShowRulesDialog(false)
    } catch (e) {
      alert(`${t('home.rulesSaveFail')}: ${String(e)}`)
    } finally {
      setRulesSaving(false)
    }
  }

  /** 任务回滚：dry_run 预览 → confirm 确认 → git reset --hard 回起点 */
  const handleRollback = async () => {
    if (!currentConversation || rollbackBusy) return
    setRollbackBusy(true)
    try {
      const info = await rollbackTask(currentConversation.id, true)
      if (!info.is_repo) {
        alert(t('home.rollbackNoRepo'))
        return
      }
      const ok = window.confirm(
        t('home.rollbackConfirm', {
          date: info.commit_date || '-',
          changed: String(info.changed),
          untracked: String(info.untracked),
        }),
      )
      if (!ok) return
      const res = await rollbackTask(currentConversation.id, false)
      alert(t('home.rollbackDone', { commit: res.commit.slice(0, 8), date: res.commit_date || '-' }))
    } catch (e) {
      alert(`${t('home.rollbackFail')}\n${String(e)}`)
    } finally {
      setRollbackBusy(false)
    }
  }

  // ---------- 划词菜单：选中文本后弹出（复制/解释/翻译/搜索/引用回复） ----------
  // 拖拽进行中：selectionchange 持续保存"更长"的完整选区快照。
  // WebView2 内核只在 mouseup 后把跨表格选区端点归一化（截断），拖拽过程中的选区始终完整，
  // 因此最后一次完整快照能覆盖任意时机的内核截断（mouseup 后截断版更短，不会覆盖快照）。
  useEffect(() => {
    const onDown = () => {
      dragActiveRef.current = true
      // 新一次拖拽开始：清空旧快照，避免残留上一次划词的完整选区
      captureRangeRef.current = null
      captureTextRef.current = ''
      selectionTextRef.current = ''
      selectionContainerRef.current = null
    }
    document.addEventListener('mousedown', onDown, true)
    const onChange = () => {
      if (!dragActiveRef.current) return
      const sel = window.getSelection()
      if (!sel || sel.isCollapsed || sel.rangeCount === 0) return
      const range = sel.getRangeAt(0)
      const text = range.toString()
      // 只保存更长的快照：拖拽中选区单调增长（完整），mouseup 后内核提交的截断版更短，不会覆盖
      if (text.length > captureTextRef.current.length) {
        captureRangeRef.current = range.cloneRange()
        captureTextRef.current = text
      }
    }
    document.addEventListener('selectionchange', onChange, true)
    return () => {
      document.removeEventListener('mousedown', onDown, true)
      document.removeEventListener('selectionchange', onChange, true)
    }
  }, [])

  // capture 阶段 mouseup：标记拖拽结束（阻止内核截断的 selectionchange 覆盖完整快照），
  // 并兜底保存——若释放点命中的内容在选区终点之后（内核已截断），扩展端点到命中内容末尾，
  // 仅在比已有快照更长时覆盖（正常情况下 selectionchange 已保存完整快照）
  useEffect(() => {
    const handler = (e: MouseEvent) => {
      dragActiveRef.current = false
      const sel = window.getSelection()
      if (!sel || sel.isCollapsed || sel.rangeCount === 0) return
      let range = sel.getRangeAt(0)
      const hit = document.elementFromPoint(e.clientX, e.clientY)
      const target = hit?.closest?.('td,th,p,li,ul,ol,table,h1,h2,h3,h4,h5,h6,blockquote,pre,code')
      if (target && range.comparePoint(target, 0) === 1) {
        const walker = document.createTreeWalker(target, NodeFilter.SHOW_TEXT)
        let last: Node | null = null
        while (walker.nextNode()) last = walker.currentNode
        if (last && last.textContent!.length > 0) {
          const fixed = range.cloneRange()
          fixed.setEnd(last, last.textContent!.length)
          try {
            sel.removeAllRanges()
            sel.addRange(fixed)
            range = fixed
          } catch {
            // 节点失效时静默跳过，保留已保存的完整快照
          }
        }
      }
      if (range.toString().length > captureTextRef.current.length) {
        captureRangeRef.current = range.cloneRange()
        captureTextRef.current = range.toString()
      }
    }
    document.addEventListener('mouseup', handler, true)
    return () => document.removeEventListener('mouseup', handler, true)
  }, [])

  useEffect(() => {
    const handler = () => {
      const sel = window.getSelection()
      // 优先用 capture 阶段保存的完整文本/快照（内核截断发生在 capture 与 bubble 之间）
      const raw = captureTextRef.current || sel?.toString() || ''
      const text = raw.trim()
      if (text && text.length > 1 && text.length <= 500) {
        const range = captureRangeRef.current ?? sel!.getRangeAt(0)
        // 保存选区快照（点击菜单按钮后选区可能被浏览器清除，供操作前恢复高亮）
        selectionRangeRef.current = range.cloneRange()
        // 同步保存完整文本与容器：消息 DOM 被重建后 live Range 端点归一化收缩，
        // 恢复时若检测到文本变短，按文本在容器内重新定位端点
        selectionTextRef.current = raw
        let anchor: Node | null = range.commonAncestorContainer
        if (anchor.nodeType === Node.TEXT_NODE) anchor = anchor.parentElement
        selectionContainerRef.current = (anchor as Element | null)?.closest?.('.md-body') ?? anchor
        const rect = range.getBoundingClientRect()
        // 菜单出现在选区上方居中（底部越界时上移，顶部越界时下移）
        let y = rect.top - 44
        if (y < 8) y = rect.bottom + 8
        setSelectionMenu({ x: Math.max(8, Math.min(window.innerWidth - 340, rect.left + rect.width / 2)), y, text })
      } else {
        setSelectionMenu(null)
      }
    }
    document.addEventListener('mouseup', handler)
    return () => document.removeEventListener('mouseup', handler)
  }, [])

  // 菜单渲染完成后恢复选区高亮：菜单渲染期间 DOM 更新可能让跨格式选区收缩
  //（只剩第一个格式块），渲染提交后再恢复一次，下一帧再补一次（rAF 双保险）
  useEffect(() => {
    if (!selectionMenu) return
    const raf = requestAnimationFrame(() => {
      restoreSelectionRange(selectionRangeRef.current, selectionTextRef.current, selectionContainerRef.current)
      requestAnimationFrame(() =>
        restoreSelectionRange(selectionRangeRef.current, selectionTextRef.current, selectionContainerRef.current)
      )
    })
    // WebView2 拖拽状态机在 mouseup 后约 600ms 内会持续覆盖选区（跨表格拖拽截断），
    // rAF 恢复可能被覆盖；延迟 800ms 再恢复一次，保证跨表格选区完整显示
    const timer = setTimeout(
      () => restoreSelectionRange(selectionRangeRef.current, selectionTextRef.current, selectionContainerRef.current),
      800
    )
    return () => {
      cancelAnimationFrame(raf)
      clearTimeout(timer)
    }
  }, [selectionMenu])

  /** 恢复划词选区高亮（基于 mouseup 时保存的完整快照，供菜单操作前/渲染后恢复） */
  const restoreSelection = () =>
    restoreSelectionRange(selectionRangeRef.current, selectionTextRef.current, selectionContainerRef.current)

  /** 划词操作：复制（先恢复选区高亮，复制后保留选区，便于确认复制内容） */
  const copySelection = () => {
    if (!selectionMenu) return
    restoreSelection()
    navigator.clipboard.writeText(selectionMenu.text).catch(() => {})
    setSelectionMenu(null)
  }

  /** 划词操作：带指令发送（解释/翻译） */
  const sendWithInstruction = async (instruction: string, text: string) => {
    restoreSelection()
    setSelectionMenu(null)
    const quote = text
      .split('\n')
      .map((l) => `> ${l}`)
      .join('\n')
    if (!currentConversation) {
      await newConversation()
    }
    await sendUserMessage(`${instruction}\n\n${quote}`, modelOptions)
    setDraft('')
  }

  /** 划词操作：搜索（系统浏览器打开） */
  const searchSelection = async () => {
    if (!selectionMenu) return
    restoreSelection()
    setSelectionMenu(null)
    const { open } = await import('@tauri-apps/plugin-shell')
    open(`https://www.bing.com/search?q=${encodeURIComponent(selectionMenu.text)}`).catch(() => {})
  }

  /** 划词操作：引用回复（把选中文本作为引用插入输入框） */
  const quoteSelection = () => {
    if (!selectionMenu) return
    restoreSelection()
    const quote = selectionMenu.text
      .split('\n')
      .map((l) => `> ${l}`)
      .join('\n')
    setDraft((d) => (d ? `${d}\n\n${quote}\n\n` : `${quote}\n\n`))
    setSelectionMenu(null)
    inputRef.current?.focus()
  }

  /** 语音朗读（Web Speech API；再次点击停止） */
  const toggleSpeak = (messageId: string, text: string) => {
    if (!('speechSynthesis' in window)) return
    if (speakingId === messageId) {
      window.speechSynthesis.cancel()
      setSpeakingId(null)
      return
    }
    window.speechSynthesis.cancel()
    const clean = text.replace(/```[\s\S]*?```/g, '（代码块省略）').replace(/[#*`>_~|]/g, '').slice(0, 4000)
    const utter = new SpeechSynthesisUtterance(clean)
    const zh = window.speechSynthesis.getVoices().find((v) => v.lang.toLowerCase().startsWith('zh'))
    if (zh) utter.voice = zh
    utter.lang = zh?.lang ?? 'zh-CN'
    utter.onend = () => setSpeakingId((cur) => (cur === messageId ? null : cur))
    utter.onerror = () => setSpeakingId((cur) => (cur === messageId ? null : cur))
    setSpeakingId(messageId)
    window.speechSynthesis.speak(utter)
  }

  /** 导出会话：下载 md/txt/html 或复制全文 */
  const exportConversationFile = (format: 'md' | 'txt' | 'html') => {
    const store = useProjectStore.getState()
    const text = store.exportConversation(format)
    const ext = format === 'html' ? 'html' : format === 'txt' ? 'txt' : 'md'
    const blob = new Blob([text], { type: format === 'html' ? 'text/html;charset=utf-8' : 'text/plain;charset=utf-8' })
    const url = URL.createObjectURL(blob)
    const a = document.createElement('a')
    a.href = url
    a.download = `${currentConversation?.title ?? '对话记录'}.${ext}`
    a.click()
    setTimeout(() => URL.revokeObjectURL(url), 1000)
    setShowExportMenu(false)
  }

  /** 复制会话全文 */
  const copyConversation = () => {
    const store = useProjectStore.getState()
    navigator.clipboard.writeText(store.exportConversation('md')).catch(() => {})
    setShowExportMenu(false)
  }

  /** 记忆总结：请求 LLM 生成草稿 → 弹窗确认保存 */
  const handleSummarizeMemory = async () => {
    if (!currentConversation) return
    const draft = await useProjectStore.getState().summarizeMemory(currentConversation.id)
    setMemoryDraft(draft)
  }

  /** 手动压缩会话历史（较早消息总结为摘要，保留最近 10 条） */
  const [compacting, setCompacting] = useState(false)
  const handleCompact = async () => {
    if (!currentConversation || compacting) return
    setCompacting(true)
    try {
      const summary = await compactConversation(currentConversation.id, 10)
      alert(`${t('home.compactDone')}\n\n${summary}`)
      await useProjectStore.getState().openConversation(currentConversation.id).catch(() => {})
    } catch (e) {
      alert(`${t('home.compactFail')}: ${String(e)}`)
    } finally {
      setCompacting(false)
    }
  }

  /** 确认保存记忆草稿 */
  const confirmSaveMemory = async () => {
    if (!memoryDraft) return
    await saveMemory(memoryDraft)
    setMemoryDraft(null)
  }

  /** 打开版本 diff 弹窗（assistant 消息操作栏） */
  const openVersionDialog = (message: ChatMessage, userMessageId: string) => {
    setVersionDialog({ userMessageId, current: message.content })
  }

  /** 会话重命名提交 */
  const submitRename = async (id: string) => {
    const title = renamingText.trim()
    setRenamingId(null)
    if (title) {
      await renameConversation(id, title)
    }
  }

  /** 会话删除（二次确认） */
  const handleDeleteConversation = async (id: string) => {
    if (confirmDeleteId !== id) {
      setConfirmDeleteId(id)
      setTimeout(() => setConfirmDeleteId((cur) => (cur === id ? null : cur)), 3000)
      return
    }
    setConfirmDeleteId(null)
    await deleteConversation(id)
  }

  /** 置顶 / 取消置顶 */
  const togglePin = async (id: string, pinned: boolean) => {
    await pinConversation(id, !pinned)
  }

  /** 归档 / 取消归档（归档后按当前视图刷新） */
  const toggleArchive = async (id: string, archived: boolean) => {
    await archiveConversation(id, !archived)
  }

  /** 切换归档视图：重新拉取对应列表 */
  const switchArchiveView = async () => {
    if (!currentProject) return
    const next = !showArchived
    setShowArchived(next)
    // 归档视图同样保持搜索关键字过滤
    const kw = useProjectStore.getState().conversationKeyword.trim()
    const list = await listConversations(currentProject.id, next, kw).catch(() => [])
    useProjectStore.setState({ conversations: list })
  }

  /** 输入框拖拽调高 */
  const onDragStart = (e: React.PointerEvent) => {
    dragRef.current = { startY: e.clientY, startH: inputHeight }
    e.currentTarget.setPointerCapture(e.pointerId)
  }
  const onDragMove = (e: React.PointerEvent) => {
    if (!dragRef.current) return
    const h = Math.min(360, Math.max(64, dragRef.current.startH + (dragRef.current.startY - e.clientY)))
    setInputHeight(h)
  }
  const onDragEnd = () => {
    dragRef.current = null
  }

  /** 左侧栏拖拽调宽（拖右边缘） */
  const onSidebarDragStart = (e: React.PointerEvent) => {
    sidebarDragRef.current = { startX: e.clientX, startW: sidebarWidth }
    setResizing('sidebar')
    e.currentTarget.setPointerCapture(e.pointerId)
  }
  const onSidebarDragMove = (e: React.PointerEvent) => {
    if (!sidebarDragRef.current) return
    const w = Math.min(420, Math.max(180, sidebarDragRef.current.startW + (e.clientX - sidebarDragRef.current.startX)))
    setSidebarWidth(w)
  }
  const onSidebarDragEnd = () => {
    sidebarDragRef.current = null
    setResizing(null)
    localStorage.setItem('deveco-switch-sidebar-width', String(sidebarWidth))
  }

  /** 右侧栏拖拽调宽（拖左边缘，向右拖变窄） */
  const onRightDragStart = (e: React.PointerEvent) => {
    rightDragRef.current = { startX: e.clientX, startW: rightWidth }
    setResizing('right')
    e.currentTarget.setPointerCapture(e.pointerId)
  }
  const onRightDragMove = (e: React.PointerEvent) => {
    if (!rightDragRef.current) return
    // 上限随窗口尺寸动态计算：最大占窗口 65%（封顶 900px），避免大屏拖到 520 就卡住
    const max = Math.min(900, Math.max(360, Math.floor(window.innerWidth * 0.65)))
    const w = Math.min(
      max,
      Math.max(240, rightDragRef.current.startW + (rightDragRef.current.startX - e.clientX)),
    )
    setRightWidth(w)
  }
  const onRightDragEnd = () => {
    rightDragRef.current = null
    setResizing(null)
    localStorage.setItem('deveco-switch-right-width', String(rightWidth))
  }

  const formatTime = (ts: number) => {
    const d = new Date(ts * 1000)
    return d.toLocaleString(undefined, { month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit' })
  }

  // token 数缩写（1.2k / 3.4w），标题下累计展示用
  const fmtTokens = (n: number) =>
    n >= 10000 ? `${(n / 10000).toFixed(1)}w` : n >= 1000 ? `${(n / 1000).toFixed(1)}k` : String(n)

  // 任务计时 + 静默检测：每秒刷新。任务在任意会话运行时都持续计时（切走会话不影响），
  // 会话列表对运行中的会话显示“已运行 mm:ss”；静默超阈值时消息流提示模型仍在工作
  useEffect(() => {
    if (!streaming.startedAt) {
      setTaskElapsed(0)
      setSilentSeconds(0)
      return
    }
    setTaskElapsed(Math.floor((Date.now() - streaming.startedAt) / 1000))
    const timer = setInterval(() => {
      const s = useProjectStore.getState()
      if (!s.streaming.startedAt) {
        setTaskElapsed(0)
        setSilentSeconds(0)
        return
      }
      setTaskElapsed(Math.floor((Date.now() - s.streaming.startedAt) / 1000))
      const ref = s.streaming.lastDeltaAt ?? s.streaming.startedAt
      setSilentSeconds(Math.floor((Date.now() - ref) / 1000))
    }, 1000)
    return () => clearInterval(timer)
  }, [streaming.startedAt])

  // 概览面板：切换到 overview 时刷新最近任务（task_runs 明细）
  useEffect(() => {
    if (rightTab === 'overview' && currentProject) void loadRecentRuns()
  }, [rightTab, currentProject?.id, loadRecentRuns])

  // 当前选择的模型标签（输入区按钮展示，默认跟随 Provider）
  const currentModelLabel = (() => {
    if (modelOptions.model_id) {
      for (const g of modelCatalog) {
        const m = g.models.find((x) => x.id === modelOptions.model_id)
        if (m) return m.display_name ?? m.model_id
      }
      return modelOptions.model_id
    }
    return t('provider.modelDefault')
  })()

  return (
    <div className="flex h-screen w-screen overflow-hidden bg-[var(--bg-window)]">
      {/* ============ 左侧栏：项目 + 会话 ============ */}
      <aside
        style={{ width: sidebarCollapsed ? 56 : sidebarWidth }}
        className={`shrink-0 bg-[var(--bg-secondary)] border-r border-[var(--border)] flex flex-col overflow-hidden ${resizing === 'sidebar' ? '' : 'transition-[width] duration-200 ease-out'}`}
      >
        <div
          style={{ width: sidebarCollapsed ? 56 : sidebarWidth }}
          className={`h-full flex flex-col min-h-0 ${resizing === 'sidebar' ? '' : 'transition-[width] duration-200 ease-out'}`}
        >
        {/* Logo */}
        <div className={`h-14 shrink-0 flex items-center gap-2.5 ${sidebarCollapsed ? 'justify-center px-2' : 'px-4'}`}>
          <div className="w-8 h-8 rounded-[10px] bg-gradient-to-br from-[var(--accent)] to-[#8b5cf6] flex items-center justify-center shadow-lg shadow-[var(--accent)]/25 shrink-0">
            <Icon name="bolt" size={17} white />
          </div>
          <div className={`min-w-0 leading-tight ${sidebarCollapsed ? 'hidden' : ''}`}>
            <div className="text-[13px] font-semibold tracking-wide">DevEco Switch</div>
            <div className="text-[10px] text-[var(--text-muted)]">Agent Workspace</div>
          </div>
        </div>

        {/* 添加项目 */}
        <div className="px-3 pb-3 shrink-0">
          <button
            onClick={() => setShowAddDialog(true)}
            title={t('home.addProject')}
            className={`h-9 flex items-center justify-center gap-1.5 rounded-[10px] bg-[var(--accent)] text-white text-[13px] font-medium hover:bg-[var(--accent-hover)] active:scale-[0.98] transition-all shadow-lg shadow-[var(--accent)]/15 ${sidebarCollapsed ? 'w-9 mx-auto' : 'w-full'}`}
          >
            <Icon name="plus" size={15} white /> {!sidebarCollapsed && t('home.addProject')}
          </button>
        </div>

        {/* 最近项目 */}
        <div className={`shrink-0 px-3 ${sidebarCollapsed ? 'pb-3' : 'pb-2'}`}>
          {!sidebarCollapsed && (
            <div className="px-2 pb-1.5 text-[11px] font-medium text-[var(--text-muted)]">{t('home.recentProjects')}</div>
          )}
          {projects.length === 0 ? (
            !sidebarCollapsed && (
              <div className="px-2 py-4 text-[11px] text-[var(--text-muted)] text-center leading-relaxed whitespace-pre-line">
                {t('home.noProject')}
              </div>
            )
          ) : (
            <div className={`space-y-0.5 pr-0.5 ${sidebarCollapsed ? 'flex flex-col items-center' : 'max-h-52 overflow-y-auto'}`}>
              {projects.map((p) => {
                const active = p.id === currentProject?.id
                const counts = scopedCounts[p.id]
                const total = counts ? counts.mcp + counts.skills : 0
                return (
                  <button
                    key={p.id}
                    onClick={() => openProject(p.id)}
                    title={p.name}
                    className={`group relative flex items-center gap-2 rounded-lg text-left transition-colors ${
                      sidebarCollapsed ? 'w-9 h-9 justify-center' : 'w-full pl-3 pr-2 py-[7px]'
                    } ${active ? 'bg-[var(--accent-soft)]' : 'hover:bg-[var(--bg-hover)]'}`}
                  >
                    {active && !sidebarCollapsed && (
                      <span className="absolute left-0 top-1/2 -translate-y-1/2 w-[3px] h-4 rounded-full bg-[var(--accent)]" />
                    )}
                    {active && sidebarCollapsed && (
                      <span className="absolute left-1 top-1/2 -translate-y-1/2 w-[3px] h-4 rounded-full bg-[var(--accent)]" />
                    )}
                    <Icon name="folder" size={15} className={`shrink-0 ${active ? '' : 'opacity-60'}`} />
                    {!sidebarCollapsed && (
                      <>
                        <span
                          className={`flex-1 text-[13px] truncate ${
                            active ? 'text-[var(--text-primary)]' : 'text-[var(--text-secondary)] group-hover:text-[var(--text-primary)]'
                          }`}
                        >
                          {p.name}
                        </span>
                        {total > 0 && (
                          <span
                            className="shrink-0 min-w-[16px] h-4 px-1 flex items-center justify-center rounded-full bg-[var(--accent-soft)] text-[var(--accent)] text-[10px] font-medium"
                            title={t('home.scopedCountHint', { mcp: counts!.mcp, skills: counts!.skills })}
                          >
                            {total}
                          </span>
                        )}
                        <span
                          className={`shrink-0 w-1.5 h-1.5 rounded-full ${p.trusted ? 'bg-[var(--success)]' : 'bg-[var(--warning)]'} ${
                            active ? '' : 'opacity-0 group-hover:opacity-60'
                          } transition-opacity`}
                          title={p.trusted ? t('home.trusted') : t('home.untrusted')}
                        />
                      </>
                    )}
                  </button>
                )
              })}
            </div>
          )}
        </div>

        {/* 会话列表 */}
        <div className="flex-1 flex flex-col min-h-0 mt-1">
          <div className={`flex items-center justify-between pb-1.5 ${sidebarCollapsed ? 'px-3 justify-center' : 'px-4'}`}>
            {!sidebarCollapsed && (
              <span className="text-[11px] font-medium text-[var(--text-muted)]">
                {showArchived ? t('home.archived') : t('home.conversations')}
              </span>
            )}
            {currentProject && !sidebarCollapsed && (
              <div className="flex items-center gap-0.5">
                <button
                  onClick={switchArchiveView}
                  className={`p-1 rounded-md transition-colors ${
                    showArchived
                      ? 'text-[var(--accent)] bg-[var(--accent-soft)]'
                      : 'text-[var(--text-muted)] hover:text-[var(--accent)] hover:bg-[var(--bg-hover)]'
                  }`}
                  title={showArchived ? t('home.backToConversations') : t('home.viewArchived')}
                >
                  <Icon name="archive" size={14} />
                </button>
                <button
                  onClick={() => newConversation()}
                  className="p-1 rounded-md text-[var(--text-muted)] hover:text-[var(--accent)] hover:bg-[var(--bg-hover)] transition-colors"
                  title={t('home.newConversation')}
                >
                  <Icon name="plus" size={14} />
                </button>
              </div>
            )}
          </div>
          {/* 会话/消息搜索（Ctrl+K 聚焦；conv 模式匹配标题/首条消息，msg 模式全文检索消息正文） */}
          {currentProject && !sidebarCollapsed && (
            <div className="px-3 pb-1.5">
              <div className="relative">
                <Icon
                  name="search"
                  size={12}
                  className="absolute left-2.5 top-1/2 -translate-y-1/2 text-[var(--text-muted)] pointer-events-none"
                />
                <input
                  ref={searchInputRef}
                  value={searchText}
                  onChange={(e) => setSearchText(e.target.value)}
                  placeholder={
                    searchMode === 'msg'
                      ? t('home.messageSearchPlaceholder')
                      : `${t('home.searchPlaceholder')} (Ctrl+K)`
                  }
                  className="w-full h-7 pl-7 pr-6 rounded-lg bg-[var(--bg-secondary)] border border-[var(--border)] text-[11.5px] text-[var(--text-primary)] placeholder:text-[var(--text-muted)] outline-none focus:border-[var(--accent)] transition-colors"
                />
                {searchText && (
                  <button
                    onClick={() => {
                      setSearchText('')
                      setMsgHits([])
                      if (searchMode === 'conv') void setConversationKeyword('')
                    }}
                    className="absolute right-1.5 top-1/2 -translate-y-1/2 p-0.5 rounded text-[var(--text-muted)] hover:text-[var(--text-primary)]"
                    title={t('home.clearSearch')}
                  >
                    <Icon name="close" size={12} />
                  </button>
                )}
              </div>
              {/* 范围切换：会话 / 消息 */}
              <div className="flex gap-1 mt-1">
                <button
                  onClick={() => {
                    setSearchMode('conv')
                    setMsgHits([])
                  }}
                  className={`flex-1 h-6 rounded-md text-[10.5px] transition-colors ${
                    searchMode === 'conv'
                      ? 'bg-[var(--accent-soft)] text-[var(--accent)] font-medium'
                      : 'text-[var(--text-muted)] hover:text-[var(--text-secondary)] hover:bg-[var(--bg-hover)]'
                  }`}
                >
                  {t('home.searchModeConv')}
                </button>
                <button
                  onClick={() => {
                    setSearchMode('msg')
                    void setConversationKeyword('')
                  }}
                  className={`flex-1 h-6 rounded-md text-[10.5px] transition-colors ${
                    searchMode === 'msg'
                      ? 'bg-[var(--accent-soft)] text-[var(--accent)] font-medium'
                      : 'text-[var(--text-muted)] hover:text-[var(--text-secondary)] hover:bg-[var(--bg-hover)]'
                  }`}
                >
                  {t('home.searchModeMsg')}
                </button>
              </div>
              {/* 消息全文搜索结果下拉 */}
              {searchMode === 'msg' && searchText.trim().length >= 2 && (
                <div className="mt-1.5 max-h-72 overflow-y-auto rounded-lg border border-[var(--border)] bg-[var(--bg-card)] shadow-lg shadow-black/20 py-1">
                  {msgSearching && msgHits.length === 0 && (
                    <div className="px-2.5 py-2 text-[11px] text-[var(--text-muted)]">{t('home.searching')}</div>
                  )}
                  {!msgSearching && msgHits.length === 0 && (
                    <div className="px-2.5 py-2 text-[11px] text-[var(--text-muted)]">{t('home.noMessageHits')}</div>
                  )}
                  {msgHits.slice(0, 30).map((hit) => (
                    <button
                      key={hit.message_id}
                      onClick={() => void openMessageHit(hit)}
                      className="w-full text-left px-2.5 py-1.5 hover:bg-[var(--bg-hover)] transition-colors border-b border-[var(--border)]/50 last:border-0"
                    >
                      <div className="flex items-center gap-1.5 mb-0.5">
                        <span
                          className={`w-1 h-1 rounded-full shrink-0 ${
                            hit.role === 'user' ? 'bg-[var(--accent)]' : 'bg-[var(--success)]'
                          }`}
                        />
                        <span className="text-[10.5px] font-medium text-[var(--text-secondary)] truncate flex-1">
                          {hit.conversation_title}
                        </span>
                        <span className="text-[9.5px] text-[var(--text-muted)] shrink-0">{formatTime(hit.created_at)}</span>
                      </div>
                      <div className="text-[10.5px] text-[var(--text-muted)] leading-relaxed line-clamp-2">
                        {highlightSnippet(hit.snippet)}
                      </div>
                    </button>
                  ))}
                </div>
              )}
            </div>
          )}
          <div className={`flex-1 overflow-y-auto pb-2 space-y-0.5 ${sidebarCollapsed ? 'px-2' : 'px-2'}`}>
            {conversations.length === 0 && !sidebarCollapsed && (
              <p className="text-[11px] text-[var(--text-muted)] px-2 py-4 text-center leading-relaxed">
                {currentProject ? t('home.noConversation') : t('home.selectProjectFirst')}
              </p>
            )}
            {conversations.map((c) => {
              const active = c.id === currentConversation?.id
              const renaming = renamingId === c.id
              return (
                <div
                  key={c.id}
                  className={`group w-full flex items-center rounded-lg transition-colors ${
                    active ? 'bg-[var(--bg-card)]' : 'hover:bg-[var(--bg-hover)]'
                  } ${sidebarCollapsed ? 'justify-center py-1' : 'pl-2.5 pr-1.5 py-1.5'}`}
                >
                  {renaming ? (
                    <input
                      autoFocus
                      value={renamingText}
                      onChange={(e) => setRenamingText(e.target.value)}
                      onKeyDown={(e) => {
                        if (e.key === 'Enter') submitRename(c.id)
                        if (e.key === 'Escape') setRenamingId(null)
                      }}
                      onBlur={() => submitRename(c.id)}
                      className="flex-1 min-w-0 bg-[var(--bg-primary)] border border-[var(--accent)] rounded-md px-2 py-1 text-[13px] outline-none"
                    />
                  ) : sidebarCollapsed ? (
                    <button
                      onClick={() => openConversation(c.id)}
                      title={c.title}
                      className={`w-8 h-8 flex items-center justify-center rounded-lg ${active ? 'text-[var(--accent)]' : 'text-[var(--text-secondary)]'}`}
                    >
                      <Icon name="chat" size={15} />
                    </button>
                  ) : (
                    <>
                      <button onClick={() => openConversation(c.id)} className="flex-1 min-w-0 text-left">
                        <span
                          className={`block text-[13px] truncate ${c.is_pinned ? 'text-[var(--accent)]' : active ? 'text-[var(--text-primary)]' : 'text-[var(--text-secondary)] group-hover:text-[var(--text-primary)]'}`}
                        >
                          {c.is_pinned && <Icon name="pin" size={11} className="mr-1 text-[var(--accent)] align-[-1px]" />}
                          {c.title}
                        </span>
                        {streaming.conversationId === c.id ? (
                          <span className="flex items-center gap-1.5 text-[11px] text-[var(--accent)] mt-0.5 tabular-nums">
                            <span className="w-1.5 h-1.5 rounded-full bg-[var(--accent)] animate-pulse shrink-0" />
                            {t('home.taskElapsed', { time: fmtElapsed(taskElapsed) })}
                          </span>
                        ) : (
                          <span className="flex items-center gap-1.5 mt-0.5">
                            <span className="text-[11px] text-[var(--text-muted)]">{formatTime(c.updated_at)}</span>
                            {!active && (unreadMap[c.id] ?? 0) > 0 && (
                              <span className="min-w-[16px] h-[16px] px-1 flex items-center justify-center rounded-full bg-[var(--accent)] text-white text-[9.5px] font-semibold leading-none">
                                {unreadMap[c.id] > 99 ? '99+' : unreadMap[c.id]}
                              </span>
                            )}
                          </span>
                        )}
                      </button>
                      <button
                        onClick={(e) => {
                          e.stopPropagation()
                          togglePin(c.id, c.is_pinned)
                        }}
                        className={`p-1 rounded-md transition-all shrink-0 ${
                          c.is_pinned
                            ? 'text-[var(--accent)] opacity-100'
                            : 'text-[var(--text-muted)] opacity-0 group-hover:opacity-100 hover:text-[var(--accent)] hover:bg-[var(--bg-hover)]'
                        }`}
                        title={c.is_pinned ? t('home.unpin') : t('home.pin')}
                      >
                        <Icon name="pin" size={13} />
                      </button>
                      <button
                        onClick={(e) => {
                          e.stopPropagation()
                          toggleArchive(c.id, c.archived)
                        }}
                        className="p-1 rounded-md text-[var(--text-muted)] opacity-0 group-hover:opacity-100 hover:text-[var(--accent)] hover:bg-[var(--bg-hover)] transition-all shrink-0"
                        title={c.archived ? t('home.unarchive') : t('home.archive')}
                      >
                        <Icon name="archive" size={13} />
                      </button>
                      <button
                        onClick={(e) => {
                          e.stopPropagation()
                          setRenamingId(c.id)
                          setRenamingText(c.title)
                        }}
                        className="p-1 rounded-md text-[var(--text-muted)] opacity-0 group-hover:opacity-100 hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] transition-all shrink-0"
                        title={t('home.rename')}
                      >
                        <Icon name="edit" size={13} />
                      </button>
                      <button
                        onClick={(e) => {
                          e.stopPropagation()
                          handleDeleteConversation(c.id)
                        }}
                        className={`p-1 rounded-md transition-all shrink-0 ${
                          confirmDeleteId === c.id
                            ? 'text-[var(--danger)] bg-[var(--danger)]/10 opacity-100'
                            : 'text-[var(--text-muted)] opacity-0 group-hover:opacity-100 hover:text-[var(--danger)] hover:bg-[var(--bg-hover)]'
                        }`}
                        title={t('home.deleteConversation')}
                      >
                        <Icon name="delete" size={13} />
                      </button>
                    </>
                  )}
                  {active && !sidebarCollapsed && !renaming && (
                    <span className="ml-1.5 w-1.5 h-1.5 rounded-full bg-[var(--accent)] shrink-0" />
                  )}
                </div>
              )
            })}
          </div>
        </div>

        {/* 底部：设置 + 主题 + 折叠 */}
        <div className={`p-2 border-t border-[var(--border)] flex gap-1 ${sidebarCollapsed ? 'flex-col items-center' : 'items-center'}`}>
          <div className="relative flex-1" ref={settingsRef}>
            <button
              onClick={() => setShowSettingsMenu((v) => !v)}
              title={t('home.settings')}
              className={`flex items-center gap-2 px-2 py-1.5 rounded-lg text-[13px] text-[var(--text-secondary)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] transition-colors ${sidebarCollapsed ? 'w-9 h-9 justify-center' : 'w-full'} ${showSettingsMenu ? 'text-[var(--accent)] bg-[var(--accent-soft)]' : ''}`}
            >
              <Icon name="settings" size={15} />
              {!sidebarCollapsed && t('home.settings')}
            </button>
            {showSettingsMenu && (
              <div
                className={`absolute bottom-full mb-1.5 rounded-xl border border-[var(--border)] bg-[var(--bg-card)] shadow-2xl shadow-black/40 py-1 z-50 animate-modal-in ${sidebarCollapsed ? 'left-0' : 'left-0'} w-52`}
              >
                <div className="px-3 py-1.5 text-[10px] font-medium text-[var(--text-muted)] border-b border-[var(--border)]">
                  {t('home.settings')}
                </div>
                {settingsItems.map((item) => (
                  <button
                    key={item.path}
                    onClick={() => {
                      setShowSettingsMenu(false)
                      navigate(item.path)
                    }}
                    className="w-full flex items-center gap-2.5 px-3 py-2 text-[12px] text-[var(--text-secondary)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] transition-colors"
                  >
                    <Icon name={item.icon} size={14} />
                    <span className="flex-1 text-left">{t(item.labelKey)}</span>
                    {item.path === '/health' && envHealth && envHealth !== 'ok' && (
                      <span
                        className={`w-2 h-2 rounded-full shrink-0 ${envHealth === 'bad' ? 'bg-[var(--danger)]' : 'bg-[var(--warning)]'}`}
                        title={envHealth === 'bad' ? t('home.envHealthBad') : t('home.envHealthWarn')}
                      />
                    )}
                  </button>
                ))}
              </div>
            )}
          </div>
          <button
            onClick={toggleTheme}
            title={t('home.theme')}
            className={`p-2 rounded-lg text-[var(--text-secondary)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] transition-colors ${sidebarCollapsed ? 'w-9 h-9 flex items-center justify-center' : ''}`}
          >
            <Icon name={theme === 'dark' ? 'sun' : 'moon'} size={15} />
          </button>
          {currentProject && !sidebarCollapsed && (
            <span
              className={`w-2 h-2 rounded-full shrink-0 ${currentProject.trusted ? 'bg-[var(--success)]' : 'bg-[var(--warning)]'}`}
              title={currentProject.trusted ? t('home.trusted') : t('home.untrusted')}
            />
          )}
          <button
            onClick={() =>
              setSidebarCollapsed((v) => {
                const next = !v
                localStorage.setItem('deveco-switch-sidebar-collapsed', next ? '1' : '0')
                return next
              })
            }
            title={sidebarCollapsed ? t('home.expandSidebar') : t('home.collapseSidebar')}
            className={`p-2 rounded-lg text-[var(--text-secondary)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] transition-colors ${sidebarCollapsed ? 'w-9 h-9 flex items-center justify-center mt-1' : ''}`}
          >
            <Icon name={sidebarCollapsed ? 'chevron-right' : 'chevron-left'} size={15} />
          </button>
        </div>
        </div>
      </aside>

      {/* 左侧拖拽手柄：调整左侧栏宽度 */}
      {!sidebarCollapsed && (
        <div
          onPointerDown={onSidebarDragStart}
          onPointerMove={onSidebarDragMove}
          onPointerUp={onSidebarDragEnd}
          title={t('home.dragSidebar')}
          className="w-1 shrink-0 cursor-col-resize bg-transparent hover:bg-[var(--accent)]/50 active:bg-[var(--accent)]/50 transition-colors touch-none select-none"
        />
      )}

      {/* ============ 中间：对话区 ============ */}
      <main className="flex-1 flex flex-col min-w-0 bg-[var(--bg-primary)]">
        {/* 顶部栏 */}
        <header className="glass-bar h-14 shrink-0 border-b border-[var(--border)] flex items-center justify-between px-4 z-20">
          <div className="flex items-center gap-2.5 min-w-0">
            <div className="w-7 h-7 rounded-lg bg-gradient-to-br from-[var(--accent)] to-[#8b5cf6] flex items-center justify-center brand-glow shrink-0">
              <Icon name="bolt" size={14} white />
            </div>
            <span className="text-[13.5px] font-medium truncate">
              {currentConversation?.title ?? (currentProject ? currentProject.name : t('home.welcome'))}
            </span>
            {/* 会话内 token/成本累计（会话打开时加载，任务结束后刷新） */}
            {currentConversation && tokenStats && tokenStats.messages_count > 0 && (
              <span
                className="shrink-0 text-[10px] tabular-nums px-1.5 py-0.5 rounded-md bg-[var(--bg-hover)] text-[var(--text-muted)]"
                title={t('home.tokenTotalHint')}
              >
                ↑{fmtTokens(tokenStats.total_in)} ↓{fmtTokens(tokenStats.total_out)}
                {tokenStats.cost_cny > 0.001 && <span> · ¥{tokenStats.cost_cny.toFixed(2)}</span>}
              </span>
            )}
            {currentProject && currentProject.kind !== 'global' && (() => {
              const badge = deriveProjectType(currentProject)
              return (
                <span className={`shrink-0 text-[10px] font-medium px-1.5 py-0.5 rounded-md ${projectTypeBadgeClass(badge.kind)}`}>
                  {badge.label}
                </span>
              )
            })()}
            {currentProject && currentProject.path && gitBranches?.has_git && (
              <BranchSelector current={gitBranches.current} branches={gitBranches.branches} onSwitch={switchBranch} />
            )}
          </div>
          {/* 顶栏右侧：模型切换（常驻）+ 更多操作折叠菜单 + 面板开关 */}
          <div className="flex items-center gap-1.5">
            {modelCatalog.length > 0 && (
              <select
                value={modelOptions.model_id ?? ''}
                onChange={(e) => updateModelOptions({ ...modelOptions, model_id: e.target.value || undefined })}
                title={t('home.switchModel')}
                className="h-7 max-w-[8.5rem] rounded-lg bg-[var(--bg-card)] border border-[var(--border)] px-2 text-[11px] text-[var(--text-secondary)] outline-none focus:border-[var(--accent)] transition-colors cursor-pointer"
              >
                <option value="">{t('provider.modelDefault')}</option>
                {modelCatalog.map((g) => (
                  <optgroup key={g.providerName} label={g.providerName}>
                    {g.models.map((m) => (
                      <option key={m.id} value={m.id}>
                        {m.display_name ?? m.model_id}
                      </option>
                    ))}
                  </optgroup>
                ))}
              </select>
            )}

            {currentConversation && (
              <div className="relative" ref={moreMenuRef}>
                <button
                  onClick={() => setShowMoreMenu((v) => !v)}
                  title={t('home.moreActions')}
                  className={`p-2 rounded-lg transition-colors ${
                    showMoreMenu
                      ? 'text-[var(--accent)] bg-[var(--accent-soft)]'
                      : 'text-[var(--text-secondary)] hover:bg-[var(--bg-hover)]'
                  }`}
                >
                  <Icon name="more-vert" size={16} />
                </button>
                {showMoreMenu && (
                  <div className="absolute right-0 top-full mt-1.5 rounded-xl border border-[var(--border)] bg-[var(--bg-card)] shadow-2xl shadow-black/40 py-1 z-50 w-52 animate-modal-in">
                    {/* 导出会话：hover/focus 展开格式子菜单 */}
                    <div className="relative" ref={exportMenuRef}>
                      <button
                        onClick={() => setShowExportMenu((v) => !v)}
                        disabled={messages.length === 0}
                        className="w-full flex items-center justify-between gap-2 px-3 py-2 text-[12px] text-[var(--text-secondary)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] transition-colors disabled:opacity-40 disabled:cursor-not-allowed"
                      >
                        <span className="flex items-center gap-2.5">
                          <Icon name="download" size={14} />
                          {t('home.export')}
                        </span>
                        <Icon name="chevron-right" size={12} className="opacity-60" />
                      </button>
                      {showExportMenu && (
                        <div className="absolute right-full top-0 mr-1 rounded-xl border border-[var(--border)] bg-[var(--bg-card)] shadow-2xl shadow-black/40 py-1 w-44 animate-modal-in">
                          {([
                            ['md', t('home.exportMd')],
                            ['txt', t('home.exportTxt')],
                            ['html', t('home.exportHtml')],
                          ] as const).map(([fmt, label]) => (
                            <button
                              key={fmt}
                              onClick={() => {
                                exportConversationFile(fmt)
                                setShowMoreMenu(false)
                              }}
                              className="w-full flex items-center gap-2.5 px-3 py-2 text-[12px] text-[var(--text-secondary)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] transition-colors"
                            >
                              <Icon name="file" size={14} />
                              {label}
                            </button>
                          ))}
                          <button
                            onClick={() => {
                              copyConversation()
                              setShowMoreMenu(false)
                            }}
                            className="w-full flex items-center gap-2.5 px-3 py-2 text-[12px] text-[var(--text-secondary)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] transition-colors"
                          >
                            <Icon name="check" size={14} />
                            {t('home.exportCopy')}
                          </button>
                        </div>
                      )}
                    </div>
                    <button
                      onClick={() => {
                        setShowMoreMenu(false)
                        void handleRollback()
                      }}
                      disabled={rollbackBusy || messages.length === 0}
                      className="w-full flex items-center gap-2.5 px-3 py-2 text-[12px] text-[var(--text-secondary)] hover:text-[var(--danger)] hover:bg-[var(--bg-hover)] transition-colors disabled:opacity-40"
                    >
                      <Icon name="git-branch" size={14} />
                      {t('home.rollback')}
                    </button>
                    <button
                      onClick={() => {
                        setShowMoreMenu(false)
                        void handleSummarizeMemory()
                      }}
                      disabled={summarizing || messages.length === 0}
                      className="w-full flex items-center gap-2.5 px-3 py-2 text-[12px] text-[var(--text-secondary)] hover:text-[var(--accent)] hover:bg-[var(--bg-hover)] transition-colors disabled:opacity-40"
                    >
                      <Icon name="lightbulb" size={14} />
                      {t('home.summarizeMemory')}
                    </button>
                    <button
                      onClick={() => {
                        setShowMoreMenu(false)
                        void handleCompact()
                      }}
                      disabled={compacting || messages.length === 0}
                      className="w-full flex items-center gap-2.5 px-3 py-2 text-[12px] text-[var(--text-secondary)] hover:text-[var(--accent)] hover:bg-[var(--bg-hover)] transition-colors disabled:opacity-40"
                    >
                      <Icon name="refresh" size={14} className={compacting ? 'animate-spin' : ''} />
                      {t('home.compactHistory')}
                    </button>
                    {currentProject && (
                      <button
                        onClick={() => {
                          setShowMoreMenu(false)
                          void handleOpenTerminal()
                        }}
                        className="w-full flex items-center gap-2.5 px-3 py-2 text-[12px] text-[var(--text-secondary)] hover:text-[var(--accent)] hover:bg-[var(--bg-hover)] transition-colors"
                      >
                        <Icon name="terminal" size={14} />
                        {t('home.openTerminal')}
                      </button>
                    )}
                  </div>
                )}
              </div>
            )}

            <button
              onClick={() =>
                setShowRightPanel((v) => {
                  const next = !v
                  localStorage.setItem('deveco-switch-right-panel', next ? 'expanded' : 'collapsed')
                  return next
                })
              }
              className={`p-2 rounded-lg transition-colors ${
                showRightPanel && currentProject
                  ? 'text-[var(--accent)] bg-[var(--accent-soft)]'
                  : 'text-[var(--text-secondary)] hover:bg-[var(--bg-hover)]'
              }`}
              title={t('home.togglePanel')}
            >
              <Icon name="panel" size={16} />
            </button>
          </div>
        </header>

        {/* 消息区 / 空状态 */}
        <div ref={scrollRef} onScroll={handleScroll} className="chat-scroll flex-1 overflow-y-auto px-6 py-6 scroll-smooth">
          {!currentProject ? (
            <EmptyState onAdd={() => setShowAddDialog(true)} />
          ) : messages.length === 0 && !isStreaming ? (
            <ChatEmptyState onQuick={(text) => fillDraft(text)} />
          ) : (
            <div className="max-w-3xl mx-auto space-y-6 animate-fade-in-up">
              {/* 任务过程徽章（ChatGPT 式）：中间所有过程折叠为“已处理 N 个操作中”，点击展开明细，对话流不中断 */}
              {isStreaming && streaming.startedAt && (
                <TaskOpsBadge
                  running
                  count={toolRuns.length + agentRuns.length}
                  time={fmtElapsed(taskElapsed)}
                  toolName={
                    toolRuns.some((r) => r.status === 'running') ? toolRuns[toolRuns.length - 1]?.tool : undefined
                  }
                  open={opsOpen}
                  onToggle={() => setOpsOpen((v) => !v)}
                  runs={toolRuns}
                  agents={agentRuns}
                />
              )}
              {renderItems.map((item) => {
                // 日期分隔线：今天/昨天/具体日期
                if (item.kind === 'divider') {
                  return (
                    <div key={item.key} className="flex items-center gap-3 my-1">
                      <span className="flex-1 h-px bg-[var(--border)]" />
                      <span className="text-[10.5px] text-[var(--text-muted)] px-2 py-0.5 rounded-full bg-[var(--bg-card)] border border-[var(--border)] shrink-0">
                        {item.label}
                      </span>
                      <span className="flex-1 h-px bg-[var(--border)]" />
                    </div>
                  )
                }
                // 工具记录折叠组（历史连续工具调用合并为一行）
                if (item.kind === 'tools') {
                  return <ToolRunGroup key={item.key} runs={item.runs} onRetry={retryTool} />
                }
                const m = item.message
                return (
                  <MessageItem
                    key={m.id}
                    message={m}
                    time={formatTime(m.created_at)}
                    userMessageId={item.userMessageId}
                    isLastAssistant={m.role === 'assistant' && !isStreaming && messages[messages.length - 1]?.id === m.id}
                    onRegenerate={() => regenerateLast(modelOptions)}
                    onBranch={(msg) => regenerateLast(modelOptions, msg.id)}
                    onRate={rateMessage}
                    onDislike={(id) => setFeedbackDialog({ messageId: id })}
                    onOpenVersions={(msg) => openVersionDialog(msg, item.userMessageId)}
                    onSpeak={toggleSpeak}
                    speaking={speakingId === m.id}
                    onEditMessage={setEditTarget}
                    onDeleteMessage={handleDeleteMessage}
                    confirmDeleteMsgId={confirmDeleteMsgId}
                    projectPath={currentProject?.path}
                    highlighted={highlightMsgId === m.id}
                    onOpenFile={openCodeFile}
                  />
                )
              })}
              {/* 任务进度清单（计划卡）：工具联动推进，任务结束后保留展示 */}
              {plan && <PlanCard plan={plan} />}
              {/* 任务过程回看（ChatGPT 式）：完成后“已处理 N 个操作”徽章，点击展开全部过程 */}
              {!isStreaming && (toolRuns.length > 0 || agentRuns.length > 0) && (
                <TaskOpsBadge
                  count={toolRuns.length + agentRuns.length}
                  open={opsOpen}
                  onToggle={() => setOpsOpen((v) => !v)}
                  runs={toolRuns}
                  agents={agentRuns}
                />
              )}
              {/* 任务完成摘要（ChatGPT 式收尾统计）：耗时 + 工具调用 + 文件变更 */}
              {lastTaskSummary && !isStreaming && (
                <div className="md-task-summary animate-fade-in-up">
                  <div className="md-task-summary-icon">
                    <Icon name="check" size={13} white />
                  </div>
                  <div className="min-w-0">
                    <span className="md-task-summary-title">{t('home.taskDoneTitle')}</span>
                    <span className="md-task-summary-meta tabular-nums">
                      {t('home.taskSummary', {
                        time: fmtElapsed(lastTaskSummary.durationMs / 1000),
                        tools: lastTaskSummary.toolCount,
                        files: lastTaskSummary.fileCount,
                        tokens: (lastTaskSummary.tokensIn + lastTaskSummary.tokensOut).toLocaleString(),
                      })}
                    </span>
                  </div>
                </div>
              )}
              {/* 工具过程已收进顶部“已处理 N 个操作”徽章（展开查看），对话流不再平铺工具卡 */}
              {isStreaming && <StreamingMessage content={streaming.content} reasoning={streaming.reasoning} />}
              {isStreaming && silentSeconds >= 15 && !toolRuns.some((r) => r.status === 'running') && (
                <div className="flex items-center gap-2 text-[11.5px] text-[var(--text-muted)] animate-pulse">
                  <Icon name="spark" size={12} />
                  {t('home.silentHint', { time: fmtElapsed(silentSeconds) })}
                </div>
              )}
              {streaming.error && (
                <ErrorCard
                  error={streaming.error}
                  detail={streaming.errorDetail}
                  onRetry={() => regenerateLast(modelOptions)}
                  retryLabel={t('home.retry')}
                />
              )}
              <div ref={bottomRef} />
            </div>
          )}
          {/* 回到底部：流式中或用户上滑时显示 */}
          {showScrollBottom && (
            <button
              onClick={() => scrollToBottom(true)}
              className="sticky bottom-2 left-1/2 -translate-x-1/2 flex items-center gap-1.5 pl-3 pr-2 h-8 rounded-full bg-[var(--bg-elevated)] border border-[var(--border)] shadow-lg text-xs text-[var(--text-secondary)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-card)] transition-all z-10 animate-fade-in-up"
            >
              <Icon name="chevron-right" size={13} className="rotate-90" />
              {isStreaming ? t('home.scrollToLatest') : t('home.scrollBottom')}
              {unreadCount > 0 && (
                <span className="ml-0.5 min-w-[18px] h-[18px] px-1 flex items-center justify-center rounded-full bg-[var(--accent)] text-white text-[10px] font-semibold leading-none">
                  {unreadCount > 99 ? '99+' : unreadCount}
                </span>
              )}
            </button>
          )}
        </div>

        {/* 输入区 */}
        <div className="shrink-0 px-6 pb-4 pt-1">
          {/* 上下文可视条 + 断点续跑入口 */}
          <div className="max-w-3xl mx-auto flex items-center gap-2 pb-1.5 min-h-5">
            {runtimeWatching && (
              <span className="flex items-center gap-1 px-1.5 py-px rounded-full bg-[var(--success)]/15 text-[var(--success)] text-[11px]">
                <span className="relative flex h-1.5 w-1.5">
                  <span className="animate-ping absolute inline-flex h-full w-full rounded-full bg-[var(--success)] opacity-75" />
                  <span className="relative inline-flex rounded-full h-1.5 w-1.5 bg-[var(--success)]" />
                </span>
                {t('runtime.watching')}
              </span>
            )}
            {ctxInfo && currentConversation && (
              <div className="flex items-center gap-2 text-[11px] text-[var(--text-muted)]">
                <Icon name="info" size={11} />
                {t('home.ctxBar', { count: ctxInfo.message_count })}
                {ctxInfo.event_count > 0 && (
                  <span className="tabular-nums">
                    {t('home.ctxEvents', { count: ctxInfo.event_count })}
                  </span>
                )}
                {ctxInfo.total_tokens_in + ctxInfo.total_tokens_out > 0 && (
                  <span className="tabular-nums">
                    {t('home.ctxStats', {
                      total: (ctxInfo.total_tokens_in + ctxInfo.total_tokens_out).toLocaleString(),
                      tin: ctxInfo.total_tokens_in.toLocaleString(),
                      tout: ctxInfo.total_tokens_out.toLocaleString(),
                      time: fmtElapsed(ctxInfo.total_duration_ms / 1000),
                    })}
                  </span>
                )}
                {ctxInfo.has_summary && (
                  <span className="px-1.5 py-px rounded-full bg-[var(--success)]/15 text-[var(--success)]">
                    {t('home.ctxSummaryBadge')}
                  </span>
                )}
                {/* token 预算进度条：估算占用 / 模型上下文窗口（>85% 触发自动压缩） */}
                {ctxInfo.context_limit > 0 && (() => {
                  const pct = Math.min(100, Math.round((ctxInfo.estimated_tokens / ctxInfo.context_limit) * 100))
                  const barColor = pct > 85 ? 'bg-[var(--danger)]' : pct > 60 ? 'bg-[var(--warning)]' : 'bg-[var(--success)]'
                  return (
                    <span
                      className="flex items-center gap-1.5"
                      title={t('home.ctxBudgetTitle', {
                        tokens: ctxInfo.estimated_tokens.toLocaleString(),
                        limit: ctxInfo.context_limit.toLocaleString(),
                        pct,
                      })}
                    >
                      <span className="w-16 h-1.5 rounded-full bg-[var(--bg-hover)] overflow-hidden shrink-0">
                        <span
                          className={`block h-full rounded-full transition-all ${barColor}`}
                          style={{ width: `${Math.max(pct, 2)}%` }}
                        />
                      </span>
                      <span className="tabular-nums">{pct}%</span>
                    </span>
                  )
                })()}
              </div>
            )}
            {unfinishedConv && unfinishedConv.conversationId === currentConversation?.id && !isStreaming && (
              <button
                onClick={() => void sendUserMessage(t('home.continuePrompt'), modelOptions)}
                className="ml-auto flex items-center gap-1.5 h-6 px-2.5 rounded-full bg-[var(--accent-soft)] text-[var(--accent)] text-[11px] font-medium hover:brightness-110 transition-all"
              >
                <Icon name="arrow-down" size={11} />
                {t('home.continueTask')}
              </button>
            )}
          </div>
          {/* 排队中消息条：运行中提交的消息，任务结束后续跑；支持单条移除 */}
          {queuedList.length > 0 && currentConversation && (
            <div className="max-w-3xl mx-auto pb-1.5">
              <button
                onClick={() => setQueuedOpen((v) => !v)}
                className="flex items-center gap-1.5 text-[11px] text-[var(--text-muted)] hover:text-[var(--text-primary)] transition-colors"
              >
                <Icon name="terminal" size={11} />
                {t('home.queuedBar', { count: queuedList.length })}
                <Icon
                  name="chevron-right"
                  size={10}
                  className={`transition-transform ${queuedOpen ? 'rotate-90' : ''}`}
                />
              </button>
              {queuedOpen && (
                <div className="mt-1 space-y-1 max-h-36 overflow-y-auto">
                  {queuedList.map((q) => (
                    <div
                      key={q.id}
                      className="flex items-center gap-2 rounded-lg bg-[var(--bg-card)] border border-[var(--border)] px-2.5 py-1.5"
                    >
                      <span className="text-[11px] text-[var(--text-primary)] truncate flex-1" title={q.content}>
                        {q.content}
                      </span>
                      {q.agent_owned && (
                        <span className="text-[9px] px-1 py-px rounded bg-[var(--accent)]/10 text-[var(--accent)] shrink-0">
                          {t('home.queuedAgentLabel')}
                        </span>
                      )}
                      <button
                        onClick={() => removeQueued(q.id)}
                        className="p-0.5 rounded text-[var(--text-muted)] hover:text-[var(--danger)] hover:bg-[var(--bg-hover)] transition-colors shrink-0"
                        title={t('home.queuedRemove')}
                      >
                        <Icon name="close" size={11} />
                      </button>
                    </div>
                  ))}
                </div>
              )}
            </div>
          )}
          <div
            className="relative max-w-3xl mx-auto rounded-2xl border border-[var(--border)] bg-[var(--bg-secondary)] transition-all focus-within:border-[var(--accent)] focus-within:shadow-[0_0_0_3px_var(--accent-soft)]"
            onDragOver={(e) => {
              if (Array.from(e.dataTransfer.types).includes('Files')) e.preventDefault()
            }}
            onDrop={(e) => {
              e.preventDefault()
              if (e.dataTransfer.files.length > 0) addImageFiles(e.dataTransfer.files)
            }}
          >
            {/* 斜杠快捷指令面板（行首 / 后弹出） */}
            {slashCandidates && (
              <div className="absolute bottom-full left-3 right-3 mb-1.5 rounded-xl border border-[var(--border)] bg-[var(--bg-card)] shadow-2xl shadow-black/40 py-1 z-50 max-h-72 overflow-y-auto animate-modal-in">
                <div className="px-3 py-1.5 text-[10px] font-medium text-[var(--text-muted)] border-b border-[var(--border)]">
                  {t('home.slashHint')}
                </div>
                {slashCandidates.length === 0 && (
                  <div className="px-3 py-2.5 text-[11px] text-[var(--text-muted)]">{t('home.slashNoMatch')}</div>
                )}
                {slashCandidates.map((c, i) => (
                  <button
                    key={c.id}
                    onMouseEnter={() => setSlashIdx(i)}
                    onClick={() => pickSlash(c)}
                    className={`w-full flex items-center gap-2.5 px-3 py-2 text-left transition-colors ${
                      i === slashIdx ? 'bg-[var(--bg-hover)]' : 'hover:bg-[var(--bg-hover)]'
                    }`}
                  >
                    <span className="w-6 h-6 rounded-md bg-[var(--accent-soft)] text-[var(--accent)] flex items-center justify-center shrink-0">
                      <Icon name={c.icon} size={13} />
                    </span>
                    <span className="min-w-0">
                      <span className="block text-[12px] text-[var(--text-primary)]">{c.title}</span>
                      <span className="block text-[10.5px] text-[var(--text-muted)] truncate">/{c.id}</span>
                    </span>
                  </button>
                ))}
              </div>
            )}
            {/* @ 引用候选面板（@ 后输入路径片段时弹出） */}
            {refCandidates && (
              <div className="absolute bottom-full left-3 right-3 mb-1.5 rounded-xl border border-[var(--border)] bg-[var(--bg-card)] shadow-2xl shadow-black/40 py-1 z-50 max-h-60 overflow-y-auto animate-modal-in">
                <div className="px-3 py-1.5 text-[10px] font-medium text-[var(--text-muted)] border-b border-[var(--border)]">
                  {t('home.refHint')}「@{refQuery}」
                </div>
                {refCandidates.length === 0 && (
                  <div className="px-3 py-2.5 text-[11px] text-[var(--text-muted)]">{t('home.refNoMatch')}</div>
                )}
                {refCandidates.map((c, i) => (
                  <button
                    key={c.path}
                    onMouseEnter={() => setRefIdx(i)}
                    onClick={() => pickReference(c.path)}
                    className={`w-full flex items-center gap-2 px-3 py-1.5 text-left transition-colors ${
                      i === refIdx ? 'bg-[var(--bg-hover)]' : 'hover:bg-[var(--bg-hover)]'
                    }`}
                  >
                    <Icon name="file" size={12} className="text-[var(--text-muted)] shrink-0" />
                    <span className="text-[11.5px] text-[var(--text-primary)] truncate">{c.path}</span>
                  </button>
                ))}
              </div>
            )}
            <textarea
              ref={inputRef}
              value={draft}
              onChange={(e) => handleDraftChange(e.target.value)}
              onKeyDown={(e) => {
                // 斜杠候选面板打开时：上下选择、回车/Tab 确认、Esc 关闭
                if (slashCandidates && slashCandidates.length > 0) {
                  if (e.key === 'ArrowDown') {
                    e.preventDefault()
                    setSlashIdx((i) => (i + 1) % slashCandidates.length)
                    return
                  }
                  if (e.key === 'ArrowUp') {
                    e.preventDefault()
                    setSlashIdx((i) => (i - 1 + slashCandidates.length) % slashCandidates.length)
                    return
                  }
                  if (e.key === 'Enter' || e.key === 'Tab') {
                    e.preventDefault()
                    if (slashCandidates[slashIdx]) pickSlash(slashCandidates[slashIdx])
                    return
                  }
                  if (e.key === 'Escape') {
                    e.preventDefault()
                    setSlashCandidates(null)
                    return
                  }
                }
                // @ 引用候选面板：上下选择、回车/Tab 确认、Esc 关闭
                if (refCandidates && refCandidates.length > 0) {
                  if (e.key === 'ArrowDown') {
                    e.preventDefault()
                    setRefIdx((i) => (i + 1) % refCandidates.length)
                    return
                  }
                  if (e.key === 'ArrowUp') {
                    e.preventDefault()
                    setRefIdx((i) => (i - 1 + refCandidates.length) % refCandidates.length)
                    return
                  }
                  if (e.key === 'Enter' || e.key === 'Tab') {
                    e.preventDefault()
                    pickReference(refCandidates[refIdx]?.path ?? '')
                    return
                  }
                  if (e.key === 'Escape') {
                    e.preventDefault()
                    setRefCandidates(null)
                    return
                  }
                }
                if (e.key === 'Enter' && !e.shiftKey) {
                  e.preventDefault()
                  handleSend()
                }
              }}
              onPaste={(e) => {
                // 图片粘贴：读剪贴板文件为 data URL 随消息发送（多模态）
                if (e.clipboardData.files.length > 0) {
                  e.preventDefault()
                  addImageFiles(e.clipboardData.files)
                }
              }}
              placeholder={currentProject ? t('home.inputPlaceholder') : t('home.inputDisabled')}
              disabled={!currentProject}
              rows={1}
              style={{ height: inputHeight }}
              className="w-full resize-none bg-transparent px-4 pt-3.5 pb-1 text-sm outline-none placeholder:text-[var(--text-muted)] disabled:opacity-40 overflow-y-auto"
            />
            {/* 引用标签（@ 选择后展示，可移除） */}
            {references.length > 0 && (
              <div className="flex flex-wrap gap-1 px-3 pt-1">
                {references.map((p) => (
                  <span
                    key={p}
                    className="flex items-center gap-1 px-2 py-0.5 rounded-md bg-[var(--accent-soft)] text-[10.5px] text-[var(--accent)] max-w-56"
                  >
                    <Icon name="file" size={10} className="shrink-0" />
                    <span className="truncate">{p}</span>
                    <button
                      onClick={() => setReferences((r) => r.filter((x) => x !== p))}
                      className="shrink-0 hover:text-[var(--danger)] transition-colors"
                      title={t('home.removeReference')}
                    >
                      <Icon name="close" size={10} />
                    </button>
                  </span>
                ))}
              </div>
            )}
            {/* 待发送图片（粘贴/拖入，最多 4 张） */}
            {pickedImages.length > 0 && (
              <div className="flex gap-1.5 px-3 pt-1.5">
                {pickedImages.map((img, i) => (
                  <div key={i} className="relative group/img">
                    <img src={img} alt="" className="w-12 h-12 object-cover rounded-lg border border-[var(--border)]" />
                    <button
                      onClick={() => removePickedImage(i)}
                      className="absolute -top-1.5 -right-1.5 w-4 h-4 rounded-full bg-[var(--bg-card)] border border-[var(--border)] text-[var(--text-muted)] hover:text-[var(--danger)] flex items-center justify-center opacity-0 group-hover/img:opacity-100 transition-opacity"
                      title={t('home.removeImage')}
                    >
                      <Icon name="close" size={9} />
                    </button>
                  </div>
                ))}
              </div>
            )}
            <div className="flex items-center justify-between px-3 pb-2.5 pt-1">
              <div className="flex items-center gap-1.5">
                {/* Rules 编辑：全局指令 + 项目级 rules（注入 system_prompt） */}
                <button
                  onClick={() => void openRulesDialog()}
                  title={t('home.rules')}
                  className="p-1.5 rounded-lg text-[var(--text-secondary)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] transition-colors"
                >
                  <Icon name="settings" size={13} />
                </button>
                {/* 模型设置：切换模型 / 代理 / 采样参数 */}
                <div className="relative shrink-0" ref={modelSettingsRef}>
                  <button
                    onClick={() => setShowModelSettings((v) => !v)}
                    title={t('home.modelSettings')}
                    className={`flex items-center gap-1.5 pl-1.5 pr-2 py-1 rounded-lg text-[11px] transition-colors ${
                      showModelSettings
                        ? 'text-[var(--accent)] bg-[var(--accent-soft)]'
                        : 'text-[var(--text-secondary)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)]'
                    }`}
                  >
                    <Icon name="bolt" size={12} />
                    <span className="max-w-32 truncate">{currentModelLabel}</span>
                    <Icon name="chevron-right" size={10} className="rotate-90 opacity-60" />
                  </button>
                  {showModelSettings && (
                    <ModelSettingsPopover catalog={modelCatalog} options={modelOptions} onChange={updateModelOptions} />
                  )}
                </div>
                {/* Web 预览已移至右侧栏 Preview 面板 */}
                {isStreaming && (
                  <span className="flex items-center gap-1.5 text-[11px] text-[var(--text-secondary)]">
                    <span className="w-1.5 h-1.5 rounded-full bg-[var(--accent)] animate-pulse" />
                    {t('home.agentWorking')}
                  </span>
                )}
                <span className="text-[11px] text-[var(--text-muted)] hidden sm:block">
                  {isStreaming
                    ? t('home.queuedHint')
                    : `Enter ${t('home.send')} · Shift+Enter ${t('home.newline')}`}
                </span>
              </div>
              <div className="flex items-center gap-2.5">
                {!isStreaming && (
                  <span className="text-[11px] tabular-nums text-[var(--text-muted)]">{draft.length}</span>
                )}
                {isStreaming ? (
                  <>
                    {toolRuns.some((r) => r.status === 'running') && (
                      <button
                        onClick={() => stopCurrentTool()}
                        className="h-8 px-3 rounded-full bg-[var(--warning)]/12 text-[var(--warning)] flex items-center gap-1.5 hover:bg-[var(--warning)]/20 active:scale-95 transition-all text-[12px] font-medium"
                        title={t('home.stopTool')}
                      >
                        <Icon name="bolt" size={12} />
                        {t('home.stopTool')}
                      </button>
                    )}
                    <button
                      onClick={() => stopGeneration()}
                      className="h-8 px-3 rounded-full bg-[var(--danger)]/12 text-[var(--danger)] flex items-center gap-1.5 hover:bg-[var(--danger)]/20 active:scale-95 transition-all text-[12px] font-medium"
                      title={t('home.stopGenerating')}
                    >
                      <span className="w-2.5 h-2.5 rounded-[3px] bg-[var(--danger)]" />
                      {t('home.stopGenerating')}
                    </button>
                    <button
                      onClick={handleSendToAgent}
                      disabled={!draft.trim()}
                      className="h-8 px-3 rounded-full bg-[var(--accent)]/12 text-[var(--accent)] flex items-center gap-1.5 hover:bg-[var(--accent)]/20 active:scale-95 disabled:opacity-35 disabled:cursor-not-allowed transition-all text-[12px] font-medium"
                      title={t('home.sendToAgent')}
                    >
                      <Icon name="bolt" size={12} />
                      {t('home.sendToAgent')}
                    </button>
                    <button
                      onClick={handleSend}
                      disabled={!draft.trim()}
                      className="w-8 h-8 rounded-full text-white flex items-center justify-center active:scale-95 disabled:opacity-35 disabled:cursor-not-allowed transition-all shadow-lg shadow-[var(--accent)]/30 bg-[linear-gradient(135deg,var(--accent),var(--accent-hover))] hover:shadow-[0_4px_16px_var(--accent-glow)]"
                      title={t('home.queueSend')}
                    >
                      <Icon name="send" size={14} white />
                    </button>
                  </>
                ) : (
                  <button
                    onClick={handleSend}
                    disabled={!draft.trim() || !currentProject}
                    className="w-8 h-8 rounded-full text-white flex items-center justify-center active:scale-95 disabled:opacity-35 disabled:cursor-not-allowed transition-all shadow-lg shadow-[var(--accent)]/30 bg-[linear-gradient(135deg,var(--accent),var(--accent-hover))] hover:shadow-[0_4px_16px_var(--accent-glow)]"
                    title={t('home.send')}
                  >
                    <Icon name="send" size={14} white />
                  </button>
                )}
              </div>
            </div>
            {/* 拖拽手柄：上下调整输入区高度 */}
            <div
              onPointerDown={onDragStart}
              onPointerMove={onDragMove}
              onPointerUp={onDragEnd}
              className="h-3 flex items-center justify-center cursor-ns-resize touch-none select-none group/drag"
            >
              <div className="w-9 h-1 rounded-full bg-[var(--border-strong)] group-hover/drag:bg-[var(--accent)] transition-colors" />
            </div>
          </div>
        </div>
      </main>

      {/* ============ 右侧：概览 / 文件树面板 ============ */}
      {showRightPanel && currentProject && (
        <>
          {/* 右侧拖拽手柄：调整右侧栏宽度 */}
          <div
            onPointerDown={onRightDragStart}
            onPointerMove={onRightDragMove}
            onPointerUp={onRightDragEnd}
            title={t('home.dragRightPanel')}
            className="w-1 shrink-0 cursor-col-resize bg-transparent hover:bg-[var(--accent)]/50 active:bg-[var(--accent)]/50 transition-colors touch-none select-none"
          />
          <aside
            style={{ width: rightWidth }}
            className={`shrink-0 min-w-0 bg-[var(--bg-secondary)] border-l border-[var(--border)] flex flex-col animate-fade-in-up ${resizing === 'right' ? '' : 'transition-[width] duration-200 ease-out'}`}
          >
          {/* Tab 切换：右侧栏过窄时仅显示图标（flex-1 均分），悬停 title 提示 */}
          {(() => {
            const tabs: { key: typeof rightTab; icon: IconName; label: string }[] = [
              { key: 'overview', icon: 'info', label: t('home.overview') },
              { key: 'files', icon: 'folder', label: t('home.fileTree') },
              { key: 'memories', icon: 'lightbulb', label: t('home.memories') },
              { key: 'stats', icon: 'receipt', label: t('home.toolStats') },
              { key: 'git', icon: 'git-branch', label: t('home.git') },
              { key: 'preview', icon: 'devices', label: t('home.openPreview') },
              { key: 'devices', icon: 'phone', label: t('home.devices') },
              { key: 'symbols', icon: 'search', label: t('home.symbols') },
              { key: 'analyze', icon: 'spark', label: t('home.analyze') },
              { key: 'terminal', icon: 'terminal', label: t('home.terminal') },
            ]
            return (
              <div
                className={`right-tabbar h-16 flex items-center gap-1 border-b border-[var(--border)] shrink-0 overflow-x-auto ${
                  rightCompact ? 'px-2' : 'px-3'
                }`}
              >
                {tabs.map((tb) => {
                  const active = rightTab === tb.key
                  return (
                    <button
                      key={tb.key}
                      onClick={() => setRightTab(tb.key)}
                      title={tb.label}
                      className={`flex items-center gap-2 h-9 shrink-0 rounded-lg text-[12.5px] font-medium transition-colors ${
                        active
                          ? 'text-[var(--accent)] bg-[var(--accent-soft)]'
                          : 'text-[var(--text-secondary)] hover:bg-[var(--bg-hover)]'
                      } ${rightCompact ? 'w-9 justify-center px-0' : 'px-3.5'}`}
                    >
                      <Icon name={tb.icon} size={15} />
                      {!rightCompact && <span className="whitespace-nowrap">{tb.label}</span>}
                    </button>
                  )
                })}
              </div>
            )
          })()}

          {/* Tab 内容 */}
          <div className="flex-1 overflow-y-auto">
            {rightTab === 'overview' ? (
              <div className="p-3 space-y-2.5">
                {/* 项目信息卡 */}
                <div className="rounded-xl border border-[var(--border)] bg-[var(--bg-card)] p-3.5">
                  <div className="flex items-center gap-2.5">
                    <div className="w-9 h-9 rounded-[10px] bg-gradient-to-br from-[var(--accent)] to-[#8b5cf6] flex items-center justify-center shrink-0 shadow-md shadow-[var(--accent)]/20">
                      <Icon name="bolt" size={16} white />
                    </div>
                    <div className="min-w-0">
                      <div className="text-[13px] font-medium truncate">{currentProject.name}</div>
                      <div className="text-[11px] text-[var(--text-muted)]">
                        {deriveProjectType(currentProject).label}
                      </div>
                    </div>
                  </div>
                  <div className="mt-3 pt-3 border-t border-[var(--border)] space-y-2.5">
                    <OverviewRow icon="folder" label={t('home.projectPath')} value={currentProject.path} mono />
                    <OverviewRow
                      icon="health"
                      label={t('home.indexState')}
                      value={t(`home.index.${currentProject.index_state}`)}
                      tone={currentProject.index_state === 'ready' ? 'ok' : 'warn'}
                    />
                    <OverviewRow
                      icon="check"
                      label={t('home.trusted')}
                      value={currentProject.trusted ? t('home.trustedYes') : t('home.trustedNo')}
                      tone={currentProject.trusted ? 'ok' : 'warn'}
                    />
                    <OverviewRow
                      icon="info"
                      label={t('home.createdAt')}
                      value={new Date(currentProject.created_at * 1000).toLocaleDateString()}
                    />
                    <button
                      type="button"
                      onClick={() => void handleOpenTerminal()}
                      className="w-full flex items-center justify-center gap-1.5 mt-1 px-2.5 py-2 rounded-lg text-[11px] text-[var(--text-secondary)] bg-[var(--bg-hover)]/60 border border-[var(--border)] hover:text-[var(--accent)] hover:border-[var(--accent)]/40 transition-colors"
                    >
                      <Icon name="terminal" size={12} />
                      {t('home.openTerminal')}
                    </button>
                  </div>
                </div>

                {/* Git 变更摘要：当前工作区状态入口（点击跳转 Git 面板） */}
                <OverviewGitSummary projectPath={currentProject.path} onOpenGit={() => setRightTab('git')} />

                {/* 工作区模块：混合工作区中识别到的各类型子工程（Vue/Java/Go/HarmonyOS 等），支持手动绑定 */}
                {(() => {
                  const mods = parseWorkspaceModules(currentProject.workspace_modules)
                  if (currentProject.kind === 'global') return null
                  const kindColor = (k: ModuleKind): string => {
                    switch (k) {
                      case 'harmony': return 'bg-[#e6f7ef] text-[#1a9b5c] dark:bg-[#1a9b5c]/15 dark:text-[#4ade80]'
                      case 'vue': return 'bg-[#e6fbf3] text-[#42b883] dark:bg-[#42b883]/15 dark:text-[#4ade80]'
                      case 'react': return 'bg-[#e6f4fe] text-[#149eca] dark:bg-[#149eca]/15 dark:text-[#61dafb]'
                      case 'angular': return 'bg-[#ffe9e9] text-[#dd0031] dark:bg-[#dd0031]/15 dark:text-[#ff6b6b]'
                      case 'node': return 'bg-[#e9f9e3] text-[#5fa04e] dark:bg-[#5fa04e]/15 dark:text-[#86efac]'
                      case 'java': case 'kotlin': return 'bg-[#fff3e0] text-[#e76f00] dark:bg-[#e76f00]/15 dark:text-[#fbbf24]'
                      case 'go': return 'bg-[#e3f2fd] text-[#00add8] dark:bg-[#00add8]/15 dark:text-[#38bdf8]'
                      case 'python': return 'bg-[#fef3c7] text-[#d97706] dark:bg-[#d97706]/15 dark:text-[#fbbf24]'
                      case 'rust': return 'bg-[#fce7e7] text-[#ce422b] dark:bg-[#ce422b]/15 dark:text-[#f87171]'
                      case 'flutter': return 'bg-[#e7f0ff] text-[#02569b] dark:bg-[#02569b]/15 dark:text-[#60a5fa]'
                      case 'android': return 'bg-[#e3f9e5] text-[#3ddc84] dark:bg-[#3ddc84]/15 dark:text-[#4ade80]'
                      case 'ios': return 'bg-[var(--bg-hover)] text-[var(--text-secondary)]'
                      case 'html': return 'bg-[#fff0e6] text-[#e34c26] dark:bg-[#e34c26]/15 dark:text-[#fb923c]'
                      case 'dotnet': return 'bg-[#f3e8ff] text-[#7c3aed] dark:bg-[#7c3aed]/15 dark:text-[#c4b5fd]'
                      default: return 'bg-[var(--bg-hover)] text-[var(--text-muted)]'
                    }
                  }
                  // 类别归组：鸿蒙 / 前端 / 后端 / 其它（用于筛选 chips 与计数）
                  const kindGroup = (k: ModuleKind): 'harmony' | 'frontend' | 'backend' | 'other' => {
                    if (k === 'harmony') return 'harmony'
                    if (k === 'vue' || k === 'react' || k === 'angular' || k === 'html' || k === 'node') return 'frontend'
                    if (k === 'java' || k === 'kotlin' || k === 'go' || k === 'python' || k === 'rust' || k === 'dotnet') return 'backend'
                    return 'other'
                  }
                  const groupCount = (g: 'harmony' | 'frontend' | 'backend' | 'other') =>
                    mods.filter((m) => kindGroup(m.kind) === g).length
                  const filteredMods =
                    moduleFilter === 'all' ? mods : mods.filter((m) => kindGroup(m.kind) === moduleFilter)
                  // 解析后的鸿蒙主工程相对路径（非项目根时模块行显示“主工程”徽标）
                  const mainRel = (() => {
                    if (!mainRootAbs || !currentProject.path || mainRootAbs === currentProject.path) return null
                    const base = currentProject.path.replace(/[\\/]+$/, '')
                    const norm = mainRootAbs.replace(/\\/g, '/')
                    const normBase = base.replace(/\\/g, '/')
                    return norm.startsWith(normBase + '/') ? norm.slice(normBase.length + 1) : null
                  })()
                  const openInExplorer = (rel: string) => {
                    void shellOpen((currentProject.path.replace(/[\\/]+$/, '') + '/' + rel).replace(/\\/g, '/')).catch(() => {})
                  }
                  return (
                    <div className="rounded-xl border border-[var(--border)] bg-[var(--bg-card)] p-3">
                      <div className="flex items-center gap-1.5">
                        <Icon name="folder" size={12} className="text-[var(--accent)]" />
                        <span className="text-[12px] font-medium">{t('home.workspaceModules')}</span>
                        <span className="px-1.5 py-0.5 rounded-full text-[10px] font-medium bg-[var(--accent)]/10 text-[var(--accent)]">
                          {editingModules ? moduleDraft.length : mods.length}
                        </span>
                        {!editingModules && (
                          <>
                            <button
                              type="button"
                              onClick={() => void handleRescanModules()}
                              disabled={rescanning}
                              title={t('home.rescanModules')}
                              className="ml-auto p-1 rounded-md text-[var(--text-muted)] hover:text-[var(--accent)] hover:bg-[var(--bg-hover)] transition-colors disabled:opacity-50"
                            >
                              <Icon name="refresh" size={11} className={rescanning ? 'animate-spin' : ''} />
                            </button>
                            <button
                              type="button"
                              onClick={startEditModules}
                              title={t('home.editModules')}
                              className="p-1 rounded-md text-[var(--text-muted)] hover:text-[var(--accent)] hover:bg-[var(--bg-hover)] transition-colors"
                            >
                              <Icon name="edit" size={11} />
                            </button>
                          </>
                        )}
                      </div>

                      {/* 只读展示 */}
                      {!editingModules && (
                        mods.length === 0 ? (
                          <p className="text-[11px] text-[var(--text-muted)] mt-2 flex items-center gap-2">
                            {t('home.noWorkspaceModules')}
                            <button onClick={() => void handleRescanModules()} className="text-[var(--accent)] hover:underline">
                              {t('home.rescanModules')}
                            </button>
                          </p>
                        ) : (
                          <>
                            {/* 类别筛选 chips：混合工作区模块较多时按类型聚焦 */}
                            {mods.length > 1 && (
                              <div className="mt-2 flex items-center gap-1 flex-wrap">
                                {([
                                  { key: 'all' as const, label: `${t('home.analyzeAll')} (${mods.length})` },
                                  { key: 'harmony' as const, label: `HarmonyOS (${groupCount('harmony')})` },
                                  { key: 'frontend' as const, label: `${t('home.moduleFilterFrontend')} (${groupCount('frontend')})` },
                                  { key: 'backend' as const, label: `${t('home.moduleFilterBackend')} (${groupCount('backend')})` },
                                  { key: 'other' as const, label: `${t('home.moduleFilterOther')} (${groupCount('other')})` },
                                ]).map((g) => (
                                  <button
                                    key={g.key}
                                    type="button"
                                    onClick={() => setModuleFilter(g.key)}
                                    className={`px-1.5 py-0.5 rounded text-[9.5px] transition-colors ${
                                      moduleFilter === g.key
                                        ? 'bg-[var(--accent)]/15 text-[var(--accent)] font-medium'
                                        : 'bg-[var(--bg-hover)] text-[var(--text-muted)] hover:text-[var(--text-primary)]'
                                    }`}
                                  >
                                    {g.label}
                                  </button>
                                ))}
                              </div>
                            )}
                            <div className="mt-2 space-y-0.5 max-h-48 overflow-y-auto">
                              {filteredMods.length === 0 && (
                                <p className="text-[10.5px] text-[var(--text-muted)] py-1">{t('home.moduleNoMatch')}</p>
                              )}
                              {filteredMods.map((m) => (
                                <div
                                  key={m.rel_path}
                                  className="flex items-center gap-2 text-[11px] rounded px-1 py-0.5 -mx-1 cursor-pointer hover:bg-[var(--bg-hover)] transition-colors"
                                  title={`${m.rel_path}（${t('home.moduleOpenInExplorer')}）`}
                                  onClick={() => openInExplorer(m.rel_path)}
                                >
                                  <Icon name="chevron-right" size={11} className="text-[var(--text-muted)] shrink-0" />
                                  <span className="font-mono text-[var(--text-secondary)] truncate flex-1" title={m.rel_path}>
                                    {m.rel_path}
                                  </span>
                                  {mainRel && m.rel_path === mainRel && (
                                    <span className="shrink-0 px-1 py-0.5 rounded text-[9px] font-medium bg-[var(--accent)]/15 text-[var(--accent)]">
                                      ◉ {t('home.analyzeHarmonyRoot')}
                                    </span>
                                  )}
                                  <span className={`px-1.5 py-0.5 rounded text-[10px] font-medium shrink-0 ${kindColor(m.kind)}`}>
                                    {MODULE_KIND_LABELS[m.kind]}
                                  </span>
                                  {m.manual && (
                                    <span title={t('home.manualModule')} className="text-[var(--text-muted)] shrink-0">
                                      <Icon name="edit" size={9} />
                                    </span>
                                  )}
                                </div>
                              ))}
                            </div>
                          </>
                        )
                      )}

                      {/* 编辑态：修改类型 / 增删行 */}
                      {editingModules && (
                        <div className="mt-2 space-y-1.5">
                          <div className="max-h-56 overflow-y-auto space-y-1.5">
                            {moduleDraft.map((m, idx) => (
                              <div key={idx} className="flex items-center gap-1.5">
                                <input
                                  value={m.rel_path}
                                  onChange={(e) => updateModuleRow(idx, { rel_path: e.target.value })}
                                  placeholder={t('home.modulePathPlaceholder')}
                                  className="flex-1 min-w-0 h-7 rounded-md bg-[var(--bg-primary)] border border-[var(--border)] px-2 text-[11px] font-mono outline-none focus:border-[var(--accent)]"
                                />
                                <select
                                  value={m.kind}
                                  onChange={(e) => updateModuleRow(idx, { kind: e.target.value as ModuleKind })}
                                  className="h-7 rounded-md bg-[var(--bg-primary)] border border-[var(--border)] px-1.5 text-[11px] outline-none focus:border-[var(--accent)]"
                                >
                                  {MODULE_KINDS.map((k) => (
                                    <option key={k} value={k}>{MODULE_KIND_LABELS[k]}</option>
                                  ))}
                                </select>
                                <button
                                  type="button"
                                  onClick={() => removeModuleRow(idx)}
                                  className="p-1 rounded text-[var(--text-muted)] hover:text-[var(--danger)] hover:bg-[var(--danger)]/10 transition-colors"
                                  title={t('dialog.remove')}
                                >
                                  <Icon name="delete" size={13} />
                                </button>
                              </div>
                            ))}
                          </div>
                          <button
                            type="button"
                            onClick={addModuleRow}
                            className="w-full h-7 rounded-md border border-dashed border-[var(--border-strong)] text-[11px] text-[var(--text-secondary)] hover:border-[var(--accent)] hover:text-[var(--accent)] transition-colors"
                          >
                            + {t('home.addModule')}
                          </button>
                          <div className="flex justify-end gap-2 pt-1">
                            <button
                              type="button"
                              onClick={cancelEditModules}
                              className="px-2.5 h-7 rounded-md text-[11px] text-[var(--text-secondary)] hover:bg-[var(--bg-hover)]"
                            >
                              {t('dialog.cancel')}
                            </button>
                            <button
                              type="button"
                              onClick={() => void saveModules()}
                              disabled={savingModules}
                              className="px-2.5 h-7 rounded-md text-[11px] font-medium bg-[var(--accent)] text-white hover:bg-[var(--accent-hover)] disabled:opacity-50"
                            >
                              {savingModules ? '…' : t('dialog.save')}
                            </button>
                          </div>
                        </div>
                      )}
                    </div>
                  )
                })()}

                {/* 最近任务：task_runs 明细，点击跳转对应会话 */}
                <div className="rounded-xl border border-[var(--border)] bg-[var(--bg-card)] p-3">
                  <div className="flex items-center gap-1.5">
                    <Icon name="receipt" size={12} className="text-[var(--text-secondary)]" />
                    <span className="text-[12px] font-medium">{t('home.recentTasks')}</span>
                    <button
                      type="button"
                      onClick={() => void loadRecentRuns()}
                      title={t('home.refresh')}
                      className="ml-auto p-1 rounded-md text-[var(--text-muted)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] transition-colors"
                    >
                      <Icon name="refresh" size={11} />
                    </button>
                  </div>
                  <div className="mt-1.5 space-y-0.5">
                    {recentRuns.length === 0 ? (
                      <div className="text-[11px] text-[var(--text-muted)] py-1">{t('home.recentTasksEmpty')}</div>
                    ) : (
                      recentRuns.slice(0, 6).map((r) => (
                        <button
                          key={r.id}
                          type="button"
                          onClick={() => void openConversation(r.conversation_id)}
                          className="w-full flex items-center gap-2 px-1.5 py-1 rounded-lg hover:bg-[var(--bg-hover)] transition-colors text-left"
                          title={r.error_message || undefined}
                        >
                          <span
                            className={`w-1.5 h-1.5 rounded-full shrink-0 ${
                              r.status === 'success'
                                ? 'bg-[var(--success)]'
                                : r.status === 'error'
                                  ? 'bg-[var(--danger)]'
                                  : 'bg-[var(--warning)]'
                            }`}
                          />
                          <span className="flex-1 min-w-0">
                            <span className="block truncate text-[11px] text-[var(--text-primary)]">
                              {r.model || t('home.unknownModel')}
                            </span>
                            <span className="block text-[9.5px] text-[var(--text-muted)]">
                              {r.error_kind ||
                                (r.status === 'success'
                                  ? t('home.taskSuccess')
                                  : r.status === 'error'
                                    ? t('home.taskError')
                                    : t('home.taskCancelled'))}
                            </span>
                          </span>
                          <span className="shrink-0 text-[10px] tabular-nums text-[var(--text-muted)]">
                            {fmtElapsed(r.duration_ms / 1000)}
                          </span>
                        </button>
                      ))
                    )}
                  </div>
                </div>

                {/* 提示卡 */}
                <div className="rounded-xl border border-[var(--border)] bg-[var(--bg-window)]/50 p-3 flex gap-2">
                  <Icon name="info" size={14} className="mt-0.5 shrink-0 opacity-50" />
                  <p className="text-[11px] text-[var(--text-muted)] leading-relaxed">{t('home.panelTip')}</p>
                </div>
              </div>
            ) : rightTab === 'files' ? (
              <FileTreePanel
                key={currentProject.id}
                tree={fileTree}
                building={indexBuilding}
                projectId={currentProject.id}
                projectPath={currentProject.path}
                dirCache={dirCache}
                onLoadDir={(path) => loadDirChildren(path)}
                onRefresh={() => rebuildIndex()}
                onReference={handleReference}
                onReferenceSelection={handleReferenceSelection}
              />
            ) : rightTab === 'memories' ? (
              <MemoriesPanel
                memories={memories}
                onSave={(input) => saveMemory(input)}
                onDelete={(id) => deleteMemory(id)}
                onToggle={(id, enabled) => setMemoryEnabled(id, enabled)}
                onRefresh={() => loadMemories()}
              />
            ) : rightTab === 'git' ? (
              <GitPanel project={currentProject} onProjectUpdated={() => refreshProjects()} />
            ) : rightTab === 'preview' ? (
              <PreviewPanel
                url={previewUrl}
                setUrl={setPreviewUrl}
                src={previewSrc}
                onOpen={handleOpenPreview}
              />
            ) : rightTab === 'devices' ? (
              <DevicesPanel projectId={currentProject?.id} onChanged={() => {}} />
            ) : rightTab === 'analyze' ? (
              <AnalyzePanel
                key={currentProject.id}
                projectPath={currentProject.path}
                projectId={currentProject.id}
                projectName={currentProject.name}
                onRunBuild={handleOpenTerminal}
                onFixErrors={handleFixBuildErrors}
                onAutoFix={handleAutoFixErrors}
                refreshTick={analyzeRefreshTick}
                moduleScanTick={moduleScanTick}
                agentBusy={isStreaming}
                onHarmonyRootChanged={() => refreshProjects()}
              />
            ) : rightTab === 'symbols' ? (
              <SymbolsPanel
                key={currentProject.id}
                projectId={currentProject.id}
                projectName={currentProject.name}
                onReference={handleReference}
              />
            ) : rightTab === 'stats' ? (
              <ToolStatsPanel stats={toolStats} onRefresh={() => loadToolStats()} />
            ) : (
              <TerminalPanel
                entries={terminalEntries}
                onClear={clearTerminal}
                buildLogs={buildLogs}
                onClearBuild={clearBuildLogs}
              />
            )}
          </div>
        </aside>
        </>
      )}

      {/* ============ 对话框 ============ */}
      {showAddDialog && <AddProjectDialog onConfirm={handleAddConfirm} onClose={() => setShowAddDialog(false)} />}
      {pendingTrust && (
        <TrustDialog
          inspect={pendingTrust.inspect}
          onTrust={handleTrust}
          onReject={handleReject}
          busy={trustBusy}
        />
      )}

      {/* ============ 计划/审查模式：任务计划确认卡片 ============ */}
      {pendingPlan && (
        <div className="fixed bottom-24 left-1/2 -translate-x-1/2 z-[55] w-[640px] max-w-[calc(100vw-2rem)]">
          <div className="rounded-2xl border border-[var(--accent)]/40 bg-[var(--bg-elevated)]/95 backdrop-blur shadow-2xl shadow-black/20 animate-modal-in overflow-hidden">
            <div className="flex items-center gap-2 px-4 py-2.5 border-b border-[var(--border)] bg-[var(--accent-soft)]">
              <span className="w-2 h-2 rounded-full bg-[var(--accent)] animate-pulse" />
              <span className="text-[13px] font-semibold text-[var(--accent)]">{t('home.planReviewTitle')}</span>
              <span className="text-[11px] text-[var(--text-muted)] ml-auto">{t('home.planReviewHint')}</span>
              <button
                onClick={() => setPlanEditing((v) => !v)}
                className="ml-1 flex items-center gap-1 px-2 h-6 rounded-md text-[11px] font-medium text-[var(--text-secondary)] hover:text-[var(--accent)] hover:bg-[var(--bg-hover)] transition-colors"
                title={t('home.planEdit')}
              >
                <Icon name={planEditing ? 'check' : 'edit'} size={11} />
                {planEditing ? t('home.planDone') : t('home.planEdit')}
              </button>
            </div>
            {planEditing ? (
              <div className="max-h-56 overflow-y-auto px-4 py-3">
                <textarea
                  value={planDraft}
                  onChange={(e) => setPlanDraft(e.target.value)}
                  rows={10}
                  spellCheck={false}
                  className="w-full rounded-lg bg-[var(--bg-primary)] border border-[var(--border)] px-3 py-2 text-[12px] leading-relaxed outline-none resize-y font-mono placeholder:text-[var(--text-muted)]/60 focus:border-[var(--accent)] transition-colors"
                />
              </div>
            ) : (
              <div className="max-h-56 overflow-y-auto px-4 py-3">
                <Markdown>{pendingPlan.plan}</Markdown>
              </div>
            )}
            <div className="px-4 pb-3">
              <textarea
                value={planFeedback}
                onChange={(e) => setPlanFeedback(e.target.value)}
                placeholder={t('home.planFeedbackPlaceholder')}
                rows={2}
                className="w-full rounded-lg bg-[var(--bg-primary)] border border-[var(--border)] px-3 py-2 text-[12px] outline-none resize-none placeholder:text-[var(--text-muted)]/60 focus:border-[var(--accent)] transition-colors"
              />
            </div>
            <div className="flex items-center justify-end gap-2 px-4 py-3 border-t border-[var(--border)] bg-[var(--bg-card)]">
              <button
                onClick={() => {
                  // 驳回：若编辑过计划，把修订稿作为意见一并反馈
                  const edited = planDraft.trim()
                  const original = pendingPlan.plan.trim()
                  const extra = edited && edited !== original ? `用户修改后的计划草案：\n${edited}` : ''
                  const fb = [planFeedback.trim(), extra].filter(Boolean).join('\n\n')
                  void resolvePlanReview(pendingPlan.requestId, false, fb || undefined)
                  setPlanFeedback('')
                }}
                className="h-8 px-4 rounded-lg border border-[var(--border)] text-[12px] font-medium text-[var(--text-secondary)] hover:text-[var(--warning)] hover:border-[var(--warning)]/50 transition-colors"
              >
                {t('home.planReject')}
              </button>
              <button
                onClick={() => {
                  // 批准：若编辑过计划，把修订稿作为执行要求注入；否则纯批准
                  const edited = planDraft.trim()
                  const original = pendingPlan.plan.trim()
                  let fb = planFeedback.trim()
                  if (edited && edited !== original) {
                    fb = [fb, `用户修订后的最终计划（请严格据此执行）：\n${edited}`]
                      .filter(Boolean)
                      .join('\n\n')
                  }
                  void resolvePlanReview(pendingPlan.requestId, true, fb || undefined)
                  setPlanFeedback('')
                }}
                className="h-8 px-5 rounded-lg bg-[var(--accent)] text-white text-[12px] font-medium hover:bg-[var(--accent-hover)] active:scale-[0.98] transition-all flex items-center gap-1.5"
              >
                <Icon name="check" size={14} />
                {t('home.planApprove')}
              </button>
            </div>
          </div>
        </div>
      )}

      {/* ============ 已批准任务计划（执行中锚点，可收起） ============ */}
      {approvedPlan && approvedPlan.conversationId === currentConversation?.id && (
        <div className="fixed bottom-24 left-1/2 -translate-x-1/2 z-[54] w-[640px] max-w-[calc(100vw-2rem)]">
          <details className="rounded-xl border border-[var(--success)]/40 bg-[var(--bg-elevated)]/95 backdrop-blur shadow-lg shadow-black/10 animate-modal-in overflow-hidden" open={false}>
            <summary className="flex items-center gap-2 px-4 py-2 cursor-pointer select-none list-none [&::-webkit-details-marker]:hidden">
              <Icon name="check" size={13} className="text-[var(--success)] shrink-0" />
              <span className="text-[12px] font-semibold text-[var(--success)]">{t('home.approvedPlanTitle')}</span>
              <span className="text-[11px] text-[var(--text-muted)]">{t('home.approvedPlanHint')}</span>
            </summary>
            <div className="max-h-48 overflow-y-auto px-4 pb-3 border-t border-[var(--border)]">
              <Markdown>{approvedPlan.plan}</Markdown>
            </div>
          </details>
        </div>
      )}

      {/* ============ Agent 诊断引导卡片（签名/SDK/依赖等需用户操作） ============ */}
      {diagnoseCards.filter((c) => c.conversationId === currentConversation?.id).map((card) => {
        const icon = card.category === 'signing' ? 'info' : card.category === 'sdk' ? 'package' : card.category === 'dependency' ? 'download' : 'info'
        const color = card.category === 'signing' ? 'text-amber-500 border-amber-500/40 bg-amber-500/10'
          : card.category === 'sdk' ? 'text-blue-500 border-blue-500/40 bg-blue-500/10'
          : card.category === 'dependency' ? 'text-[var(--accent)] border-[var(--accent)]/40 bg-[var(--accent-soft)]'
          : 'text-[var(--text-secondary)] border-[var(--border)] bg-[var(--bg-card)]'
        const actionLabel = card.action === 'install_deps' ? t('home.diagnoseInstallDeps')
          : card.action === 'open_sdk_manager' ? t('home.diagnoseOpenSdk')
          : card.action === 'open_signing_config' ? t('home.diagnoseOpenSigning')
          : ''
        return (
          <div key={card.id} className={`fixed bottom-24 right-4 z-[53] w-[320px] rounded-xl border bg-[var(--bg-elevated)]/95 backdrop-blur shadow-lg shadow-black/10 animate-modal-in overflow-hidden ${color}`}>
            <div className="flex items-start gap-2 p-3">
              <Icon name={icon as React.ComponentProps<typeof Icon>['name']} size={14} className="shrink-0 mt-0.5" />
              <div className="flex-1 min-w-0">
                <div className="text-[12px] font-semibold">{card.title}</div>
                <div className="text-[10.5px] text-[var(--text-secondary)] mt-1 leading-relaxed whitespace-pre-wrap break-words">{card.message}</div>
              </div>
              <button type="button" onClick={() => handleDiagnoseDismiss(card)} className="text-[var(--text-muted)] hover:text-[var(--text-secondary)] shrink-0">
                <Icon name="close" size={12} />
              </button>
            </div>
            {actionLabel && (
              <div className="px-3 pb-3 flex justify-end gap-1.5">
                <button type="button" onClick={() => handleDiagnoseDismiss(card)} className="h-7 px-2.5 rounded-lg text-[10.5px] text-[var(--text-muted)] hover:bg-[var(--bg-hover)] transition-colors">
                  {t('home.diagnoseLater')}
                </button>
                <button type="button" onClick={() => void handleDiagnoseAction(card)} className="h-7 px-3 rounded-lg text-[10.5px] font-medium bg-[var(--accent)] text-white hover:opacity-90 transition-opacity">
                  {actionLabel}
                </button>
              </div>
            )}
          </div>
        )
      })}

      {/* ============ 任务清单（todo_write 工具，可收起，实时进度） ============ */}
      {todos.length > 0 && (() => {
        const done = todos.filter((t) => t.status === 'done').length
        const pct = Math.round((done / todos.length) * 100)
        return (
          <div
            className="fixed bottom-24 z-[52] w-[280px] max-w-[calc(100vw-2rem)]"
            style={{ left: sidebarCollapsed ? 16 : sidebarWidth + 16 }}
          >
            <details className="rounded-xl border border-[var(--border)] bg-[var(--bg-elevated)]/95 backdrop-blur shadow-lg shadow-black/10 animate-modal-in overflow-hidden" open={false}>
              <summary className="flex items-center gap-2 px-3 py-2 cursor-pointer select-none list-none [&::-webkit-details-marker]:hidden">
                <Icon name="check" size={12} className="text-[var(--accent)] shrink-0" />
                <span className="text-[12px] font-semibold">{t('home.todoTitle')}</span>
                <span className="text-[11px] text-[var(--text-muted)]">{done}/{todos.length}</span>
                <div className="ml-auto h-1.5 w-14 rounded-full bg-[var(--bg-hover)] overflow-hidden">
                  <div className="h-full rounded-full bg-[var(--accent)] transition-all" style={{ width: `${pct}%` }} />
                </div>
              </summary>
              <div className="px-3 pb-3 pt-1 border-t border-[var(--border)] max-h-56 overflow-y-auto space-y-1.5">
                {todos.map((t) => (
                  <div key={t.id} className="flex items-start gap-2 text-[11.5px] leading-relaxed">
                    {t.status === 'done' ? (
                      <Icon name="check" size={11} className="text-[var(--success)] shrink-0 mt-0.5" />
                    ) : t.status === 'in_progress' ? (
                      <span className="w-2.5 h-2.5 rounded-full border-2 border-[var(--accent)] bg-[var(--accent)]/30 animate-pulse shrink-0 mt-0.5" />
                    ) : (
                      <span className="w-2.5 h-2.5 rounded-full border border-[var(--border)] shrink-0 mt-0.5" />
                    )}
                    <span
                      className={
                        t.status === 'done'
                          ? 'line-through text-[var(--text-muted)]'
                          : t.status === 'in_progress'
                            ? 'text-[var(--accent)] font-medium'
                            : 'text-[var(--text-secondary)]'
                      }
                    >
                      {t.content}
                    </span>
                  </div>
                ))}
              </div>
            </details>
          </div>
        )
      })()}

      {/* ============ Agent 提问卡（ask_user 工具，自由文本回答闭环） ============ */}
      {askCard && askCard.conversationId === currentConversation?.id && (
        <div className="fixed inset-0 z-[62] flex items-center justify-center bg-black/30 backdrop-blur-[2px]">
          <div className="w-[480px] max-w-[92vw] rounded-2xl border border-[var(--border)] bg-[var(--bg-secondary)] shadow-2xl p-4 animate-modal-in">
            <div className="flex items-center gap-2">
              <Icon name="headphones" size={14} className="text-[var(--accent)] shrink-0" />
              <span className="text-[13px] font-semibold">{t('home.askTitle')}</span>
              <span className="text-[10px] px-2 py-0.5 rounded-md bg-[var(--accent-soft)] text-[var(--accent)] font-mono ml-auto">ask_user</span>
            </div>
            <div className="mt-3 text-[13px] leading-relaxed whitespace-pre-wrap break-words">{askCard.question}</div>
            {askCard.options.length > 0 && (
              <div className="mt-2.5 flex flex-wrap gap-1.5">
                {askCard.options.map((opt) => (
                  <button
                    key={opt}
                    type="button"
                    onClick={() => setAskAnswer(opt)}
                    className="h-7 px-2.5 rounded-lg border border-[var(--border)] text-[11px] text-[var(--text-secondary)] hover:text-[var(--accent)] hover:border-[var(--accent)]/50 bg-[var(--bg-card)] transition-colors"
                  >
                    {opt}
                  </button>
                ))}
              </div>
            )}
            <textarea
              value={askAnswer}
              onChange={(e) => setAskAnswer(e.target.value)}
              onKeyDown={(e) => {
                // Ctrl/Cmd+Enter 提交
                if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) {
                  e.preventDefault()
                  handleAskSubmit()
                }
              }}
              placeholder={t('home.askPlaceholder')}
              rows={3}
              autoFocus
              spellCheck={false}
              className="mt-3 w-full rounded-lg bg-[var(--bg-card)] border border-[var(--border)] px-3 py-2 text-[12px] outline-none resize-none placeholder:text-[var(--text-muted)]/60 focus:border-[var(--accent)] transition-colors"
            />
            <div className="mt-3 flex items-center justify-end gap-2">
              <span className="text-[10.5px] text-[var(--text-muted)] mr-auto">Ctrl+Enter ↵</span>
              <button
                type="button"
                onClick={handleAskSkip}
                className="h-8 px-4 rounded-lg border border-[var(--border)] text-[12px] font-medium text-[var(--text-secondary)] hover:bg-[var(--bg-hover)] transition-colors"
              >
                {t('home.askSkip')}
              </button>
              <button
                type="button"
                onClick={handleAskSubmit}
                className="h-8 px-5 rounded-lg bg-[var(--accent)] text-white text-[12px] font-medium hover:bg-[var(--accent-hover)] active:scale-[0.98] transition-all flex items-center gap-1.5"
              >
                <Icon name="send" size={12} />
                {t('home.askSubmit')}
              </button>
            </div>
          </div>
        </div>
      )}

      {/* ============ 工具权限审核浮层（自动审核模式，逐个确认） ============ */}
      {toolApprovals.length > 0 && (() => {
        const risk = approvalRisk(toolApprovals[0].tool)
        return (
        <div className="fixed inset-0 z-[60] flex items-center justify-center bg-black/30 backdrop-blur-[2px]">
          <div className="w-[460px] max-w-[92vw] rounded-2xl border border-[var(--border)] bg-[var(--bg-secondary)] shadow-2xl p-4 animate-modal-in">
            <div className="flex items-center gap-2">
              <span className="w-2 h-2 rounded-full bg-[var(--warning)] animate-pulse" />
              <span className="text-[13px] font-semibold">{t('home.toolApprovalTitle')}</span>
              <span className={`text-[10px] px-2 py-0.5 rounded-md font-medium ml-auto ${risk.cls}`}>{risk.label}</span>
            </div>
            <div className="mt-3 space-y-2">
              <div className="flex items-center gap-2">
                <span className="text-[10px] px-2 py-0.5 rounded-md bg-[var(--accent-soft)] text-[var(--accent)] font-mono shrink-0">
                  {toolApprovals[0].tool}
                </span>
                <span className="text-[11px] text-[var(--text-muted)] truncate">
                  {t('home.toolApprovalArgs')}
                </span>
              </div>
              <pre className="tool-output max-h-32 overflow-y-auto rounded-lg bg-[var(--bg-card)] border border-[var(--border)] p-2.5 text-[11px] font-mono whitespace-pre-wrap break-all text-[var(--text-primary)]">
                {toolApprovals[0].args || '{}'}
              </pre>
              <textarea
                value={approvalFeedback}
                onChange={(e) => setApprovalFeedback(e.target.value)}
                placeholder={t('home.toolApprovalFeedbackPlaceholder')}
                rows={2}
                className="w-full rounded-lg bg-[var(--bg-card)] border border-[var(--border)] px-3 py-2 text-[11px] outline-none resize-none placeholder:text-[var(--text-muted)]/60 focus:border-[var(--accent)] transition-colors"
              />
              {/* 记忆范围：仅本次 / 本会话 / 本项目持久化（白名单跨会话重启生效） */}
              <div className="flex items-center gap-3 select-none">
                {([
                  { v: '', label: t('home.toolApprovalOnce') },
                  { v: 'session', label: t('home.toolApprovalRemember') },
                  { v: 'project', label: t('home.toolApprovalProject') },
                ] as const).map((opt) => (
                  <label key={opt.v || 'once'} className="flex items-center gap-1.5 cursor-pointer">
                    <input
                      type="radio"
                      name="approval-scope"
                      checked={approvalScope === opt.v}
                      onChange={() => setApprovalScope(opt.v)}
                      className="w-3 h-3 accent-[var(--accent)]"
                    />
                    <span className={`text-[11px] ${approvalScope === opt.v ? 'text-[var(--accent)] font-medium' : 'text-[var(--text-secondary)]'}`}>
                      {opt.label}
                    </span>
                  </label>
                ))}
              </div>
              {/* 白名单管理入口：查看/移除本项目已永久放行的工具 */}
              <button
                onClick={() => void openWhitelistDialog()}
                className="text-[11px] text-[var(--text-muted)] hover:text-[var(--accent)] transition-colors flex items-center gap-1"
              >
                <Icon name="check" size={10} />
                {t('home.whitelistManage')}
              </button>
            </div>
            <div className="flex items-center justify-end gap-2 mt-4">
              <button
                onClick={() => resolveToolApproval(toolApprovals[0].requestId, false, false, approvalFeedback || undefined)}
                className="h-8 px-4 rounded-lg border border-[var(--border)] text-[12px] font-medium text-[var(--text-secondary)] hover:text-[var(--danger)] hover:border-[var(--danger)]/50 transition-colors"
              >
                {t('home.toolApprovalReject')}
              </button>
              <button
                onClick={() => resolveToolApproval(toolApprovals[0].requestId, true, approvalScope !== '', undefined, approvalScope || undefined)}
                className="h-8 px-4 rounded-lg bg-[var(--accent)] text-white text-[12px] font-medium hover:bg-[var(--accent-hover)] active:scale-[0.98] transition-all"
              >
                {t('home.toolApprovalAllow')}
              </button>
            </div>
          </div>
        </div>
        )
      })()}

      {/* ============ 划词菜单（选中文本弹出） ============ */}
      {selectionMenu && (
        <div
          className="fixed z-[70] animate-modal-in"
          style={{ left: selectionMenu.x, top: selectionMenu.y, transform: 'translateX(-50%)' }}
          onMouseDown={(e) => {
            // 阻止默认行为：防止按钮聚焦/点击清除选区高亮；并拦截冒泡避免全局 mouseup 重定位菜单
            e.preventDefault()
            e.stopPropagation()
            // 立即恢复完整快照：mousedown 瞬间浏览器可能已把跨格式选区收缩（只剩第一个格式块），
            // 此时读取当前选区会拿到收缩版并覆盖完整快照，因此绝不刷新快照，直接恢复
            restoreSelectionRange(selectionRangeRef.current, selectionTextRef.current, selectionContainerRef.current)
          }}
          onMouseUp={(e) => e.stopPropagation()}
        >
          <div className="flex items-center gap-0.5 rounded-xl border border-[var(--border)] bg-[var(--bg-card)] shadow-2xl shadow-black/40 py-1 px-1">
            <button
              onClick={copySelection}
              className="flex items-center gap-1.5 px-2.5 py-1.5 rounded-lg text-[11px] text-[var(--text-secondary)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] transition-colors"
            >
              <Icon name="check" size={12} />
              {t('home.selectionCopy')}
            </button>
            <button
              onClick={() => sendWithInstruction(t('home.selectionExplain'), selectionMenu.text)}
              className="flex items-center gap-1.5 px-2.5 py-1.5 rounded-lg text-[11px] text-[var(--text-secondary)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] transition-colors"
            >
              <Icon name="lightbulb" size={12} />
              {t('home.selectionExplain')}
            </button>
            <button
              onClick={() => sendWithInstruction(t('home.selectionTranslate'), selectionMenu.text)}
              className="flex items-center gap-1.5 px-2.5 py-1.5 rounded-lg text-[11px] text-[var(--text-secondary)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] transition-colors"
            >
              <Icon name="language" size={12} />
              {t('home.selectionTranslate')}
            </button>
            <button
              onClick={searchSelection}
              className="flex items-center gap-1.5 px-2.5 py-1.5 rounded-lg text-[11px] text-[var(--text-secondary)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] transition-colors"
            >
              <Icon name="notifications" size={12} />
              {t('home.selectionSearch')}
            </button>
            <button
              onClick={quoteSelection}
              className="flex items-center gap-1.5 px-2.5 py-1.5 rounded-lg text-[11px] text-[var(--text-secondary)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] transition-colors"
            >
              <Icon name="chat" size={12} />
              {t('home.selectionQuote')}
            </button>
          </div>
        </div>
      )}

      {/* ============ 点踩反馈弹窗（可选原因） ============ */}
      {feedbackDialog && (
        <FeedbackDialog
          onCancel={() => setFeedbackDialog(null)}
          onSubmit={(reason) => {
            if (feedbackDialog) {
              rateMessage(feedbackDialog.messageId, 'dislike', reason)
            }
            setFeedbackDialog(null)
          }}
        />
      )}

      {/* ============ 回复版本 diff 对比弹窗 ============ */}
      {versionDialog && (
        <VersionDiffDialog
          userMessageId={versionDialog.userMessageId}
          current={versionDialog.current}
          onClose={() => setVersionDialog(null)}
        />
      )}

      {/* ============ 编辑消息弹窗（user 消息编辑 = 重新执行任务） ============ */}
      {/* ============ 项目审批白名单管理弹窗 ============ */}
      {whitelistOpen && currentProject && (
        <div className="fixed inset-0 z-[80] flex items-center justify-center bg-black/30 backdrop-blur-[2px]">
          <div className="w-[460px] max-w-[92vw] rounded-2xl border border-[var(--border)] bg-[var(--bg-secondary)] shadow-2xl p-4 animate-modal-in">
            <div className="flex items-center gap-2">
              <Icon name="check" size={15} />
              <span className="text-[13px] font-semibold">{t('home.whitelistTitle')}</span>
              <button
                onClick={() => setWhitelistOpen(false)}
                className="ml-auto p-1 rounded-lg text-[var(--text-muted)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] transition-colors"
              >
                <Icon name="close" size={13} />
              </button>
            </div>
            <div className="mt-3 space-y-1 max-h-72 overflow-y-auto">
              {whitelist.length === 0 && (
                <div className="text-[11px] text-[var(--text-muted)] py-4 text-center">{t('home.whitelistEmpty')}</div>
              )}
              {whitelist.map((w) => (
                <div
                  key={w.tool}
                  className="flex items-center gap-2 rounded-lg bg-[var(--bg-card)] border border-[var(--border)] px-2.5 py-1.5"
                >
                  <span className="text-[11px] font-mono text-[var(--text-primary)] flex-1 truncate">{w.tool}</span>
                  <span className="text-[10px] text-[var(--text-muted)] shrink-0">{formatTime(w.created_at)}</span>
                  <button
                    onClick={() => {
                      removeToolWhitelist(currentProject.id, w.tool)
                        .then(() => setWhitelist((l) => l.filter((x) => x.tool !== w.tool)))
                        .catch(() => {})
                    }}
                    className="p-0.5 rounded text-[var(--text-muted)] hover:text-[var(--danger)] hover:bg-[var(--bg-hover)] transition-colors shrink-0"
                    title={t('home.whitelistRemove')}
                  >
                    <Icon name="delete" size={12} />
                  </button>
                </div>
              ))}
            </div>
          </div>
        </div>
      )}

      {editTarget && (
        <EditMessageDialog
          message={editTarget}
          onCancel={() => setEditTarget(null)}
          onSubmit={async (content) => {
            if (editTarget.role === 'user') {
              // 编辑用户消息 = 重新执行任务：任务已执行过，直接保存无意义。
              // 删除该消息及其后的所有消息，再以编辑后的内容重新流式执行
              const refs = (() => {
                try {
                  const v = editTarget.references_json ? JSON.parse(editTarget.references_json) : null
                  return Array.isArray(v) ? (v as string[]) : undefined
                } catch {
                  return undefined
                }
              })()
              setEditTarget(null)
              try {
                await removeMessage(editTarget.id)
              } catch {
                // 删除失败（如消息已不存在）：仍按新内容重跑，避免任务无法继续
              }
              await sendUserMessage(content, undefined, refs)
            } else {
              // 助手消息编辑：仅更新内容展示（不可重跑）
              await editMessage(editTarget.id, content)
              setEditTarget(null)
            }
          }}
        />
      )}

      {/* ============ 记忆总结确认弹窗 ============ */}
      {memoryDraft && (
        <MemoryDraftDialog
          draft={memoryDraft}
          onCancel={() => setMemoryDraft(null)}
          onConfirm={confirmSaveMemory}
        />
      )}

      {/* ============ Rules 编辑弹窗（全局指令 + 项目级 rules） ============ */}
      {showRulesDialog && (
        <RulesDialog
          tab={rulesTab}
          setTab={setRulesTab}
          globalText={rulesGlobalText}
          setGlobalText={setRulesGlobalText}
          projectText={rulesProjectText}
          setProjectText={setRulesProjectText}
          saving={rulesSaving}
          onSave={saveRules}
          onClose={() => setShowRulesDialog(false)}
        />
      )}

      {/* ============ 运行时异常提示（部署后监听捕获，一键修复） ============ */}
      {runtimeAnomaly && (
        <div className="fixed bottom-6 right-6 z-50 w-[400px] max-w-[92vw] rounded-xl bg-[var(--bg-card)] border border-red-500/60 shadow-2xl p-4 space-y-3">
          <div className="flex items-start gap-2">
            <span className="text-red-500 text-[16px] leading-none mt-0.5">⚠️</span>
            <div className="flex-1 min-w-0">
              <h3 className="text-[13px] font-semibold">{t('runtime.anomalyTitle')}</h3>
              <p className="text-[11px] text-[var(--text-muted)] mt-0.5">{t('runtime.anomalyHint', { category: runtimeAnomaly.category })}</p>
            </div>
            <button
              onClick={() => setRuntimeAnomaly(null)}
              className="text-[var(--text-muted)] hover:text-[var(--text-primary)] text-[14px] leading-none"
            >
              ×
            </button>
          </div>
          <div className="rounded-lg bg-[var(--bg-secondary)] border border-[var(--border)] p-2.5 space-y-1.5 max-h-[200px] overflow-auto">
            <div className="text-[12px] font-medium break-words">{runtimeAnomaly.summary}</div>
            <pre className="text-[11px] text-[var(--text-muted)] whitespace-pre-wrap break-words font-mono leading-relaxed">
              {runtimeAnomaly.detail.slice(0, 800)}
            </pre>
          </div>
          <div className="flex justify-end gap-2">
            <button
              onClick={() => setRuntimeAnomaly(null)}
              className="h-8 px-3 rounded-lg border border-[var(--border)] text-[12px] hover:bg-[var(--bg-hover)]"
            >
              {t('runtime.dismiss')}
            </button>
            <button
              onClick={() => void fixRuntimeAnomaly()}
              className="h-8 px-4 rounded-lg bg-red-500 text-white text-[12px] hover:bg-red-600"
            >
              {t('runtime.fixNow')}
            </button>
          </div>
        </div>
      )}

      {/* ============ 修复经验候选弹窗（构建/部署由失败转成功时） ============ */}
      {knowledgeCandidate && (
        <div
          className="fixed bottom-6 right-6 z-50 w-[380px] max-w-[92vw] rounded-xl bg-[var(--bg-card)] border border-[var(--border)] shadow-2xl p-4 space-y-3"
        >
          <div className="flex items-start gap-2">
            <span className="text-[var(--accent)] text-[16px] leading-none mt-0.5">💡</span>
            <div className="flex-1 min-w-0">
              <h3 className="text-[13px] font-semibold">{t('knowledge.candidateTitle')}</h3>
              <p className="text-[11px] text-[var(--text-muted)] mt-0.5">{t('knowledge.candidateHint')}</p>
            </div>
            <button
              onClick={() => setKnowledgeCandidate(null)}
              className="text-[var(--text-muted)] hover:text-[var(--text-primary)] text-[14px] leading-none"
            >
              ×
            </button>
          </div>
          <div className="space-y-2">
            <input
              value={knowledgeCandidate.title}
              onChange={(e) => setKnowledgeCandidate({ ...knowledgeCandidate, title: e.target.value })}
              placeholder={t('knowledge.entryTitle')}
              className="w-full h-8 px-2.5 rounded-lg bg-[var(--bg-secondary)] border border-[var(--border)] text-[12px] outline-none focus:border-[var(--accent)]"
            />
            <textarea
              value={knowledgeCandidate.error_text}
              onChange={(e) => setKnowledgeCandidate({ ...knowledgeCandidate, error_text: e.target.value })}
              rows={3}
              placeholder={t('knowledge.candidateErrorPh')}
              className="w-full px-2.5 py-1.5 rounded-lg bg-[var(--bg-secondary)] border border-[var(--border)] text-[12px] outline-none focus:border-[var(--accent)] resize-y"
            />
            <textarea
              value={knowledgeCandidate.fix}
              onChange={(e) => setKnowledgeCandidate({ ...knowledgeCandidate, fix: e.target.value })}
              rows={4}
              placeholder={t('knowledge.candidateFixPh')}
              className="w-full px-2.5 py-1.5 rounded-lg bg-[var(--bg-secondary)] border border-[var(--border)] text-[12px] outline-none focus:border-[var(--accent)] resize-y"
            />
          </div>
          <div className="flex justify-end gap-2">
            <button
              onClick={() => setKnowledgeCandidate(null)}
              className="h-8 px-3 rounded-lg border border-[var(--border)] text-[12px] hover:bg-[var(--bg-hover)]"
            >
              {t('knowledge.candidateDismiss')}
            </button>
            <button
              onClick={saveKnowledgeCandidate}
              disabled={candidateSaving}
              className="h-8 px-4 rounded-lg bg-[var(--accent)] text-white text-[12px] hover:bg-[var(--accent-hover)] disabled:opacity-50"
            >
              {t('knowledge.candidateSave')}
            </button>
          </div>
        </div>
      )}

      {/* ============ Web 预览已迁移至右侧栏 Preview 面板 ============ */}
    </div>
  )
}


/* ============ 消息（无气泡，Claude/Qoder 风格） ============ */
function MessageItem({
  message,
  time,
  userMessageId,
  isLastAssistant,
  onRegenerate,
  onBranch,
  onRate,
  onDislike,
  onOpenVersions,
  onSpeak,
  speaking,
  onEditMessage,
  onDeleteMessage,
  confirmDeleteMsgId,
  projectPath,
  highlighted,
  onOpenFile,
}: {
  message: ChatMessage
  time: string
  userMessageId: string
  isLastAssistant?: boolean
  onRegenerate?: () => void
  /** 从该 user 消息分支重生成（保留旧回复为版本历史） */
  onBranch?: (message: ChatMessage) => void
  onRate: (messageId: string, feedback: 'like' | 'dislike' | 'neutral', reason?: string) => void
  onDislike: (messageId: string) => void
  onOpenVersions: (message: ChatMessage) => void
  onSpeak: (messageId: string, text: string) => void
  speaking: boolean
  onEditMessage?: (message: ChatMessage) => void
  onDeleteMessage?: (message: ChatMessage) => void
  confirmDeleteMsgId?: string | null
  /** 项目路径（变更审查用；非 git 项目时为 undefined，保持只读列表） */
  projectPath?: string
  /** 搜索命中高亮：消息卡片闪烁背景 3 秒 */
  highlighted?: boolean
  /** 代码块文件路径点击：在项目中定位/引用该文件 */
  onOpenFile?: (path: string) => void
}) {
  const { t } = useTranslation()
  const feedbackMap = useProjectStore((s) => s.feedbackMap)
  const versionMap = useProjectStore((s) => s.versionMap)
  const { role, content, reasoning, model } = message
  const [copied, setCopied] = useState(false)
  // “记住这次修复”弹窗：把当前对话里的错误+解法沉淀为知识库条目
  const [rememberOpen, setRememberOpen] = useState(false)
  const [rememberSaving, setRememberSaving] = useState(false)
  const [rememberForm, setRememberForm] = useState({ title: '', error_text: '', fix: '' })
  // 分支展开面板：对话内切换预览历史版本（预览为临时视图，不落库）
  const [branchOpen, setBranchOpen] = useState(false)
  const [previewVersionId, setPreviewVersionId] = useState<string | null>(null)

  // 本回复修改的文件列表（ChatGPT 式折叠卡片；edit_file/write_file 收集，相对路径）
  const modifiedFiles = useMemo(() => {
    if (!message.modified_files_json) return []
    try {
      const v = JSON.parse(message.modified_files_json)
      return Array.isArray(v) ? v.filter((x) => typeof x === 'string') : []
    } catch {
      return []
    }
  }, [message.modified_files_json])

  // 本条 user 消息的 @ 引用列表（references_json，气泡下方标签展示）
  const userRefs = useMemo(() => {
    if (!message.references_json) return []
    try {
      const v = JSON.parse(message.references_json)
      return Array.isArray(v) ? v.filter((x) => typeof x === 'string') : []
    } catch {
      return []
    }
  }, [message.references_json])

  // 注：role === 'tool' 的消息已在 renderItems 中合并为 ToolRunGroup 折叠组，不会进入本组件

  const isUser = role === 'user'
  const feedback = feedbackMap[message.id]
  const versions = versionMap[userMessageId] ?? []
  const previewVersion = previewVersionId ? versions.find((v) => v.id === previewVersionId) ?? null : null
  const displayContent = previewVersion ? previewVersion.content : content
  const displayReasoning = previewVersion ? (previewVersion.reasoning ?? undefined) : reasoning

  /** 复制消息内容（带 1.5s 成功反馈） */
  const copyMessage = async () => {
    try {
      await navigator.clipboard.writeText(displayContent)
      setCopied(true)
      setTimeout(() => setCopied(false), 1500)
    } catch {
      // 剪贴板不可用时静默失败
    }
  }

  const openRemember = () => {
    // 默认把助手回复的纯文本作为“修复方法”，标题/错误现象留空让用户补充
    const plainFix = displayContent
      .replace(/```[\s\S]*?```/g, ' ')
      .replace(/[#*`>_~-]/g, ' ')
      .replace(/\s+/g, ' ')
      .trim()
      .slice(0, 600)
    setRememberForm({ title: '', error_text: '', fix: plainFix })
    setRememberOpen(true)
  }

  const saveRemember = async () => {
    if (!rememberForm.error_text.trim()) {
      alert(t('knowledge.rememberNeedError'))
      return
    }
    setRememberSaving(true)
    try {
      const pid = useProjectStore.getState().currentProject?.id ?? null
      await saveKnowledgeFromText(
        {
          title: rememberForm.title || undefined,
          error_text: rememberForm.error_text,
          fix: rememberForm.fix,
        },
        pid,
      )
      setRememberOpen(false)
    } catch (e) {
      alert(String(e))
    } finally {
      setRememberSaving(false)
    }
  }

  if (isUser) {
    const queued = message.queued === 1
    return (
      <div
        data-msg-id={message.id}
        className={`flex justify-end gap-2.5 group animate-fade-in-up ${highlighted ? 'msg-highlight' : ''}`}
      >
        <div className="max-w-[80%] rounded-2xl rounded-tr-md bg-[var(--accent-soft)] border border-[var(--accent)]/20 px-4 py-2.5 shadow-sm transition-shadow hover:shadow-md">
          <div className="flex items-center gap-2 mb-1">
            <span className="text-[11px] font-medium text-[var(--text-secondary)]">{t('home.you')}</span>
            <span className="text-[10px] text-[var(--text-muted)]">{time}</span>
            {queued && (
              <span className="text-[10px] px-1.5 py-0.5 rounded-md bg-[var(--accent)]/10 text-[var(--accent)]">
                {message.agent_owned === 1 ? t('home.queuedAgentLabel') : t('home.queuedLabel')}
              </span>
            )}
            {/* @ 引用标签（references_json 落库展示） */}
            {userRefs.length > 0 && (
              <span
                className="flex items-center gap-1 text-[10px] text-[var(--text-muted)] max-w-52 overflow-hidden"
                title={userRefs.join('\n')}
              >
                <Icon name="file" size={10} className="shrink-0" />
                <span className="truncate">{userRefs.join(', ')}</span>
              </span>
            )}
            <div className="ml-auto flex items-center gap-0.5 opacity-0 group-hover:opacity-100 transition-opacity">
              <button
                onClick={copyMessage}
                className={`text-[10px] rounded-md px-1.5 py-0.5 flex items-center gap-0.5 ${
                  copied ? 'text-[var(--success)]' : 'text-[var(--text-muted)] hover:text-[var(--accent)] hover:bg-[var(--bg-hover)]'
                }`}
                title={t('home.copyMessage')}
              >
                {copied ? <><Icon name="check" size={11} />{t('home.copied')}</> : <><Icon name="copy" size={11} />{t('home.copyMessage')}</>}
              </button>
              {onEditMessage && (
                <button
                  onClick={() => onEditMessage(message)}
                  className="text-[10px] text-[var(--text-muted)] hover:text-[var(--accent)] hover:bg-[var(--bg-hover)] rounded-md px-1.5 py-0.5"
                  title={t('home.editMessage')}
                >
                  {t('home.editMessage')}
                </button>
              )}
              {onDeleteMessage && (
                <button
                  onClick={() => onDeleteMessage(message)}
                  className={`text-[10px] px-1.5 py-0.5 rounded-md ${
                    confirmDeleteMsgId === message.id
                      ? 'text-[var(--danger)] bg-[var(--danger)]/10'
                      : 'text-[var(--text-muted)] hover:text-[var(--danger)] hover:bg-[var(--bg-hover)]'
                  }`}
                  title={t('home.deleteMessage')}
                >
                  {confirmDeleteMsgId === message.id ? t('home.deleteMessageConfirm') : t('home.deleteMessage')}
                </button>
              )}
              {onBranch && !queued && (
                <button
                  onClick={() => onBranch(message)}
                  className="text-[10px] px-1.5 py-0.5 rounded-md text-[var(--text-muted)] hover:text-[var(--accent)] hover:bg-[var(--bg-hover)] flex items-center gap-0.5"
                  title={t('home.branchRegenerate')}
                >
                  <Icon name="git-branch" size={10} />
                  {t('home.branchRegenerate')}
                </button>
              )}
            </div>
          </div>
          <div className="text-sm break-words leading-relaxed">
            <Markdown>{content}</Markdown>
          </div>
        </div>
        <div className="w-7 h-7 shrink-0 rounded-lg bg-[var(--bg-card)] border border-[var(--border)] flex items-center justify-center text-[11px] font-medium text-[var(--text-secondary)]">
          {t('home.you').charAt(0)}
        </div>
      </div>
    )
  }

  return (
    <div
      data-msg-id={message.id}
      className={`flex gap-2.5 group animate-fade-in-up ${highlighted ? 'msg-highlight' : ''}`}
    >
      <div className="w-7 h-7 rounded-lg bg-gradient-to-br from-[var(--accent)] to-[#8b5cf6] flex items-center justify-center shrink-0 mt-0.5 shadow-md shadow-[var(--accent)]/20">
        <Icon name="spark" size={13} white />
      </div>
      <div className="max-w-[85%] min-w-0 flex-1">
        <div className="flex items-center gap-2 mb-1.5">
          <span className="text-[11px] font-medium text-[var(--text-secondary)]">{t('home.agent')}</span>
          <span className="text-[10px] text-[var(--text-muted)]">{time}</span>
          {model && <span className="text-[10px] px-1.5 py-0.5 rounded-md bg-[var(--bg-hover)] text-[var(--text-muted)]">{model}</span>}
          {/* 本条回复用时（ChatGPT 式“已处理 mm:ss”）：任务耗时持久化到消息，历史对话同样可见 */}
          {message.duration_ms != null && message.duration_ms > 0 && (
            <span
              className="text-[10px] tabular-nums px-1.5 py-0.5 rounded-md bg-[var(--bg-hover)]/60 text-[var(--text-muted)]"
              title={t('home.replyDurationHint')}
            >
              ⏱ {fmtElapsed(message.duration_ms / 1000)}
            </span>
          )}
          {/* 本条回复 token 消耗（入库 tokens_in/tokens_out，悬浮提示） */}
          {(message.tokens_in != null || message.tokens_out != null) && (
            <span
              className="text-[10px] tabular-nums px-1.5 py-0.5 rounded-md bg-[var(--bg-hover)]/60 text-[var(--text-muted)]"
              title={`${t('home.tokenHint')}：${t('home.tokenIn')} ${message.tokens_in ?? 0} / ${t('home.tokenOut')} ${message.tokens_out ?? 0}`}
            >
              ↑{(message.tokens_in ?? 0).toLocaleString()} ↓{(message.tokens_out ?? 0).toLocaleString()}
            </span>
          )}
        </div>
        {previewVersion && (
          <div className="flex items-center gap-2 mb-1.5">
            <span className="text-[10px] px-1.5 py-0.5 rounded-md bg-[var(--warning)]/12 text-[var(--warning)] flex items-center gap-1">
              <Icon name="git-branch" size={10} />
              {t('home.branchPreviewBadge')} · {new Date(previewVersion.created_at * 1000).toLocaleString()}
            </span>
            <button onClick={() => setPreviewVersionId(null)} className="text-[10px] text-[var(--accent)] hover:underline">
              {t('home.branchBackToMain')}
            </button>
          </div>
        )}
        {displayReasoning && <ThinkingBlock content={displayReasoning} />}
        <div className="text-sm break-words leading-relaxed text-[var(--text-primary)]">
          <Markdown onOpenFile={onOpenFile}>{sanitizeToolMarkers(displayContent)}</Markdown>
        </div>
        {modifiedFiles.length > 0 && <ModifiedFilesCard files={modifiedFiles} projectPath={projectPath} />}
        {/* 分支切换面板（对话内预览历史版本，点击版本直接切换显示） */}
        {branchOpen && versions.length > 0 && (
          <div className="mt-2 rounded-xl border border-[var(--accent)]/25 bg-[var(--accent-soft)]/40 p-2 animate-fade-in-up">
            <div className="flex items-center gap-1.5 flex-wrap">
              <button
                onClick={() => setPreviewVersionId(null)}
                className={`px-2.5 py-1 rounded-lg text-[11px] border transition-colors ${
                  previewVersionId === null
                    ? 'border-[var(--accent)] text-[var(--accent)] bg-[var(--accent-soft)]'
                    : 'border-[var(--border)] text-[var(--text-secondary)] hover:border-[var(--text-muted)]'
                }`}
              >
                {t('home.versionCurrent')}
              </button>
              {[...versions].reverse().map((v) => (
                <button
                  key={v.id}
                  onClick={() => setPreviewVersionId(v.id)}
                  className={`px-2.5 py-1 rounded-lg text-[11px] border transition-colors ${
                    previewVersionId === v.id
                      ? 'border-[var(--accent)] text-[var(--accent)] bg-[var(--accent-soft)]'
                      : 'border-[var(--border)] text-[var(--text-secondary)] hover:border-[var(--text-muted)]'
                  }`}
                >
                  {t('home.versionLabel', { n: new Date(v.created_at * 1000).toLocaleString() })}
                </button>
              ))}
              <button
                onClick={() => setBranchOpen(false)}
                className="ml-auto p-1 rounded-lg text-[var(--text-muted)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] transition-colors"
                title={t('home.close')}
              >
                <Icon name="close" size={12} />
              </button>
              <button
                onClick={() => onOpenVersions(message)}
                className="px-2.5 py-1 rounded-lg text-[11px] border border-[var(--border)] text-[var(--text-secondary)] hover:text-[var(--accent)] hover:border-[var(--accent)] transition-colors"
              >
                {t('home.branchCompareDiff')}
              </button>
            </div>
          </div>
        )}
        {/* 操作栏：复制 / 重新生成 / 点赞 / 点踩 / 朗读 / 版本对比 */}
        <div className="flex items-center gap-0.5 mt-1.5 opacity-0 group-hover:opacity-100 transition-opacity">
          <button
            onClick={copyMessage}
            className={`p-1 rounded-md transition-colors ${copied ? 'text-[var(--success)]' : 'text-[var(--text-muted)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)]'}`}
            title={t('home.copyMessage')}
          >
            {copied ? <Icon name="check" size={13} /> : <Icon name="copy" size={13} />}
          </button>
          <button
            onClick={openRemember}
            className="p-1 rounded-md text-[var(--text-muted)] hover:text-[var(--accent)] hover:bg-[var(--bg-hover)] transition-colors"
            title={t('knowledge.rememberThisFix')}
          >
            <Icon name="lightbulb" size={13} />
          </button>
          {isLastAssistant && onRegenerate && (
            <button
              onClick={onRegenerate}
              className="p-1 rounded-md text-[var(--text-muted)] hover:text-[var(--accent)] hover:bg-[var(--bg-hover)] transition-colors"
              title={t('home.regenerate')}
            >
              <Icon name="refresh" size={13} />
            </button>
          )}
          <button
            onClick={() => onRate(message.id, feedback?.feedback === 'like' ? 'neutral' : 'like')}
            className={`p-1 rounded-md transition-colors ${feedback?.feedback === 'like' ? 'text-[var(--accent)]' : 'text-[var(--text-muted)] hover:text-[var(--accent)] hover:bg-[var(--bg-hover)]'}`}
            title={t('home.like')}
          >
            <ThumbUpIcon filled={feedback?.feedback === 'like'} />
          </button>
          <button
            onClick={() =>
              feedback?.feedback === 'dislike'
                ? onRate(message.id, 'neutral')
                : onDislike(message.id)
            }
            className={`p-1 rounded-md transition-colors ${feedback?.feedback === 'dislike' ? 'text-[var(--danger)]' : 'text-[var(--text-muted)] hover:text-[var(--danger)] hover:bg-[var(--bg-hover)]'}`}
            title={t('home.dislike')}
          >
            <ThumbDownIcon filled={feedback?.feedback === 'dislike'} />
          </button>
          <button
            onClick={() => onSpeak(message.id, displayContent)}
            className={`p-1 rounded-md transition-colors ${speaking ? 'text-[var(--accent)]' : 'text-[var(--text-muted)] hover:text-[var(--accent)] hover:bg-[var(--bg-hover)]'}`}
            title={speaking ? t('home.stopSpeak') : t('home.speak')}
          >
            <Icon name="headphones" size={13} />
          </button>
          {versions.length > 0 && (
            <button
              onClick={() => setBranchOpen((v) => !v)}
              className={`p-1 rounded-md transition-colors text-[10px] flex items-center gap-1 ${
                branchOpen
                  ? 'text-[var(--accent)] bg-[var(--accent-soft)]'
                  : 'text-[var(--text-muted)] hover:text-[var(--accent)] hover:bg-[var(--bg-hover)]'
              }`}
              title={t('home.viewVersions')}
            >
              <Icon name="git-branch" size={12} />
              {versions.length}
            </button>
          )}
          {onDeleteMessage && (
            <button
              onClick={() => onDeleteMessage(message)}
              className={`p-1 rounded-md transition-colors ${confirmDeleteMsgId === message.id ? 'text-[var(--danger)] bg-[var(--danger)]/10' : 'text-[var(--text-muted)] hover:text-[var(--danger)] hover:bg-[var(--bg-hover)]'}`}
              title={confirmDeleteMsgId === message.id ? t('home.deleteMessageConfirm') : t('home.deleteMessage')}
            >
              <Icon name="delete" size={13} />
            </button>
          )}
        </div>
      </div>

      {rememberOpen && (
        <div
          className="fixed inset-0 z-50 flex items-center justify-center bg-black/40"
          onClick={() => setRememberOpen(false)}
        >
          <div
            className="w-[480px] max-w-[92vw] rounded-xl bg-[var(--bg-card)] border border-[var(--border)] shadow-2xl p-4 space-y-3"
            onClick={(e) => e.stopPropagation()}
          >
            <h3 className="text-[14px] font-semibold">{t('knowledge.rememberTitle')}</h3>
            <p className="text-[11px] text-[var(--text-muted)]">{t('knowledge.rememberHint')}</p>
            <div>
              <label className="block text-[11px] text-[var(--text-secondary)] mb-1">{t('knowledge.entryTitle')}</label>
              <input
                value={rememberForm.title}
                onChange={(e) => setRememberForm({ ...rememberForm, title: e.target.value })}
                placeholder={t('knowledge.rememberTitlePh')}
                className="w-full h-8 px-2.5 rounded-lg bg-[var(--bg-secondary)] border border-[var(--border)] text-[12px] outline-none focus:border-[var(--accent)]"
              />
            </div>
            <div>
              <label className="block text-[11px] text-[var(--text-secondary)] mb-1">{t('knowledge.rememberError')}</label>
              <textarea
                value={rememberForm.error_text}
                onChange={(e) => setRememberForm({ ...rememberForm, error_text: e.target.value })}
                rows={4}
                placeholder={t('knowledge.rememberErrorPh')}
                className="w-full px-2.5 py-1.5 rounded-lg bg-[var(--bg-secondary)] border border-[var(--border)] text-[12px] outline-none focus:border-[var(--accent)] resize-y"
              />
            </div>
            <div>
              <label className="block text-[11px] text-[var(--text-secondary)] mb-1">{t('knowledge.fix')}</label>
              <textarea
                value={rememberForm.fix}
                onChange={(e) => setRememberForm({ ...rememberForm, fix: e.target.value })}
                rows={5}
                className="w-full px-2.5 py-1.5 rounded-lg bg-[var(--bg-secondary)] border border-[var(--border)] text-[12px] outline-none focus:border-[var(--accent)] resize-y"
              />
            </div>
            <div className="flex justify-end gap-2 pt-1">
              <button
                onClick={() => setRememberOpen(false)}
                className="h-8 px-3 rounded-lg border border-[var(--border)] text-[12px] hover:bg-[var(--bg-hover)]"
              >
                {t('mcp.cancel')}
              </button>
              <button
                onClick={saveRemember}
                disabled={rememberSaving}
                className="h-8 px-4 rounded-lg bg-[var(--accent)] text-white text-[12px] hover:bg-[var(--accent-hover)] disabled:opacity-50"
              >
                {t('knowledge.save')}
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  )
}


/* ============ 设置菜单项 ============ */
const settingsItems: { path: string; labelKey: string; icon: IconName }[] = [
  { path: '/providers', labelKey: 'nav.provider', icon: 'bolt' },
  { path: '/versions', labelKey: 'nav.version', icon: 'package' },
  { path: '/config', labelKey: 'nav.config', icon: 'settings' },
  { path: '/cost', labelKey: 'nav.cost', icon: 'payments' },
  { path: '/proxy', labelKey: 'nav.proxy', icon: 'proxy' },
  { path: '/mcp', labelKey: 'nav.mcp', icon: 'mcp' },
  { path: '/skills', labelKey: 'nav.skill', icon: 'skill' },
  { path: '/health', labelKey: 'nav.health', icon: 'health' },
]
