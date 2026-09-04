// @ui-states: loading, empty, partial, failed, retry
import { lazy, memo, Suspense, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import type { JSX } from 'react'
import { useVirtualizer } from '@tanstack/react-virtual'
import { useNavigate } from 'react-router-dom'
import { useTranslation } from 'react-i18next'
import { useShallow } from 'zustand/react/shallow'
import { listen } from '@tauri-apps/api/event'
import { watch, writeTextFile, readTextFile, readFile, stat } from '@tauri-apps/plugin-fs'
import { getCurrentWebview } from '@tauri-apps/api/webview'
import { save as dialogSave, open as dialogOpen } from '@tauri-apps/plugin-dialog'
import { open as shellOpen } from '@tauri-apps/plugin-shell'
import { useProjectStore, type ToolRun } from '../stores/projectStore'
import { useThemeStore } from '../stores/themeStore'
import { useNotificationStore } from '../stores/notificationStore'
import NotificationBell from '../components/NotificationBell'
import LangToggle from '../components/LangToggle'
import { useAuditStore, type AuditCategory } from '../stores/auditStore'
import { usePinStore, PIN_MAX_PER_CONV } from '../stores/pinStore'
import { useRatingStore } from '../stores/ratingStore'
import { useNoteStore, NOTE_MAX_LEN } from '../stores/noteStore'
import {
  inspectProject,
  listConversations,
  listMessages,
  listConversationTags,
  type TagCount,
  type ProjectInspect,
  type ChatOptions,
  type ChatMessage,
  type MemoryDraft,
  getGlobalRules,
  setGlobalRules,
  updateProjectRules,
  compactConversation,
  getConversationContext,
  getConversationContextV2,
  getSessionHealth,
  setConversationContextPin,
  type ConversationContextInfo,
  type ConversationContextV2,
  type SessionHealthV2,
  searchMessages,
  searchMessagesAllProjects,
  type MessageSearchHit,
  rescanWorkspaceModules,
  setWorkspaceModules,
  parseWorkspaceModules,
  MODULE_KINDS,
  MODULE_KIND_LABELS,
  deriveProjectType,
  type WorkspaceModule,
  type ModuleKind,
  listToolWhitelist,
  removeToolWhitelist,
  type WhitelistEntry,
  resolveDiagnoseCard,
  setConversationModel,
  projectScopedCounts,
  getHarmonyRoot,
  type Project,
  type Conversation,
  type SnapshotInfo,
  listToolGroups,
  queueMessage,
  conversationRoot,
  getConversationBranchParent,
  mergeConversationBranch,
} from '../api/project'
import { toolsHealth } from '../api/health'
import { sendNotification } from '../api/desktop'
import { listProviders, listProviderModels, type ProviderModel } from '../api/provider'
import { generateMedia, type GenKind } from '../api/generation'
import { listPaletteCommands, type PaletteEntry } from '../api/palette'
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
import { getItem, setItem, removeItem, getJSON, setJSON } from '../utils/storage'
import { STORAGE_KEYS } from '../constants'
import {
  ThinkingBlock,
  ThumbUpIcon,
  ThumbDownIcon,
  ModifiedFilesCard,
  ErrorCard,
  EmptyState,
  ChatEmptyState,
} from '../chat/components/messageBlocks'
import { ModelSettingsPopover, PlanCard, TaskOpsBadge } from '../chat/components/plan'
import {
  ConversationRunStatus,
  RunningTaskOpsBadge,
  SilentStreamHint,
  StreamingOutput,
} from '../chat/components/streamingStatus'
import { LedgerCard } from '../chat/components/ledger'
import { ToolRunGroup } from '../chat/components/toolRuns'
import { FeedbackDialog, VersionDiffDialog, MemoryDraftDialog, EditMessageDialog, RulesDialog } from '../chat/components/dialogs'
import { OverviewRow, OverviewGitSummary, MemoriesPanel, ToolStatsPanel, PreviewPanel, TerminalPanel, ShellPanel } from '../chat/components/panels'
import CommandPalette, { type PaletteCommand } from '../components/CommandPalette'
import { Button } from '../components/ui/Button'
import { IconButton } from '../components/ui/IconButton'
import { Field, TextArea } from '../components/ui/Field'
import { Spinner } from '../components/ui/Spinner'
import { useEscapeKey } from '../hooks/useEscapeKey'
import {
  fmtElapsed,
  interruptedTailMessage,
  restoreSelectionRange,
  sanitizeToolMarkers,
  shouldSubmitComposerKey,
} from '../chat/chatUtils'
import { detectGpu, getRecommendedOverscan, shouldUseSmoothScroll } from '../utils/gpuDetect'
import { getLastProjectId } from '../stores/slices/projectSlice'
import {
  externalTextReference,
  imageMimeFromPath,
  isImagePath,
  projectRelativePath,
} from './homeDropUtils'

/** 消息区渲染条目：消息 / 工具组 / 日期分隔线 / 尾部动态区（流式消息、计划卡、工具徽章等，高度动态测量） */
type RenderItem =
  | { kind: 'msg'; key: string; message: ChatMessage; userMessageId: string }
  | { kind: 'tools'; key: string; runs: ToolRun[] }
  | { kind: 'divider'; key: string; label: string }
  | { kind: 'tail'; key: string }

/** 生成媒体菜单项：图片 / 视频 / 音频（对应各厂商生成模型） */
const GEN_ITEMS: { kind: GenKind; icon: IconName }[] = [
  { kind: 'image', icon: 'image' },
  { kind: 'video', icon: 'videocam' },
  { kind: 'audio', icon: 'mic' },
]

// 设备/工程分析/时间线仅在右侧对应 Tab 打开时加载，避免约 100KB 的低频组件源码进入首页首屏执行路径。
const DevicesPanel = lazy(() => import('../chat/components/devicePanels').then((m) => ({ default: m.DevicesPanel })))
const AnalyzePanel = lazy(() => import('../chat/components/devicePanels').then((m) => ({ default: m.AnalyzePanel })))
const SymbolsPanel = lazy(() => import('../chat/components/devicePanels').then((m) => ({ default: m.SymbolsPanel })))
const TimelinePanel = lazy(() => import('../components/TimelinePanel'))

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

/** 草稿持久化（按项目分区，localStorage；空串剔除避免残留） */
const readDraftMap = (pid: string): Record<string, string> =>
  getJSON<Record<string, string>>(STORAGE_KEYS.DRAFTS_PREFIX + pid, {})
const writeDraftMap = (pid: string, m: Record<string, string>) => {
  const cleaned: Record<string, string> = {}
  for (const [k, v] of Object.entries(m)) if (v) cleaned[k] = v
  if (Object.keys(cleaned).length === 0) removeItem(STORAGE_KEYS.DRAFTS_PREFIX + pid)
  else setJSON(STORAGE_KEYS.DRAFTS_PREFIX + pid, cleaned)
}

/** 稳定的短 ID 格式化函数：作为 MessageItem prop 时不因 Home 重渲染改变引用。 */
const shortId = (id: string) => id.slice(0, 8)

// 会话时间分组键（纯函数，模块级保证引用稳定，供 useMemo 直接使用）
const convGroupKey = (ts: number): 'today' | 'yesterday' | 'week' | 'earlier' => {
  const startOfDay = (d: Date) => new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime()
  const diffDays = Math.round((startOfDay(new Date()) - startOfDay(new Date(ts * 1000))) / 86400000)
  if (diffDays <= 0) return 'today'
  if (diffDays === 1) return 'yesterday'
  if (diffDays < 7) return 'week'
  return 'earlier'
}

export default function Home() {
  const { t } = useTranslation()
  const navigate = useNavigate()

  // useShallow：只在选出的字段对象浅比较变化时才触发 Home 重渲染，
  // 否则裸 useProjectStore() 订阅整个 store，任何子切片变更（流式 content/思考增量、
  // 未读数、标签计数等）都会让 Home 整棵子树重新执行，造成流式/切换会话期间卡顿。
  const {
    projects,
    currentProject,
    conversations,
    currentConversation,
    messages,
    streamingConversationId,
    streamingRecoveryParentRunId,
    streamingRecoveryVerificationTotal,
    streamingRecoveryVerificationVerified,
    streamingError,
    streamingErrorDetail,
    toolRuns,
    agentRuns,
    plan,
    toolApprovals,
    pendingConfirmations,
    taskLedgers,
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
    toggleProjectPin,
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
    toolTokenStats,
    saveMemory,
    deleteMemory,
    setMemoryEnabled,
    loadMemories,
    loadToolStats,
    loadToolTokenStats,
    rateMessage,
    summarizing,
    queueUserMessage,
    editMessage,
    removeMessage,
    tokenStats,
    rollbackTask,
    snapshots,
    loadingSnapshots,
    loadSnapshots,
    restoreToSnapshot,
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
    olderHasMore,
    loadingOlder,
    loadOlderMessages,
    forkCurrentConversation,
    genStatus,
  } = useProjectStore(useShallow((s) => ({
    projects: s.projects,
    currentProject: s.currentProject,
    conversations: s.conversations,
    currentConversation: s.currentConversation,
    messages: s.messages,
    streamingConversationId: s.streaming.conversationId,
    streamingRecoveryParentRunId: s.streaming.recoveryParentRunId,
    streamingRecoveryVerificationTotal: s.streaming.recoveryVerificationTotal,
    streamingRecoveryVerificationVerified: s.streaming.recoveryVerificationVerified,
    streamingError: s.streaming.error,
    streamingErrorDetail: s.streaming.errorDetail,
    toolRuns: s.toolRuns,
    agentRuns: s.agentRuns,
    plan: s.plan,
    toolApprovals: s.toolApprovals,
    pendingConfirmations: s.pendingConfirmations,
    taskLedgers: s.taskLedgers,
    resolveToolApproval: s.resolveToolApproval,
    diagnoseCards: s.diagnoseCards,
    dismissDiagnoseCard: s.dismissDiagnoseCard,
    pendingPlan: s.pendingPlan,
    resolvePlanReview: s.resolvePlanReview,
    approvedPlan: s.approvedPlan,
    unfinishedConv: s.unfinishedConv,
    todos: s.todos,
    askCard: s.askCard,
    resolveAskUser: s.resolveAskUser,
    refreshProjects: s.refreshProjects,
    toggleProjectPin: s.toggleProjectPin,
    openProject: s.openProject,
    addProjectByPath: s.addProjectByPath,
    confirmTrust: s.confirmTrust,
    removeProject: s.removeProject,
    newConversation: s.newConversation,
    openConversation: s.openConversation,
    sendUserMessage: s.sendUserMessage,
    stopGeneration: s.stopGeneration,
    stopCurrentTool: s.stopCurrentTool,
    regenerateLast: s.regenerateLast,
    renameConversation: s.renameConversation,
    deleteConversation: s.deleteConversation,
    pinConversation: s.pinConversation,
    archiveConversation: s.archiveConversation,
    gitBranches: s.gitBranches,
    switchBranch: s.switchBranch,
    fileTree: s.fileTree,
    indexBuilding: s.indexBuilding,
    dirCache: s.dirCache,
    loadDirChildren: s.loadDirChildren,
    rebuildIndex: s.rebuildIndex,
    memories: s.memories,
    toolStats: s.toolStats,
    toolTokenStats: s.toolTokenStats,
    saveMemory: s.saveMemory,
    deleteMemory: s.deleteMemory,
    setMemoryEnabled: s.setMemoryEnabled,
    loadMemories: s.loadMemories,
    loadToolStats: s.loadToolStats,
    loadToolTokenStats: s.loadToolTokenStats,
    rateMessage: s.rateMessage,
    summarizing: s.summarizing,
    queueUserMessage: s.queueUserMessage,
    editMessage: s.editMessage,
    removeMessage: s.removeMessage,
    tokenStats: s.tokenStats,
    rollbackTask: s.rollbackTask,
    snapshots: s.snapshots,
    loadingSnapshots: s.loadingSnapshots,
    loadSnapshots: s.loadSnapshots,
    restoreToSnapshot: s.restoreToSnapshot,
    setConversationKeyword: s.setConversationKeyword,
    recentRuns: s.recentRuns,
    loadRecentRuns: s.loadRecentRuns,
    terminalEntries: s.terminalEntries,
    clearTerminal: s.clearTerminal,
    buildLogs: s.buildLogs,
    clearBuildLogs: s.clearBuildLogs,
    lastTaskSummary: s.lastTaskSummary,
    queuedList: s.queuedList,
    refreshQueued: s.refreshQueued,
    removeQueued: s.removeQueued,
    olderHasMore: s.olderHasMore,
    loadingOlder: s.loadingOlder,
    loadOlderMessages: s.loadOlderMessages,
    forkCurrentConversation: s.forkCurrentConversation,
    genStatus: s.genStatus,
  })))
  // 只订阅运行会话 ID 集合。流式正文变化时选择器结果保持相同字符串，Home 不会重渲染。
  const runningConversationKey = useProjectStore((s) =>
    Object.entries(s.streamings)
      .filter(([, bucket]) => !bucket.error)
      .map(([id]) => id)
      .sort()
      .join('\u0000'),
  )
  const runningConversationIds = useMemo(
    () => new Set(runningConversationKey ? runningConversationKey.split('\u0000') : []),
    [runningConversationKey],
  )
  // 会话工作目录：worktree 会话指向 worktree 路径，本地会话为 undefined（后端回退主仓库）
  const convRoot = conversationRoot(currentConversation)
  // 当前会话的任务账本（Ledger 协议）：进行中实时刷新/中断保留/切回恢复；无账本不展示
  const ledgerCard = currentConversation ? taskLedgers[currentConversation.id] : undefined
  const themeResolved = useThemeStore((s) => s.resolved)
  const toggleTheme = useThemeStore((s) => s.toggle)

  const [showAddDialog, setShowAddDialog] = useState(false)
  const [pendingTrust, setPendingTrust] = useState<{ projectId: string; inspect: ProjectInspect } | null>(null)
  const [trustBusy, setTrustBusy] = useState(false)
  const [showRightPanel, setShowRightPanel] = useState(
    () => getItem(STORAGE_KEYS.RIGHT_PANEL) !== 'collapsed',
  )
  const [rightTab, setRightTab] = useState<'overview' | 'files' | 'memories' | 'stats' | 'git' | 'preview' | 'devices' | 'symbols' | 'terminal' | 'shell' | 'analyze' | 'timeline'>('overview')
  const [sidebarCollapsed, setSidebarCollapsed] = useState(
    () => getItem(STORAGE_KEYS.SIDEBAR_COLLAPSED) === '1',
  )
  // 侧栏宽度（可拖拽调宽，记忆上次调整）
  const [sidebarWidth, setSidebarWidth] = useState(() => {
    const v = Number(getItem(STORAGE_KEYS.SIDEBAR_WIDTH))
    return Number.isFinite(v) && v >= 180 && v <= 420 ? v : 256
  })
  const [rightWidth, setRightWidth] = useState(() => {
    const v = Number(getItem(STORAGE_KEYS.RIGHT_WIDTH))
    const max = Math.min(900, Math.floor(window.innerWidth * 0.65))
    return Number.isFinite(v) && v >= 240 && v <= max ? v : 288
  })
  // 右侧栏过窄时 Tab 仅显示图标、隐藏文字，避免文字竖排（阈值：全文字 Tab 需约 420px）
  const rightCompact = rightWidth < 400
  const [draft, setDraft] = useState('')
  // 会话搜索（侧栏搜索框；Ctrl+K 聚焦）
  const [searchText, setSearchText] = useState('')
  const searchInputRef = useRef<HTMLInputElement>(null)
  // 搜索模式：conv=按会话标题/首条消息；msg=当前项目内消息；all=跨项目消息全文
  const [searchMode, setSearchMode] = useState<'conv' | 'msg' | 'all'>('conv')
  /** 消息搜索范围：all=项目全部会话；current=仅当前会话 */
  const [msgSearchScope, setMsgSearchScope] = useState<'all' | 'current'>('all')
  const [msgHits, setMsgHits] = useState<MessageSearchHit[]>([])
  const [msgSearching, setMsgSearching] = useState(false)
  // 跨项目搜索结果（searchMode='all' 时使用，msgHits 复用也行但语义不同：单独 state 更清晰）
  const [allProjectHits, setAllProjectHits] = useState<MessageSearchHit[]>([])
  const [allProjectSearching, setAllProjectSearching] = useState(false)
  // 搜索请求序号：每次防抖触发 +1；响应回来时序号不匹配（期间又改了关键字/模式）则丢弃，
  // 防止慢响应乱序覆盖新结果
  const searchSeqRef = useRef(0)
  // 标签筛选：null = 全部；string = 该 tag
  const [activeTagFilter, setActiveTagFilter] = useState<string | null>(null)
  // 项目下所有出现过的标签 + 频次（用于筛选下拉）
  const [tagCounts, setTagCounts] = useState<TagCount[]>([])
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
  // 外部文件拖拽悬停中（显示"松手添加"覆盖层；Tauri onDragDropEvent 驱动）
  const [dragActive, setDragActive] = useState(false)
  // 生成媒体模式：选中图片/视频/音频后，发送按钮提交生成任务（而非普通对话）
  const [genMode, setGenMode] = useState<GenKind | null>(null)
  const [genMenuOpen, setGenMenuOpen] = useState(false)
  const genMenuRef = useRef<HTMLDivElement>(null)
  // Rules 编辑弹窗（全局指令 + 项目级 rules，均注入 system_prompt）
  const [showRulesDialog, setShowRulesDialog] = useState(false)
  const [rulesTab, setRulesTab] = useState<'global' | 'project'>('global')
  const [rulesGlobalText, setRulesGlobalText] = useState('')
  const [rulesProjectText, setRulesProjectText] = useState('')
  const [rulesSaving, setRulesSaving] = useState(false)
  // 任务回滚（git 硬重置到任务起点前最后一次提交）
  const [rollbackBusy, setRollbackBusy] = useState(false)
  // 时间线刷新信号：任务流式结束时 +1（复盘刚完成的任务，事件已全部落库）
  const [timelineTick, setTimelineTick] = useState(0)
  const prevStreamingRef = useRef(streamingConversationId)
  useEffect(() => {
    if (prevStreamingRef.current && !streamingConversationId) {
      setTimelineTick((n) => n + 1)
    }
    prevStreamingRef.current = streamingConversationId
  }, [streamingConversationId])
  // 任务过程徽章展开态：流式“已处理 N 个操作中”与完成后回看共用
  const [opsOpen, setOpsOpen] = useState(false)
  // 任务清单开合：首个进行中任务出现时自动展开一次；用户手动收起后本轮不再干预；全部完成收起并复位
  const [todoOpen, setTodoOpen] = useState(false)
  const todoAutoRef = useRef(false)
  useEffect(() => {
    const hasActive = todos.some((t) => t.status === 'in_progress')
    const allDone = todos.length > 0 && todos.every((t) => t.status === 'done')
    if (hasActive && !todoAutoRef.current) {
      setTodoOpen(true)
      todoAutoRef.current = true
    } else if (allDone) {
      setTodoOpen(false)
      todoAutoRef.current = false
    }
  }, [todos])
  // 用户点击停止后立即进入本地“停止中”状态，直到对应流式桶真正收敛。
  const [stopRequested, setStopRequested] = useState(false)
  // 右侧栏 Web 预览：待打开地址 + 当前 iframe 地址
  const [previewUrl, setPreviewUrl] = useState(() => getItem(STORAGE_KEYS.PREVIEW_URL) || 'http://localhost:5173')
  const [previewSrc, setPreviewSrc] = useState('')
  const [inputHeight, setInputHeight] = useState(96)
  const [renamingId, setRenamingId] = useState<string | null>(null)
  const [renamingText, setRenamingText] = useState('')
  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null)
  const [confirmDeleteProjectId, setConfirmDeleteProjectId] = useState<string | null>(null)
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
  // 项目标识文件（框架标志文件）变更 → 项目类型自动重新分类的提示（toast，4 秒自动消失）
  const [projectMetaToast, setProjectMetaToast] = useState<{
    project_id: string
    old_kind: string
    new_kind: string
  } | null>(null)
  // 最近项目右键菜单（打开文件夹 / 刷新项目信息）
  const [projectMenu, setProjectMenu] = useState<{ x: number; y: number; project: Project } | null>(null)
  const projectMenuRef = useRef<HTMLDivElement>(null)
  // 右键菜单关闭：点击菜单外 / 任意滚动
  useEffect(() => {
    if (!projectMenu) return
    const onDown = (e: MouseEvent) => {
      if (projectMenuRef.current && !projectMenuRef.current.contains(e.target as Node)) {
        setProjectMenu(null)
      }
    }
    const onScroll = () => setProjectMenu(null)
    document.addEventListener('mousedown', onDown)
    window.addEventListener('scroll', onScroll, true)
    return () => {
      document.removeEventListener('mousedown', onDown)
      window.removeEventListener('scroll', onScroll, true)
    }
  }, [projectMenu])
  // Escape 走全局栈：菜单开在模态之上时，一次 Esc 只关菜单，不把底下那层一起带走
  useEscapeKey(projectMenu ? () => setProjectMenu(null) : null)
  useEffect(() => {
    if (!projectMetaToast) return
    const timer = setTimeout(() => setProjectMetaToast(null), 4000)
    return () => clearTimeout(timer)
  }, [projectMetaToast])
  // 鸿蒙工具链健康状态：ok=齐全 / warn=部分缺失 / bad=关键工具缺失，用于设置菜单红点提示
  const [envHealth, setEnvHealth] = useState<'ok' | 'warn' | 'bad' | null>(null)
  // [75] 工具 → task_group 映射（后端 TOOL_GROUP 同源，统计面板分组折叠 UI）
  const [toolGroupMap, setToolGroupMap] = useState<Record<string, string>>({})
  // [66] 工具链体检横幅：启动 5s 后自动 ping，缺失关键工具链（hvigorw/hdc/ohpm）时展示，点击跳转体检页
  const [toolHealthMissing, setToolHealthMissing] = useState<string[]>([])
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
  // [75] 工具分组映射为静态全量数据（TOOL_GROUP），挂载时拉取一次即可
  useEffect(() => {
    listToolGroups()
      .then((pairs) => setToolGroupMap(Object.fromEntries(pairs)))
      .catch(() => {})
  }, [])
  // [66] 启动 5s 后自动 ping 工具链；缺失关键项时展示顶部横幅（点击跳转体检页）
  useEffect(() => {
    let cancelled = false
    const timer = setTimeout(() => {
      toolsHealth()
        .then((checks) => {
          if (!cancelled) setToolHealthMissing(checks.filter((c) => !c.found).map((c) => c.name))
        })
        .catch(() => {})
    }, 5000)
    return () => {
      cancelled = true
      clearTimeout(timer)
    }
  }, [])
  const [showModelSettings, setShowModelSettings] = useState(false)
  // 工具栏"更多"菜单：sm-md 把 1x / 发给 Agent / 批量任务 三个次要按钮收进浮层
  // （与顶栏 moreMenuOpen 区分——顶栏是会话级导出/回滚）
  const [toolbarMoreOpen, setToolbarMoreOpen] = useState(false)
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
  // 会话时间线弹窗（快照点列表 → 回到历史决策点重新引导）
  const [timelineOpen, setTimelineOpen] = useState(false)
  const [branchParentId, setBranchParentId] = useState<string | null>(null)
  useEffect(() => {
    let active = true
    const conversationId = currentConversation?.id
    if (!conversationId) {
      setBranchParentId(null)
      return () => { active = false }
    }
    void getConversationBranchParent(conversationId)
      .then((parent) => { if (active) setBranchParentId(parent) })
      .catch(() => { if (active) setBranchParentId(null) })
    return () => { active = false }
  }, [currentConversation?.id])
  // 正在朗读的消息 id
  const [speakingId, setSpeakingId] = useState<string | null>(null)
  // 编辑消息弹窗目标（仅 user 消息可编辑）
  const [editTarget, setEditTarget] = useState<ChatMessage | null>(null)
  // 删除消息二次确认（第一次点击进入确认态，3 秒内再点执行）
  const [confirmDeleteMsgId, setConfirmDeleteMsgId] = useState<string | null>(null)
  // 拖拽调宽中的侧栏（拖拽时禁用宽度过渡动画，避免拖尾）
  const [resizing, setResizing] = useState<'sidebar' | 'right' | null>(null)
  const [modelCatalog, setModelCatalog] = useState<{ providerName: string; providerId: string; autoPoolMode: number; isActive: boolean; models: ProviderModel[] }[]>([])
  // 命令面板（Cmd+K）：后端静态命令 + 前端动态命令（会话/模型/斜杠）合并后 fuzzy 搜索
  const [paletteOpen, setPaletteOpen] = useState(false)
  // 快捷键速查浮层（? 触发 / Esc 关闭）
  const [showShortcuts, setShowShortcuts] = useState(false)
  // 项目快速切换器（Ctrl+Shift+P 触发）
  const [projectSwitcherOpen, setProjectSwitcherOpen] = useState(false)
  // 批量任务浮层（每行一条 → 串行入队）
  const [batchOpen, setBatchOpen] = useState(false)
  // 会话导入弹层：纯前端解析 md/json 文件 → 创建新会话
  const [importDialog, setImportDialog] = useState<{ open: boolean; title: string; messages: Array<{ role: string; content: string }> } | null>(null)
  // 审计日志查看页（顶部"审计"按钮触发）
  const [auditOpen, setAuditOpen] = useState(false)
  // 通用确认弹层：用于替代 window.confirm，按危险等级显示不同色调
  const [confirmCfg, setConfirmCfg] = useState<{
    open: boolean
    title: string
    body: string
    tone: 'danger' | 'warn' | 'info'
    requireInput?: string
    confirmLabel?: string
    cancelLabel?: string
    onConfirm: () => void
    onCancel?: () => void
  }>({ open: false, title: '', body: '', tone: 'danger', onConfirm: () => {} })
  /** 触发确认弹层（封装 onConfirm/onCancel + 默认 tone=danger） */
  const askConfirm = (cfg: Omit<typeof confirmCfg, 'open'>) =>
    setConfirmCfg({ ...cfg, open: true })
  // 流式输出速度倍率：0.5x / 1x / 2x / 4x（前端节流倍率，0.5x 让长回复更慢可读，4x 让等待秒过）
  const [streamSpeed, setStreamSpeed] = useState<number>(1)
  const [backendCmds, setBackendCmds] = useState<PaletteEntry[]>([])
  const [modelOptions, setModelOptions] = useState<ChatOptions>(() =>
    getJSON<ChatOptions>(STORAGE_KEYS.CHAT_OPTIONS, {}),
  )
  const [planFeedback, setPlanFeedback] = useState('')
  // 工具审批弹窗：选择记忆范围（空=仅本次；session=本会话免审；project=本项目持久化免审）；拒绝理由反馈给模型
  const [approvalScope, setApprovalScope] = useState<'' | 'session' | 'project'>('')
  const [approvalFeedback, setApprovalFeedback] = useState('')
  // 审批队列首项 ID：切换时重置选择与理由（每个工具独立决策）
  const firstApprovalRequestId = toolApprovals[0]?.requestId
  useEffect(() => {
    setApprovalScope('')
    setApprovalFeedback('')
  }, [firstApprovalRequestId])
  // 工具风险分级展示：L0 只读=绿 / L1 写入=橙 / L2 危险=红
  const approvalRisk = (tool: string, authoritativeLevel?: string): { label: string; cls: string } => {
    if (authoritativeLevel === 'L2') return { label: 'L2 高风险', cls: 'bg-[var(--danger)]/15 text-[var(--danger)]' }
    if (authoritativeLevel === 'L1') return { label: 'L1 写入', cls: 'bg-[var(--warning)]/15 text-[var(--warning)]' }
    if (authoritativeLevel === 'L0') return { label: 'L0 只读', cls: 'bg-[var(--success)]/15 text-[var(--success)]' }
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
  }, [pendingPlan])
  // Agent 提问卡：新问题到来时重置回答输入
  const [askAnswer, setAskAnswer] = useState('')
  useEffect(() => {
    if (askCard) setAskAnswer('')
  }, [askCard])
  // 上下文可视条：消息数 + 摘要状态 + token 预算占用（切换会话/收到新消息后刷新）
  const [ctxInfo, setCtxInfo] = useState<ConversationContextInfo | null>(null)
  const [ctxV2Detail, setCtxV2Detail] = useState<ConversationContextV2 | null>(null)
  const [sessionHealth, setSessionHealth] = useState<SessionHealthV2 | null>(null)
  const [ctxV2Open, setCtxV2Open] = useState(false)
  const [ctxDecisionDraft, setCtxDecisionDraft] = useState('')
  const reconciliationNoticeRef = useRef('')
  // 当前会话 ID：上下文可视条刷新依赖（避免 effect 内直接引用会话对象）
  const convId = currentConversation?.id
  useEffect(() => {
    if (!convId) {
      setCtxInfo(null)
      setCtxV2Detail(null)
      setSessionHealth(null)
      setCtxV2Open(false)
      return
    }
    let cancelled = false
    setCtxV2Open(false)
    getConversationContext(convId)
      .then((info) => !cancelled && setCtxInfo(info))
      .catch(() => {})
    getConversationContextV2(convId)
      .then((context) => !cancelled && setCtxV2Detail(context))
      .catch(() => !cancelled && setCtxV2Detail(null))
    getSessionHealth(convId)
      .then((health) => !cancelled && setSessionHealth(health))
      .catch(() => !cancelled && setSessionHealth(null))
    return () => {
      cancelled = true
    }
  }, [convId, messages.length, modelOptions.model_id])
  const changeContextPin = useCallback(async (
    pinKind: 'message' | 'decision' | 'file' | 'acceptance',
    sourceRef: string,
    label: string,
    content: string,
    pinned: boolean,
  ) => {
    if (!convId) return
    await setConversationContextPin({
      conversation_id: convId,
      pin_kind: pinKind,
      source_ref: sourceRef,
      label,
      content,
      pinned,
    })
    if (pinKind === 'message') {
      const messageId = sourceRef.replace(/^message:/, '')
      if (pinned) usePinStore.getState().pin(convId, messageId)
      else usePinStore.getState().unpin(convId, messageId)
    }
    const context = await getConversationContextV2(convId)
    setCtxV2Detail(context)
  }, [convId])
  // One-time-compatible union: migrate legacy localStorage message pins into
  // Context V2, and mirror durable pins back to the existing pinned-message bar.
  useEffect(() => {
    if (!convId || !ctxV2Detail) return
    const durableIds = new Set(
      ctxV2Detail.pins
        .filter((pin) => pin.pin_kind === 'message' && pin.source_ref.startsWith('message:'))
        .map((pin) => pin.source_ref.slice('message:'.length)),
    )
    for (const id of durableIds) usePinStore.getState().pin(convId, id)
    const localIds = usePinStore.getState().pins[convId] ?? []
    const missing = localIds.filter((id) => !durableIds.has(id))
    if (missing.length === 0) return
    const byId = new Map(messages.map((message) => [message.id, message]))
    void Promise.all(missing.map((id) => {
      const message = byId.get(id)
      if (!message) return Promise.resolve(null)
      return setConversationContextPin({
        conversation_id: convId,
        pin_kind: 'message',
        source_ref: `message:${id}`,
        label: message.role,
        content: message.content,
        pinned: true,
      })
    })).then(() => getConversationContextV2(convId)).then(setCtxV2Detail).catch(() => {})
  }, [convId, ctxV2Detail, messages])
  useEffect(() => {
    if (!convId || ctxV2Detail?.reconciliation.latest_status !== 'corrected') return
    const key = `${convId}:${ctxV2Detail.reconciliation.latest_at ?? 0}`
    if (reconciliationNoticeRef.current === key) return
    reconciliationNoticeRef.current = key
    useNotificationStore.getState().push({
      tone: 'warn',
      title: t('home.ctxConflictTitle'),
      body: t('home.ctxConflictBody', {
        conflicts: ctxV2Detail.reconciliation.latest_conflicts.join(' · '),
      }),
    })
  }, [convId, ctxV2Detail?.reconciliation, t])
  // 会话跟随模型：切换会话时恢复该会话绑定的模型（未绑定的会话保持当前全局选择）
  useEffect(() => {
    if (currentConversation?.model_id) {
      setModelOptions((prev) => {
        if (prev.model_id === currentConversation.model_id) return prev
        const next = { ...prev, model_id: currentConversation.model_id ?? undefined }
        setJSON(STORAGE_KEYS.CHAT_OPTIONS, next)
        return next
      })
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [currentConversation?.id])
  // 项目目录监视：外部工具（IDE/编辑器/其他进程）修改文件时，节流刷新文件树与 Git 面板，
  // 让界面实时感知项目变化（Agent 执行工具产生的修改同样感知；构建产物目录已过滤）
  // 当前项目路径：目录监视与后续依赖使用（避免 effect 内直接引用项目对象）
  const projectPath = currentProject?.path
  useEffect(() => {
    // 监视会话实际工作目录（worktree 会话监视 worktree，本地会话监视项目主路径）
    const watchPath = convRoot ?? projectPath
    if (!watchPath) return
    let cancelled = false
    let unwatch: (() => void) | undefined
    let lastRefresh = 0
    const IGNORE_SEG = ['.git', 'node_modules', 'build', 'oh_modules', '.hvigor', 'target', 'dist', '.idea', '.preview']
    const shouldIgnore = (p: string) => {
      const lower = p.toLowerCase()
      return IGNORE_SEG.some((s) => lower.includes(`/${s}/`) || lower.includes(`\\${s}\\`))
    }
    watch(
      watchPath,
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
  }, [projectPath, convRoot])
  // 排队中消息列表：消息/任务状态变化时刷新（任务结束后排队消息被消费清空）
  const [queuedOpen, setQueuedOpen] = useState(false)
  useEffect(() => {
    if (!currentConversation) return
    refreshQueued(currentConversation.id)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [currentConversation?.id, messages.length, streamingConversationId])
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
  // 工具栏"更多"菜单 ref（与顶栏 moreMenuRef 区分）
  const toolbarMoreRef = useRef<HTMLDivElement>(null)
  const bottomRef = useRef<HTMLDivElement>(null)
  const inputRef = useRef<HTMLTextAreaElement>(null)
  const dragRef = useRef<{ startY: number; startH: number } | null>(null)
  const sidebarDragRef = useRef<{ startX: number; startW: number } | null>(null)
  const rightDragRef = useRef<{ startX: number; startW: number } | null>(null)
  const settingsRef = useRef<HTMLDivElement>(null)

  // —— 草稿按会话隔离 + localStorage 持久化 ——
  // 内存真源 draftsRef[conversationId]；切换会话/项目时：旧草稿立即落盘 → 载入新项目草稿集 → 恢复新会话草稿；
  // skip 标志防止切换 commit 里同步 effect 用"旧会话草稿值"脏写新会话键
  const draftsRef = useRef<Record<string, string>>({})
  const draftCtxRef = useRef<{ conv: string | null; proj: string | null }>({ conv: null, proj: null })
  const skipDraftSyncRef = useRef(false)

  // 会话/项目切换：保存旧草稿（含防抖切断兜底）→ 载入新项目草稿集 → 恢复新会话草稿
  useEffect(() => {
    const cur = currentConversation?.id ?? null
    const pid = currentProject?.id ?? null
    const prev = draftCtxRef.current
    if (prev.conv === cur && prev.proj === pid) return
    // 旧会话草稿写回旧项目 map 并立即落盘（600ms 防抖可能被切换切断）
    if (prev.proj && prev.conv && prev.conv !== cur) {
      draftsRef.current[prev.conv] = draft
      writeDraftMap(prev.proj, draftsRef.current)
    }
    // 跨项目：载入新项目的草稿集（同项目切会话则复用当前 map）
    if (pid && prev.proj !== pid) draftsRef.current = readDraftMap(pid)
    draftCtxRef.current = { conv: cur, proj: pid }
    setDraft(cur ? draftsRef.current[cur] ?? '' : '')
    skipDraftSyncRef.current = true
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [currentConversation?.id, currentProject?.id])

  // 草稿变化：写回内存 map + 防抖落盘（空草稿在 writeDraftMap 内剔除）
  useEffect(() => {
    if (skipDraftSyncRef.current) {
      skipDraftSyncRef.current = false
      return
    }
    const cur = currentConversation?.id
    const pid = currentProject?.id
    if (!cur || !pid) return
    draftsRef.current[cur] = draft
    const h = setTimeout(() => writeDraftMap(pid, draftsRef.current), 600)
    return () => clearTimeout(h)
  }, [draft, currentConversation?.id, currentProject?.id])

  // 输入框自动增高：内容超过当前高度时增高（上限与拖拽一致 360），不自动缩小（尊重手动拖拽调低）
  // 注意：必须先重置为 auto 再读 scrollHeight——否则未溢出时 scrollHeight 等于当前高度，
  // 每敲一个字 target 都 +8px，导致单行输入高度也不断增长（自引用漂移）
  useEffect(() => {
    const el = inputRef.current
    if (!el) return
    const prev = el.style.height
    el.style.height = 'auto'
    const target = Math.min(360, Math.max(64, el.scrollHeight + 8))
    el.style.height = prev
    if (target > inputHeight) setInputHeight(target)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [draft])

  // 渲染分组：连续 tool 消息合并为工具折叠组（历史工具记录一行展示，点击展开全部）；
  // 其余消息保持原序，并附带回复归属的 userMessageId（版本分组键）；日期变化处插入分隔线
  const renderItems = useMemo<RenderItem[]>(() => {
    const items: RenderItem[] = []
    let lastDayKey = ''
    // 回复归属缓存：最近一条 user 消息 id（顺序扫描均摊 O(1)，避免长会话下每条消息向前查找的 O(n²)）
    let lastUserMessageId = ''
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
    messages.forEach((m) => {
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
      // 回复归属：最近一条 user 消息（版本分组键；user 消息自身归属自己）
      if (m.role === 'user') lastUserMessageId = m.id
      items.push({ kind: 'msg', key: m.id, message: m, userMessageId: lastUserMessageId })
    })
    // 旧数据兼容：历史版本中 tool 消息时间戳晚于正文（工具入库在正文之后），
    // 把位于 assistant 正文之后的工具组前移到正文之前（工具先执行后输出的自然顺序）
    const reordered: RenderItem[] = []
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
  // ===== 消息列表虚拟滚动 =====
  // 只挂载可视区域附近的条目（含 overscan），几千条长会话也能流畅滚动与切换；
  // 条目高度动态测量（measureElement + ResizeObserver），按 key 缓存测量结果，
  // 同会话来回切换滚动位置时可精确还原（estimateSize 命中缓存即准）。
  // estimateSize 按条目类型给出差异化初值：用户气泡短、助手回复长、工具组折叠、分隔线矮，
  // 减少首次挂载后因估计偏差过大导致的 totalSize 跳变和多轮 ResizeObserver 重测量
  const sizeCacheRef = useRef(new Map<string, number>())
  // 尾部动态区（流式消息/计划卡/工具徽章/任务摘要等）作为最后一个虚拟条目，高度随内容自动测量；
  // key 携带会话 id：不同会话尾部内容高度差异大，避免高度缓存跨会话污染导致切换后布局跳动
  const virtualItems = useMemo<RenderItem[]>(
    () => [...renderItems, { kind: 'tail', key: `tail-${currentConversation?.id ?? 'none'}` }],
    [renderItems, currentConversation?.id],
  )
  // 渲染性能分级检测（GPU/CPU）：根据硬件能力自动调整虚拟列表参数
  // - high（独显/Apple Silicon）：overscan=6，启用 smooth 滚动
  // - medium（集显/基本硬件加速）：overscan=4，平衡配置
  // - low（软件渲染/远程桌面/虚拟机）：overscan=2，禁用 smooth 滚动，减少动画
  const gpuTier = useMemo(() => detectGpu().tier, [])
  const overscan = useMemo(() => getRecommendedOverscan(gpuTier), [gpuTier])
  const smoothScrollEnabled = useMemo(() => shouldUseSmoothScroll(gpuTier), [gpuTier])
  const estimateItemSize = useCallback((index: number) => {
    const cached = sizeCacheRef.current.get(virtualItems[index]?.key ?? '')
    if (cached != null) return cached
    const item = virtualItems[index]
    if (!item) return 250
    switch (item.kind) {
      case 'msg':
        return item.message.role === 'user' ? 120 : 380
      case 'tools':
        return 60
      case 'divider':
        return 36
      case 'tail':
        return 200
      default:
        return 250
    }
  }, [virtualItems])
  // TanStack Virtual 的 useVirtualizer 返回不可 memoize 的函数，编译器跳过该组件；
  // 与其它告警不同，这是库 API 限制，无法通过依赖数组消除。
  // eslint-disable-next-line react-hooks/incompatible-library
  const virtualizer = useVirtualizer({
    count: virtualItems.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: estimateItemSize,
    measureElement: (el) => {
      const key = el.getAttribute('data-vkey')
      const size = el.getBoundingClientRect().height
      if (key) sizeCacheRef.current.set(key, size)
      return size
    },
    overscan,
    getItemKey: (index) => virtualItems[index]?.key ?? String(index),
  })

  const currentConvId = currentConversation?.id

  // 消息搜索命中定位：分页场景下目标消息可能尚未加载，先按需加载旧页直到包含（或已无更早），
  // 再居中定位（纯 DOM 校正），3 秒后清除高亮；消息不存在时同样到时清除
  useEffect(() => {
    if (!highlightMsgId || !currentConvId) return
    if (messages.some((m) => m.id === highlightMsgId)) {
      // 已加载：等渲染后居中定位，3 秒后清除高亮
      const timer = setTimeout(() => locateMessageCenter(highlightMsgId), 120)
      const clear = setTimeout(() => setHighlightMsgId(null), 3000)
      return () => {
        clearTimeout(timer)
        clearTimeout(clear)
      }
    }
    if (!olderHasMore || loadingOlder) return
    let cancelled = false
    void (async () => {
      let guard = 0
      while (guard++ < 100 && !cancelled) {
        const s = useProjectStore.getState()
        if (s.currentConversation?.id !== currentConvId) return
        if (s.messages.some((m) => m.id === highlightMsgId)) break
        if (!s.olderHasMore || s.loadingOlder) break
        const added = await loadOlderMessages(currentConvId)
        if (!added) break
      }
    })()
    return () => {
      cancelled = true
    }
    // locateMessageCenter 每次渲染重建：加入依赖会导致定位 effect 反复触发，此处按需引用
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [highlightMsgId, currentConvId, messages, olderHasMore, loadingOlder, loadOlderMessages])

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
    // 项目新增/删除等变更：刷新列表；若变更的是当前项目，同步重载其动态数据
    // （对话框顶部信息、右侧栏各 tab 内容随之更新）
    listen<{ project_id?: string }>('projects-changed', (e) => {
      void refreshProjects()
      const pid = (e.payload as { project_id?: string })?.project_id
      if (pid && useProjectStore.getState().currentProject?.id === pid) {
        void useProjectStore.getState().loadFileTree()
        void loadMemories()
        void loadToolStats()
        void loadToolTokenStats()
        void useProjectStore.getState().refreshGitBranches()
        void loadRecentRuns()
      }
    }).then((u) => !cancelled && unlisteners.push(u)).catch(() => {})
    // 项目标识文件（框架标志文件）变更：刷新各处项目信息；类型变化时弹提示——
    // 新增/删除 build-profile.json5、package.json、go.mod 等会改变项目身份，
    // 后端已重新分类并更新 kind，这里同步刷新列表/顶部徽标/概览/右侧栏。
    listen<{ project_id: string; old_kind: string; new_kind: string }>('project-meta-changed', (e) => {
      const payload = e.payload as { project_id: string; old_kind: string; new_kind: string }
      void refreshProjects()
      const pid = payload.project_id
      if (pid && useProjectStore.getState().currentProject?.id === pid) {
        void useProjectStore.getState().loadFileTree()
        void loadMemories()
        void loadToolStats()
        void loadToolTokenStats()
        void useProjectStore.getState().refreshGitBranches()
        void loadRecentRuns()
      }
      if (payload.old_kind && payload.new_kind && payload.old_kind !== payload.new_kind) {
        setProjectMetaToast({ project_id: pid, old_kind: payload.old_kind, new_kind: payload.new_kind })
      }
    }).then((u) => !cancelled && unlisteners.push(u)).catch(() => {})
    return () => {
      cancelled = true
      unlisteners.forEach((u) => u())
    }
    // 事件订阅只应随 newConversation 重建；回调内的 store 函数每次渲染重建引用，
    // 加入 deps 会导致订阅反复拆除重建。
    // eslint-disable-next-line react-hooks/exhaustive-deps
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
      // 压缩会递增健康度 compress_count，同步刷新健康度面板
      getSessionHealth(conv.id)
        .then((health) => !cancelled && setSessionHealth(health))
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

  // 压缩前、超限恢复和恢复验证失败都通过统一警告事件明确提示，
  // 避免后台纠偏看起来像消息或执行状态静默消失。
  useEffect(() => {
    let cancelled = false
    let dispose: (() => void) | undefined
    listen<{ conversation_id: string; kind: string; message: string }>('chat-context-warning', (event) => {
      const conv = useProjectStore.getState().currentConversation
      if (cancelled || !conv || event.payload.conversation_id !== conv.id) return
      useNotificationStore.getState().push({
        tone: event.payload.kind === 'compression_imminent' ? 'info' : 'warn',
        title: t(`home.ctxWarning.${event.payload.kind}`, { defaultValue: t('home.ctxWarning.default') }),
        body: event.payload.message,
      })
    })
      .then((unlisten) => {
        if (!cancelled) dispose = unlisten
      })
      .catch(() => {})
    return () => {
      cancelled = true
      dispose?.()
    }
  }, [t])

  // 图片输入自动切换到视觉模型：后端已在任务内完成切换，这里 toast 告知用户实际使用的模型
  useEffect(() => {
    let cancelled = false
    let dispose: (() => void) | undefined
    listen<{ conversation_id: string; from: string; to: string; reason: string }>('chat-model-switch', (event) => {
      const conv = useProjectStore.getState().currentConversation
      if (cancelled || !conv || event.payload.conversation_id !== conv.id) return
      useNotificationStore.getState().push({
        tone: 'info',
        title: t('home.modelSwitchTitle'),
        body: t('home.modelSwitchBody', { from: event.payload.from, to: event.payload.to }),
      })
    })
      .then((unlisten) => {
        if (!cancelled) dispose = unlisten
      })
      .catch(() => {})
    return () => {
      cancelled = true
      dispose?.()
    }
  }, [t])

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

  // 生成菜单外部点击关闭
  useEffect(() => {
    const handler = (e: MouseEvent) => {
      if (genMenuRef.current && !genMenuRef.current.contains(e.target as Node)) {
        setGenMenuOpen(false)
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
      if (toolbarMoreRef.current && !toolbarMoreRef.current.contains(e.target as Node)) {
        setToolbarMoreOpen(false)
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
            providerId: p.id,
            autoPoolMode: p.auto_pool_mode,
            isActive: p.is_active,
            models: await listProviderModels(p.id).catch(() => [] as ProviderModel[]),
          })),
        )
        setModelCatalog(entries)
      })
      .catch(() => {})
  }, [])

  // 命令面板静态命令（后端注册表：导航 + 动作）
  useEffect(() => {
    listPaletteCommands()
      .then(setBackendCmds)
      .catch(() => {})
  }, [])

  // 自动打开最近项目（优先恢复上次选中的项目；若无记录则打开第一个）
  useEffect(() => {
    if (projects.length === 0 || currentProject) return
    const lastId = getLastProjectId()
    const target = (lastId && projects.find((p) => p.id === lastId)) || projects[0]
    if (target) openProject(target.id).catch(() => {})
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [projects.length])

  // 新消息滚动到底部
  // 智能贴底：流式输出期间，仅当用户已在底部附近时才自动跟随；用户上滑查看历史时不打断
  const scrollRef = useRef<HTMLDivElement>(null)
  const stickToBottomRef = useRef(true)
  /** 会话切换标记：切换后首次内容落地时恢复该会话上次滚动位置（无记录则直跳底部，无动画） */
  const switchPendingRef = useRef(false)
  /** 滚动位置恢复流程进行中：恢复内部会按需加载旧页（触发 messages 变化），此标志抑制 effect 重复介入 */
  const restoringRef = useRef(false)
  // 每个会话的滚动位置记忆（conversationId → 位置）：切换会话后恢复到上次离开的位置，
  // 避免新消息从顶部逐条渲染造成的"从上到下"观感；跨会话持久化到 localStorage。
  // anchorId 为离开时视口顶部的消息 id（分页场景恢复时按需加载旧页后精确定位），null 表示贴底。
  interface ScrollPos { top: number; anchorId: string | null }
  const scrollPosMapRef = useRef<Record<string, ScrollPos | number>>(
    getJSON<Record<string, ScrollPos | number>>(STORAGE_KEYS.SCROLL_POS, {}),
  )
  const persistScrollPosRef = useRef<number | null>(null)
  const [showScrollBottom, setShowScrollBottom] = useState(false)
  const showScrollBottomRef = useRef(false)
  showScrollBottomRef.current = showScrollBottom
  /** 会话切换中标志：messages 已被清空、新会话消息尚未加载完成时抑制空状态闪烁，加载完成后复位 */
  const [switchingConv, setSwitchingConv] = useState(false)
  // 未读数按对话维度统计（conversationId → count）：滚离底部期间该对话新消息到达时累加，回到底部/切换对话清零。
  // 跨会话持久化到 localStorage，应用重启后对话列表仍保留未读标记。
  const [unreadMap, setUnreadMap] = useState<Record<string, number>>(() =>
    getJSON<Record<string, number>>(STORAGE_KEYS.UNREAD_MAP, {}),
  )
  const persistUnreadMap = useRef<number | null>(null)
  useEffect(() => {
    // WKWebView 无 requestIdleCallback：守卫降级为 setTimeout（与符号预热同款策略）
    const w = window as unknown as {
      requestIdleCallback: (cb: () => void, o?: { timeout: number }) => number
      cancelIdleCallback: (id: number) => void
      setTimeout: (cb: () => void, ms?: number) => number
      clearTimeout: (id: number) => void
    }
    const hasRic = 'requestIdleCallback' in window
    if (persistUnreadMap.current) {
      if (hasRic) w.cancelIdleCallback(persistUnreadMap.current)
      else w.clearTimeout(persistUnreadMap.current)
    }
    if (hasRic) {
      persistUnreadMap.current = w.requestIdleCallback(() => {
        setJSON(STORAGE_KEYS.UNREAD_MAP, unreadMap)
      }, { timeout: 2000 })
    } else {
      persistUnreadMap.current = w.setTimeout(() => {
        setJSON(STORAGE_KEYS.UNREAD_MAP, unreadMap)
      }, 500)
    }
  }, [unreadMap])
  const unreadCount = currentConvId ? unreadMap[currentConvId] ?? 0 : 0

  /** idle 节流持久化滚动位置（scroll 高频事件下避免频繁写 localStorage） */
  const persistScrollPos = () => {
    const w = window as unknown as {
      requestIdleCallback: (cb: () => void, o?: { timeout: number }) => number
      cancelIdleCallback: (id: number) => void
      setTimeout: (cb: () => void, ms?: number) => number
      clearTimeout: (id: number) => void
    }
    const hasRic = 'requestIdleCallback' in window
    if (persistScrollPosRef.current) {
      if (hasRic) w.cancelIdleCallback(persistScrollPosRef.current)
      else w.clearTimeout(persistScrollPosRef.current)
    }
    if (hasRic) {
      persistScrollPosRef.current = w.requestIdleCallback(() => {
        setJSON(STORAGE_KEYS.SCROLL_POS, scrollPosMapRef.current)
      }, { timeout: 2000 })
    } else {
      persistScrollPosRef.current = w.setTimeout(() => {
        setJSON(STORAGE_KEYS.SCROLL_POS, scrollPosMapRef.current)
      }, 500)
    }
  }

  const isNearBottom = () => {
    const el = scrollRef.current
    if (!el) return true
    const threshold = 120
    return el.scrollHeight - el.scrollTop - el.clientHeight < threshold
  }

  const scrollToBottom = (smooth = true) => {
    const el = scrollRef.current
    if (!el) return
    // low tier（软件渲染/无 GPU）禁用 smooth 动画，减少主线程合成压力
    const useSmooth = smooth && smoothScrollEnabled
    el.scrollTo({ top: el.scrollHeight, behavior: useSmooth ? 'smooth' : 'auto' })
    stickToBottomRef.current = true
    setShowScrollBottom(false)
    if (currentConvId) {
      // 到底部即记录当前位置（切走再切回时恢复到底部）：anchorId=null 表示贴底
      scrollPosMapRef.current[currentConvId] = { top: el.scrollHeight, anchorId: null }
      setUnreadMap((m) => {
        if (!m[currentConvId]) return m
        const next = { ...m }
        delete next[currentConvId]
        return next
      })
    }
    if (!smooth) {
      // 虚拟列表测量异步（ResizeObserver 帧末回调）：初次渲染 totalSize 是估计值，
      // 多轮 rAF 校正直至真正贴底（测量完成后 scrollHeight 增长，一次设置会停在估计位置）。
      // 帧内检查用户意图：用户主动上滚（stick 失效）或正在恢复历史位置时立即停止追赶。
      // estimateSize 已按消息类型给出差异化初值（user=120/assistant=380/...），
      // 测量收敛更快，最多 6 帧即可校正到位（原 12 帧偏保守）
      let attempts = 0
      const tick = () => {
        const el2 = scrollRef.current
        if (!el2 || attempts >= 6) return
        if (!stickToBottomRef.current || restoringRef.current) return
        attempts++
        const dist = el2.scrollHeight - el2.scrollTop - el2.clientHeight
        if (dist < 4) {
          // 已贴底：记录真实高度（测量完成后的值），避免切走再切回时恢复位置偏上
          if (currentConvId) {
            scrollPosMapRef.current[currentConvId] = { top: el2.scrollHeight, anchorId: null }
            persistScrollPos()
          }
          return
        }
        el2.scrollTo({ top: el2.scrollHeight, behavior: 'auto' })
        requestAnimationFrame(tick)
      }
      requestAnimationFrame(tick)
    } else {
      // smooth 动画目标基于测量前的高度：动画结束后若仍处于贴底状态则校正一次
      setTimeout(() => {
        const el2 = scrollRef.current
        if (el2 && stickToBottomRef.current) {
          el2.scrollTo({ top: el2.scrollHeight, behavior: 'auto' })
          if (currentConvId) {
            scrollPosMapRef.current[currentConvId] = { top: el2.scrollHeight, anchorId: null }
            persistScrollPos()
          }
        }
      }, 350)
    }
  }

  /** CSS 属性选择器转义：消息 key 作为 data-vkey 查询时避免特殊字符注入 */
  const cssEscape = (s: string) => (window.CSS ? CSS.escape(s) : s.replace(/["\\]/g, (c) => `\\${c}`))

  /** 视口顶部第一条内容条目 key（消息/工具组 id；分隔线与尾部动态区跳过） */
  const topAnchorKey = () => {
    const el = scrollRef.current
    if (!el) return null
    const st = el.scrollTop
    for (const vi of virtualizer.getVirtualItems()) {
      if (vi.end < st) continue
      const item = virtualItems[vi.index]
      if (item.kind === 'divider' || item.kind === 'tail') continue
      return item.key
    }
    return null
  }

  /** 指定条目相对滚动容器顶部的偏移（未挂载返回 null） */
  const anchorViewportOffset = (key: string) => {
    const el = scrollRef.current
    if (!el) return null
    const dom = document.querySelector(`[data-vkey="${cssEscape(key)}"]`)
    if (!dom) return null
    return dom.getBoundingClientRect().top - el.getBoundingClientRect().top
  }

  /** 触顶加载更早消息并保持视口不跳动：记录加载前视口顶部锚点条目的位置，
   *  prepend 渲染后按锚点 DOM 实际位移补偿 scrollTop（估算高度偏差由测量值修正） */
  const loadOlderAnchored = async (convId: string) => {
    const el = scrollRef.current
    if (!el) return
    const anchorKey = topAnchorKey()
    const anchorViewTop = anchorKey ? anchorViewportOffset(anchorKey) : null
    const added = await loadOlderMessages(convId)
    if (!added || currentConvId !== convId) return
    if (anchorKey != null && anchorViewTop != null) {
      let attempts = 0
      const tick = () => {
        const el2 = scrollRef.current
        if (!el2 || currentConvId !== convId) return
        // 加载期间用户已滚离触顶区：放弃修正（避免把视口拉回加载前位置）
        if (el2.scrollTop > 400) return
        const dom = document.querySelector(`[data-vkey="${cssEscape(anchorKey)}"]`)
        if (dom) {
          const top = dom.getBoundingClientRect().top - el2.getBoundingClientRect().top
          el2.scrollTo({ top: el2.scrollTop + (anchorViewTop - top), behavior: 'auto' })
          return
        }
        if (attempts++ < 5) requestAnimationFrame(tick)
      }
      requestAnimationFrame(tick)
    }
  }

  /** 按 scrollTop 恢复（虚拟列表初次渲染 totalSize 是估计值，多帧校正直至位置可达或贴底兜底） */
  const restoreTop = (saved: number) => {
    const el = scrollRef.current
    if (!el) return
    el.scrollTo({ top: saved, behavior: 'auto' })
    let attempts = 0
    const correct = () => {
      const el2 = scrollRef.current
      if (!el2) return
      const maxTop2 = el2.scrollHeight - el2.clientHeight
      if (saved <= maxTop2) {
        el2.scrollTo({ top: saved, behavior: 'auto' })
        const near = el2.scrollHeight - el2.scrollTop - el2.clientHeight < 120
        stickToBottomRef.current = near
        setShowScrollBottom(!near)
        // estimateSize 更精准后 4 帧足够校正到位
        if (attempts < 4) {
          attempts++
          requestAnimationFrame(correct)
        }
      } else if (attempts < 4) {
        // 估计高度 < 真实高度时 saved 暂超界：等待后续帧测量完成再判
        attempts++
        requestAnimationFrame(correct)
      } else {
        // 测量完成仍超界：内容确实变短，贴底即可，并同步位置记录
        stickToBottomRef.current = true
        setShowScrollBottom(false)
        el2.scrollTo({ top: el2.scrollHeight, behavior: 'auto' })
        if (currentConvId) {
          scrollPosMapRef.current[currentConvId] = { top: el2.scrollTop, anchorId: null }
          persistScrollPos()
        }
      }
    }
    requestAnimationFrame(correct)
  }

  /** 定位到指定消息（顶部对齐视口）：纯 DOM 校正，不依赖虚拟列表测量状态 */
  const locateAnchor = (anchorId: string) => {
    stickToBottomRef.current = false
    setShowScrollBottom(true)
    let attempts = 0
    const tick = () => {
      const el2 = scrollRef.current
      if (!el2) return
      const dom = document.querySelector(`[data-vkey="${cssEscape(anchorId)}"]`)
      if (dom) {
        el2.scrollTop = dom.getBoundingClientRect().top - el2.getBoundingClientRect().top
        return
      }
      if (attempts++ < 20) requestAnimationFrame(tick)
    }
    requestAnimationFrame(tick)
  }

  /** 定位到指定消息（视口居中）：搜索命中高亮用，纯 DOM 校正 */
  const locateMessageCenter = (msgId: string) => {
    let attempts = 0
    const tick = () => {
      const el2 = scrollRef.current
      if (!el2) return
      const dom = document.querySelector(`[data-vkey="${cssEscape(msgId)}"]`)
      if (dom) {
        const rel = dom.getBoundingClientRect().top - el2.getBoundingClientRect().top
        el2.scrollTop = rel - (el2.clientHeight - dom.getBoundingClientRect().height) / 2
        return
      }
      if (attempts++ < 20) requestAnimationFrame(tick)
    }
    requestAnimationFrame(tick)
  }

  /** 恢复会话上次滚动位置：锚点消息未加载时先按需加载旧页直到包含，再定位到视口顶部；
   *  贴底记录（anchorId=null）直跳底部；旧版纯数字记录按 scrollTop 恢复（超界兜底贴底） */
  const restoreScrollPosition = async (convId?: string) => {
    if (!convId || restoringRef.current) return
    restoringRef.current = true
    try {
      const el = scrollRef.current
      if (!el) return
      const raw = scrollPosMapRef.current[convId]
      if (typeof raw === 'number') {
        // 旧版数据兼容：纯 scrollTop
        restoreTop(raw)
        return
      }
      if (!raw) {
        scrollToBottom(false)
        return
      }
      if (!raw.anchorId) {
        // 贴底记录：直跳底部（scrollToBottom 已内置多帧贴底校正）
        scrollToBottom(false)
        return
      }
      // 锚点消息可能未加载（上次浏览位置在更早历史页）：循环加载旧页直到包含或已无更早
      let guard = 0
      while (guard++ < 100) {
        const s = useProjectStore.getState()
        if (s.currentConversation?.id !== convId) return
        if (s.messages.some((m) => m.id === raw.anchorId)) break
        if (!s.olderHasMore || s.loadingOlder) break
        const added = await loadOlderMessages(convId)
        if (!added) break
      }
      const s = useProjectStore.getState()
      if (s.currentConversation?.id !== convId) return
      if (s.messages.some((m) => m.id === raw.anchorId)) {
        locateAnchor(raw.anchorId)
      } else {
        // 锚点消息已被删除：回退 scrollTop 恢复（超界时兜底贴底）
        restoreTop(raw.top)
      }
    } finally {
      restoringRef.current = false
    }
  }

  const handleScroll = () => {
    const el = scrollRef.current
    if (!el) return
    const near = isNearBottom()
    stickToBottomRef.current = near
    // showScrollBottom 仅在底部可见性切换时更新，避免每次滚动都触发全量重渲染
    if (near !== showScrollBottomRef.current) {
      showScrollBottomRef.current = near
      setShowScrollBottom(!near)
    }
    // 记录当前会话滚动位置（切走后再切回时恢复）：记录视口顶部消息 id 作为锚点，
    // 分页场景下恢复时按锚点加载旧页后精确定位
    if (currentConvId) {
      scrollPosMapRef.current[currentConvId] = { top: el.scrollTop, anchorId: topAnchorKey() }
      persistScrollPos()
    }
    // 触顶自动加载更早历史（微信式上翻翻页）：距顶部一屏内且仍有更早消息时触发，
    // 加载完成 prepend 后由 loadOlderAnchored 做视口锚定补偿，不会跳动
    if (currentConvId && olderHasMore && !loadingOlder && el.scrollTop < 200) {
      void loadOlderAnchored(currentConvId)
    }
  }

  // 切换会话消息落地：ChatGPT/Minmax 风格——首屏直接停在底部，无可见滚动过程。
  // useLayoutEffect 在 DOM 更新后、绘制前同步把 scrollTop 设为 scrollHeight，
  // 用户第一眼看到的就是最新（底部）内容，没有任何「从上滚到下」的中间帧。
  // 后续帧的测量校正（虚拟列表从 estimateSize → 真实高度）统一由下方 useEffect 调用
  // scrollToBottom(false)/restoreScrollPosition 负责，不在此启动 rAF 循环——
  // 否则会与 scrollToBottom 自身的多帧校正循环（最多 12 帧）竞争，造成双倍布局抖动。
  // 依赖 messages（数组引用）而非 messages.length：openConversation 不立即清空 messages，
  // 新旧会话消息数可能相同，仅靠 length 无法检测到切换后的消息替换。
  useLayoutEffect(() => {
    if (!switchPendingRef.current) return
    if (messages.length === 0) return
    const el = scrollRef.current
    if (!el) return
    el.scrollTop = el.scrollHeight
  }, [messages])

  useEffect(() => {
    // 会话切换完成（messages 引用变化）：恢复滚动位置或贴底
    if (switchPendingRef.current) {
      switchPendingRef.current = false
      setSwitchingConv(false)
      // 空会话：无需恢复滚动位置，switchingConv 已清除即可显示空状态
      if (messages.length === 0) return
      // 异步恢复：内部可能按需加载旧页（触发本 effect 重入），restoringRef 抑制重复介入
      void restoreScrollPosition(currentConvId)
      return
    }
    // 非切换场景的 messages 变化（新消息入库 / loadOlder prepend）
    if (messages.length === 0) return
    setSwitchingConv(false)
    if (restoringRef.current) return
    if (stickToBottomRef.current) {
      scrollToBottom(true)
    } else if (currentConvId) {
      setUnreadMap((m) => ({ ...m, [currentConvId]: (m[currentConvId] ?? 0) + 1 }))
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [messages, currentConvId])

  // 流式期间贴底状态下持续跟随（rAF 节流）。正文增量已下沉到 StreamingOutput，
  // 此处只依赖任务开始/结束，避免每批 token 让 Home 重渲染。
  // streamingActive 保护不能移除：切会话时虚拟列表 totalSize 变化也会触发本 effect，
  // 若无条件跟随会把刚恢复的滚动位置（switchPending 分支）再次拉到底部
  const streamingActive = streamingConversationId === currentConversation?.id
  const streamScrollRafRef = useRef<number | null>(null)
  const streamScrollActiveRef = useRef(false)
  useEffect(() => {
    if (stickToBottomRef.current && streamingActive) {
      if (streamScrollRafRef.current == null) {
        streamScrollActiveRef.current = true
        const loop = () => {
          streamScrollRafRef.current = null
          if (!streamScrollActiveRef.current || !stickToBottomRef.current) return
          const el = scrollRef.current
          if (el) el.scrollTo({ top: el.scrollHeight, behavior: 'auto' })
          streamScrollRafRef.current = requestAnimationFrame(loop)
        }
        streamScrollRafRef.current = requestAnimationFrame(loop)
      }
    } else if (!streamingActive) {
      streamScrollActiveRef.current = false
      if (streamScrollRafRef.current != null) {
        cancelAnimationFrame(streamScrollRafRef.current)
        streamScrollRafRef.current = null
      }
    }
  }, [streamingActive])

  // 组件卸载时清理 rAF 与滚动位置持久化
  useEffect(() => {
    return () => {
      if (persistScrollPosRef.current != null) {
        // WKWebView 无 cancelIdleCallback：与 persistScrollPos 分支一致地降级（#185 同批真机修复）
        if ('cancelIdleCallback' in window) cancelIdleCallback(persistScrollPosRef.current)
        else clearTimeout(persistScrollPosRef.current)
      }
      streamScrollActiveRef.current = false
      if (streamScrollRafRef.current != null) cancelAnimationFrame(streamScrollRafRef.current)
    }
  }, [])

  // 切换对话时：重置贴底状态、标记待恢复并清除该对话的未读数，
  // 新会话消息加载落地后（messages.length 变化）由上方 effect 恢复滚动位置或直跳底部
  useEffect(() => {
    stickToBottomRef.current = true
    setShowScrollBottom(false)
    switchPendingRef.current = true
    // 切换中标志：抑制 messages 清空瞬间到新会话消息落地的空状态闪烁；
    // 新会话无消息（无 length 变化信号）时由下方超时兜底复位
    setSwitchingConv(true)
    const timer = setTimeout(() => setSwitchingConv(false), 1500)
    if (currentConvId) {
      setUnreadMap((m) => {
        if (!m[currentConvId]) return m
        const next = { ...m }
        delete next[currentConvId]
        return next
      })
    }
    return () => clearTimeout(timer)
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
      // 写 audit：信任项目 = 授予文件读写权限
      useAuditStore.getState().log({
        category: 'project.trust',
        label: t('home.auditLabelProjectTrust'),
        detail: pendingTrust.inspect.path,
        projectId: pendingTrust.projectId,
      })
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
  const handleReference = useCallback((path: string) => {
    setDraft((d) => (d ? `${d} @${path} ` : `@${path} `))
    setReferences((r) => (r.includes(path) ? r : [...r, path]))
    inputRef.current?.focus()
  }, [])

  /** 当前会话是否正在流式生成（派生值：供下方处理函数与 effect 使用，声明需在使用前） */
  const isStreaming = streamingConversationId === currentConversation?.id
  /** 当前会话生成媒体任务状态（gen-* 事件驱动：生成中禁用发送、输入区展示状态条） */
  const currentGen = currentConversation ? genStatus[currentConversation.id] : undefined
  useEffect(() => {
    if (!isStreaming) setStopRequested(false)
  }, [isStreaming])
  // 预计算最后一条助手消息 ID，供虚拟列表复用，避免每条消息渲染时都访问数组
  const lastAssistantId = useMemo(() => {
    if (isStreaming) return null
    for (let i = messages.length - 1; i >= 0; i--) {
      if (messages[i]?.role === 'assistant') return messages[i].id
    }
    return null
  }, [messages, isStreaming])

  // 中断回复检测：最后一条消息是已提交（queued=0）的 user 消息且其后无任何回复
  // （任务因应用退出/崩溃中断，assistant 内容从未入库——用户消息已在库，回复丢失）
  // 排队中（queued=1）或流式中不算；命中后尾部渲染“继续生成回复”横幅，一键重新生成
  const orphanUserMessage = useMemo(() => {
    return interruptedTailMessage(messages, isStreaming)
  }, [messages, isStreaming])

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

  /** 失败工具一键重试：注入指令让 Agent 重新执行该工具（失败输出头尾截断后附给模型参考）
   * useCallback：ToolRunGroup 按 memo 浅比较 props，回调稳定才能阻止流式/工具输出更新时历史工具组全量重渲染 */
  const retryTool = useCallback(
    (run: ToolRun) => {
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
    },
    [isStreaming, modelOptions, queueUserMessage, sendUserMessage],
  )

  /** 取消正在运行的工具（稳定引用，避免 ToolRunGroup 因 onCancel 每次都是新函数而全量重渲染） */
  const cancelToolRun = useCallback(() => {
    stopCurrentTool()
  }, [stopCurrentTool])

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
    const busy = !!streamingConversationId
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
  const openCodeFile = useCallback((rawPath: string) => {
    const lineMatch = rawPath.match(/:(\d+)$/)
    const line = lineMatch ? parseInt(lineMatch[1], 10) : undefined
    const path = rawPath.replace(/:\d+$/, '')
    handleReference(path)
    setRightTab('files')
    window.dispatchEvent(
      new CustomEvent('deveco:open-file', { detail: { path, line } }),
    )
  }, [handleReference])

  /** 收集引用列表：显式 @ 选择 + 输入框内残留的 @path 文本（容错手动输入） */
  const collectRefs = (): string[] => {
    const fromText = Array.from(draft.matchAll(/@([^\s@]+)/g), (m) => m[1])
    return Array.from(new Set([...references, ...fromText]))
  }

  const appendExternalText = useCallback((name: string, text: string) => {
    const block = externalTextReference(
      name,
      text,
      t('home.externalFileLabel'),
      t('home.externalDataOnly'),
    )
    setDraft((d) => (d ? `${d}\n\n${block}` : block))
    inputRef.current?.focus()
  }, [t])

  /** 图片文件 → data URL（粘贴/浏览器拖入共用；单图超 8MB，最多 4 张） */
  const addImageFiles = useCallback((files: FileList | File[]) => {
    let oversized = 0
    for (const f of Array.from(files)) {
      if (!f.type.startsWith('image/')) continue
      if (f.size > 8 * 1024 * 1024) {
        oversized += 1
        continue
      }
      const reader = new FileReader()
      reader.onload = () => {
        void compressImage(String(reader.result)).then((url) => {
          setPickedImages((cur) => (cur.length >= 4 ? cur : [...cur, url]))
        })
      }
      reader.onerror = () => {
        useNotificationStore.getState().push({ tone: 'warn', title: t('home.dropReadFail') })
      }
      reader.readAsDataURL(f)
    }
    if (oversized > 0) {
      useNotificationStore.getState().push({ tone: 'warn', title: t('home.dropImageTooBig', { count: oversized }) })
    }
  }, [t])

  const removePickedImage = (idx: number) => {
    setPickedImages((cur) => cur.filter((_, i) => i !== idx))
  }

  /* ============ 外部文件拖拽（Tauri 环境拿真实路径；浏览器环境回退 DOM File） ============ */
  /** 本地图片文件 → data URL 预览（拖拽场景；大小已由调用方校验，发送前统一压缩） */
  const readImageFile = useCallback(async (path: string): Promise<boolean> => {
    try {
      const bytes = await readFile(path)
      if (bytes.byteLength === 0) return false
      const mime = imageMimeFromPath(path)
      if (!mime) return false
      let binary = ''
      const chunk = 0x8000
      for (let i = 0; i < bytes.length; i += chunk) {
        binary += String.fromCharCode(...bytes.subarray(i, i + chunk))
      }
      const url = await compressImage(`data:${mime};base64,${btoa(binary)}`)
      setPickedImages((cur) => (cur.length >= 4 ? cur : [...cur, url]))
      return true
    } catch {
      return false
    }
  }, [])
  /** 项目外文本文件：读入输入框作为引用块（≤200KB；二进制/乱码跳过并提示） */
  const readExternalText = useCallback(async (path: string): Promise<'added' | 'binary' | 'failed'> => {
    try {
      const bytes = await readFile(path)
      const text = new TextDecoder('utf-8').decode(bytes)
      if (!text.trim() || text.includes('\uFFFD')) {
        return 'binary'
      }
      const name = path.split(/[\\/]/).pop() || path
      appendExternalText(name, text)
      return 'added'
    } catch {
      return 'failed'
    }
  }, [appendExternalText])
  /** 拖拽路径分发：图片 → 预览；项目内文件 → @引用（后端注入内容）；项目外文本 → 插入输入框 */
  const handleDroppedPaths = useCallback(async (paths: string[]) => {
    const root = useProjectStore.getState().currentProject?.path
    const skipped = { directories: 0, imagesTooBig: 0, textTooBig: 0, binary: 0, failed: 0 }
    for (const p of paths) {
      try {
        const info = await stat(p)
        if (info.isDirectory) {
          skipped.directories += 1
          continue
        }
        if (isImagePath(p)) {
          if (info.size > 8 * 1024 * 1024) {
            skipped.imagesTooBig += 1
            continue
          }
          if (!(await readImageFile(p))) skipped.failed += 1
          continue
        }
        const relative = root ? projectRelativePath(p, root) : null
        if (relative) {
          handleReference(relative)
          continue
        }
        if (info.size > 200 * 1024) {
          skipped.textTooBig += 1
          continue
        }
        const result = await readExternalText(p)
        if (result === 'binary') skipped.binary += 1
        else if (result === 'failed') skipped.failed += 1
      } catch {
        skipped.failed += 1
      }
    }
    if (skipped.directories) useNotificationStore.getState().push({ tone: 'info', title: t('home.dropDirCount', { count: skipped.directories }) })
    if (skipped.imagesTooBig) useNotificationStore.getState().push({ tone: 'warn', title: t('home.dropImageTooBig', { count: skipped.imagesTooBig }) })
    if (skipped.textTooBig) useNotificationStore.getState().push({ tone: 'warn', title: t('home.dropTooBigCount', { count: skipped.textTooBig }) })
    if (skipped.binary) useNotificationStore.getState().push({ tone: 'warn', title: t('home.dropBinaryCount', { count: skipped.binary }) })
    if (skipped.failed) useNotificationStore.getState().push({ tone: 'warn', title: t('home.dropReadFailCount', { count: skipped.failed }) })
  }, [handleReference, readExternalText, readImageFile, t])

  /** 浏览器开发模式拿不到真实路径：图片仍可预览，文本按外部引用直接读取。 */
  const handleDroppedBrowserFiles = useCallback(async (files: FileList) => {
    const textFiles: File[] = []
    const imageFiles: File[] = []
    let tooBig = 0
    let binary = 0
    let failed = 0
    for (const file of Array.from(files)) {
      if (file.type.startsWith('image/')) imageFiles.push(file)
      else textFiles.push(file)
    }
    addImageFiles(imageFiles)
    for (const file of textFiles) {
      if (file.size > 200 * 1024) {
        tooBig += 1
        continue
      }
      try {
        const text = await file.text()
        if (!text.trim() || text.includes('\uFFFD')) binary += 1
        else appendExternalText(file.name, text)
      } catch {
        failed += 1
      }
    }
    if (tooBig) useNotificationStore.getState().push({ tone: 'warn', title: t('home.dropTooBigCount', { count: tooBig }) })
    if (binary) useNotificationStore.getState().push({ tone: 'warn', title: t('home.dropBinaryCount', { count: binary }) })
    if (failed) useNotificationStore.getState().push({ tone: 'warn', title: t('home.dropReadFailCount', { count: failed }) })
  }, [addImageFiles, appendExternalText, t])
  // Tauri 拖拽事件：真实路径 + 悬停高亮（DOM dataTransfer 拿不到路径，须走 webview 原生事件）
  useEffect(() => {
    let unlisten: (() => void) | undefined
    let cancelled = false
    if ('__TAURI_INTERNALS__' in window) {
      getCurrentWebview()
        .onDragDropEvent((e) => {
          if (e.payload.type === 'drop') {
            setDragActive(false)
            void handleDroppedPaths(e.payload.paths)
          } else if (e.payload.type === 'enter' || e.payload.type === 'over') {
            setDragActive(true)
          } else {
            setDragActive(false)
          }
        })
        .then((u) => {
          if (cancelled) u()
          else unlisten = u
        })
        .catch(() => {})
    }
    return () => {
      cancelled = true
      unlisten?.()
    }
  }, [handleDroppedPaths])

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
        // 会话引用候选：同项目会话（排除当前会话），标题模糊匹配，追加在文件候选之后
        const convCands = conversations
          .filter((c) => c.id !== currentConversation?.id && (q === '' || c.title.toLowerCase().includes(q)))
          .slice(0, 5)
          .map((c) => ({ path: `conv:${c.id}`, name: c.title }))
        setRefCandidates([...list, ...convCands])
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

  /** 选中候选：替换 @query 为 @path，加入引用列表。会话引用（conv:）同时把
   *  标题+最近内容插入草稿（模型可见），与消息引用（Quote）同构 */
  const pickReference = (path: string) => {
    const atIdx = draft.lastIndexOf('@')
    if (atIdx < 0) return
    setDraft(draft.slice(0, atIdx) + `@${path} `)
    if (path.startsWith('conv:')) {
      const convId = path.slice(5)
      const conv = conversations.find((c) => c.id === convId)
      const title = conv?.title ?? convId
      // 异步取该会话最近内容注入草稿（失败不阻塞引用本身）
      void (async () => {
        try {
          const msgs = await listMessages(convId)
          const last = [...msgs].reverse().find((m) => m.role === 'assistant' || m.role === 'user')
          const snippet = (last?.content ?? '').trim().slice(0, 400)
          setDraft((d) => `${d}\n\n【引用会话 ${title}】\n${snippet || '（该会话暂无内容）'}\n\n`)
        } catch {
          setDraft((d) => `${d}\n\n【引用会话 ${title}】（摘要获取失败）\n\n`)
        }
      })()
    } else {
      recordMruRef(path)
    }
    setReferences((r) => (r.includes(path) ? r : [...r, path]))
    setRefCandidates(null)
    inputRef.current?.focus()
  }

  const handleSend = async () => {
    const text = draft.trim()
    if (!text || stopRequested || currentGen) return
    // 生成媒体模式：发送提交生成任务（图片/视频/音频，走对应生成模型）
    if (genMode) {
      await handleGenerateSend(text)
      return
    }
    if (!currentProject) return
    if (!currentConversation) {
      await newConversation()
    }
    const targetConversation = useProjectStore.getState().currentConversation
    if (!targetConversation) return
    const projectId = currentProject.id
    const selectedRefs = references
    const quote = pendingQuote
    const refs = collectRefs()
    if (quote) refs.push(`msg:${quote.id}`)
    const imgs = pickedImages
    // 先清空输入框：invoke 要等整个 Agent 任务结束后才 resolve，若 await 发送则任务期间输入框不会清空
    setDraft('')
    setReferences([])
    setPickedImages([])
    setRefCandidates(null)
    setSlashCandidates(null)
    setPendingQuote(null)
    if (isStreaming) {
      queueUserMessage(text, false, refs.length ? refs : undefined, imgs.length ? imgs : undefined).catch((e) => {
        // 排队落库失败时不能吞掉用户输入；仍停留在原会话则完整恢复编辑器，否则保存到原会话草稿。
        if (useProjectStore.getState().currentConversation?.id === targetConversation.id) {
          setDraft((cur) => (cur.trim() ? `${text}\n\n${cur}` : text))
          setReferences((cur) => Array.from(new Set([...selectedRefs, ...cur])))
          setPickedImages((cur) => [...imgs, ...cur].slice(0, 4))
          if (quote) setPendingQuote(quote)
        } else {
          const targetDrafts = draftCtxRef.current.proj === projectId ? draftsRef.current : readDraftMap(projectId)
          const existing = targetDrafts[targetConversation.id] ?? ''
          targetDrafts[targetConversation.id] = existing.trim() ? `${text}\n\n${existing}` : text
          writeDraftMap(projectId, targetDrafts)
        }
        useNotificationStore.getState().push({
          tone: 'error',
          title: t('chat.queueFailed', '排队消息发送失败'),
          body: String(e ?? ''),
        })
      })
    } else {
      void sendUserMessage(text, modelOptions, refs.length ? refs : undefined, imgs.length ? imgs : undefined)
    }
  }

  /** 生成媒体发送：genMode 激活时提交生成任务（异步，视频可达 15 分钟）。
   * 完成后后端入库 assistant 消息（媒体标记 content）并推送 gen-done；
   * 失败时恢复输入内容与生成模式，便于修改后重试。 */
  const handleGenerateSend = async (text: string) => {
    if (!genMode || stopRequested || currentGen) return
    if (!currentProject) return
    if (!currentConversation) {
      await newConversation()
    }
    const targetConversation = useProjectStore.getState().currentConversation
    if (!targetConversation) return
    const mode = genMode
    const imgs = pickedImages
    // 先清空输入区：invoke 要等整个生成任务结束后才 resolve
    setDraft('')
    setPickedImages([])
    setGenMode(null)
    setGenMenuOpen(false)
    try {
      await generateMedia(targetConversation.id, mode, text, undefined, imgs.length ? imgs : undefined)
    } catch (e) {
      useNotificationStore.getState().push({
        tone: 'error',
        title: t('home.genFailed', '生成失败'),
        body: String(e ?? ''),
      })
      setDraft((cur) => (cur.trim() ? `${text}\n\n${cur}` : text))
      setPickedImages((cur) => [...imgs, ...cur].slice(0, 4))
      setGenMode(mode)
    }
  }

  /** 发送到 Agent：流式运行时提交为挂起消息，由 Agent 在任务内安全点并入当前任务 */
  const handleSendToAgent = async () => {
    const text = draft.trim()
    if (!text || !currentProject || !currentConversation || !isStreaming || stopRequested) return
    const conversationId = currentConversation.id
    const projectId = currentProject.id
    const selectedRefs = references
    const quote = pendingQuote
    const refs = collectRefs()
    if (quote) refs.push(`msg:${quote.id}`)
    const imgs = pickedImages
    // 同 handleSend：先清空输入框，不 await 排队接口
    setDraft('')
    setReferences([])
    setPickedImages([])
    setRefCandidates(null)
    setPendingQuote(null)
    queueUserMessage(text, true, refs.length ? refs : undefined, imgs.length ? imgs : undefined).catch((e) => {
      if (useProjectStore.getState().currentConversation?.id === conversationId) {
        setDraft((cur) => (cur.trim() ? `${text}\n\n${cur}` : text))
        setReferences((cur) => Array.from(new Set([...selectedRefs, ...cur])))
        setPickedImages((cur) => [...imgs, ...cur].slice(0, 4))
        if (quote) setPendingQuote(quote)
      } else {
        const targetDrafts = draftCtxRef.current.proj === projectId ? draftsRef.current : readDraftMap(projectId)
        const existing = targetDrafts[conversationId] ?? ''
        targetDrafts[conversationId] = existing.trim() ? `${text}\n\n${existing}` : text
        writeDraftMap(projectId, targetDrafts)
      }
      useNotificationStore.getState().push({
        tone: 'error',
        title: t('chat.queueFailed', '排队消息发送失败'),
        body: String(e ?? ''),
      })
    })
  }

  /** 删除消息（二次确认：第一次点击进入确认态，3 秒内再点执行；级联删除其后所有消息） */
  const handleDeleteMessage = useCallback(async (msg: ChatMessage) => {
    if (confirmDeleteMsgId !== msg.id) {
      setConfirmDeleteMsgId(msg.id)
      setTimeout(() => setConfirmDeleteMsgId((cur) => (cur === msg.id ? null : cur)), 3000)
      return
    }
    setConfirmDeleteMsgId(null)
    const target = msg // 捕获到 const，audit 里使用不依赖 useCallback deps
    await removeMessage(target.id)
    // 写 audit
    useAuditStore.getState().log({
      category: 'message.delete',
      label: t('home.auditLabelMessageDelete'),
      detail: `${target.role}: ${target.content.slice(0, 60)}${target.content.length > 60 ? '…' : ''}`,
      conversationId: currentConversation?.id,
      projectId: currentProject?.id,
    })
  }, [confirmDeleteMsgId, removeMessage, currentConversation?.id, currentProject?.id, t])
  /** 更新对话级模型设置（持久化到 localStorage，随消息发送；同时绑定到当前会话使上下文预算实时生效） */
  const updateModelOptions = (next: ChatOptions) => {
    setModelOptions(next)
    setJSON(STORAGE_KEYS.CHAT_OPTIONS, next)
    if (currentConversation && next.model_id !== 'auto') {
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
  const [mruRefs, setMruRefs] = useState<string[]>(() =>
    getJSON<string[]>(STORAGE_KEYS.REF_MRU, []),
  )
  const recordMruRef = (path: string) => {
    setMruRefs((prev) => {
      const next = [path, ...prev.filter((p) => p !== path)].slice(0, 30)
      setJSON(STORAGE_KEYS.REF_MRU, next)
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
      // 本批请求序号：响应回来时若序号已变（又改了关键字/模式）则丢弃，防乱序覆盖
      const seq = ++searchSeqRef.current
      if (searchMode === 'msg') {
        const pid = currentProject?.id
        const kw = searchText.trim()
        if (!pid || kw.length < 2) {
          setMsgHits([])
          setMsgSearching(false)
          setAllProjectSearching(false)
          return
        }
        setAllProjectSearching(false)
        setMsgSearching(true)
        searchMessages(pid, kw, msgSearchScope === 'current' ? currentConversation?.id : undefined)
          .then((hits) => {
            if (seq === searchSeqRef.current) setMsgHits(hits)
          })
          .catch(() => {
            if (seq === searchSeqRef.current) setMsgHits([])
          })
          .finally(() => {
            if (seq === searchSeqRef.current) setMsgSearching(false)
          })
      } else if (searchMode === 'all') {
        // 跨项目全文搜索
        const kw = searchText.trim()
        if (kw.length < 2) {
          setAllProjectHits([])
          setAllProjectSearching(false)
          setMsgSearching(false)
          return
        }
        setMsgSearching(false)
        setAllProjectSearching(true)
        searchMessagesAllProjects(kw)
          .then((hits) => {
            if (seq === searchSeqRef.current) setAllProjectHits(hits)
          })
          .catch(() => {
            if (seq === searchSeqRef.current) setAllProjectHits([])
          })
          .finally(() => {
            if (seq === searchSeqRef.current) setAllProjectSearching(false)
          })
      } else {
        setMsgHits([])
        setAllProjectHits([])
        setMsgSearching(false)
        setAllProjectSearching(false)
        void setConversationKeyword(searchText)
      }
    }, 300)
    return () => clearTimeout(h)
  }, [searchText, searchMode, msgSearchScope, currentConversation?.id, setConversationKeyword, currentProject?.id])

  // 加载项目下所有出现过的标签 + 频次（用于标签筛选下拉）
  useEffect(() => {
    const pid = currentProject?.id
    if (!pid) {
      setTagCounts([])
      return
    }
    listConversationTags(pid)
      .then(setTagCounts)
      .catch(() => setTagCounts([]))
  }, [currentProject?.id, conversations.length])

  /** 点击消息搜索命中：打开对应会话并高亮目标消息 */
  const openMessageHit = async (hit: MessageSearchHit) => {
    setSearchText('')
    setMsgHits([])
    await openConversation(hit.conversation_id)
    // 定位由 highlightMsgId effect 完成（虚拟列表就绪后 scrollToIndex 居中，3 秒后清除高亮）
    setHighlightMsgId(hit.message_id)
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

  // 快捷键：Ctrl+K 打开命令面板；Ctrl+Shift+K 聚焦会话搜索；Ctrl+Shift+B 构建 / Ctrl+Shift+D 部署 / Ctrl+Shift+N 新会话
  // [72] 工具级快捷键：Ctrl+Shift+S 截图验证（take_screenshot）/ Ctrl+Shift+R 运行命令（run_command）
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
        } else if (k === 's') {
          e.preventDefault()
          setDraft(t('home.quickScreenshotPrompt'))
          inputRef.current?.focus()
        } else if (k === 'r') {
          e.preventDefault()
          setDraft(t('home.quickRunPrompt'))
          inputRef.current?.focus()
        } else if (k === 'n') {
          e.preventDefault()
          void newConversation()
        } else if (k === 'k') {
          e.preventDefault()
          searchInputRef.current?.focus()
        } else if (k === 'p') {
          e.preventDefault()
          setProjectSwitcherOpen(true)
        }
      } else if (mod && e.key.toLowerCase() === 'k') {
        e.preventDefault()
        setPaletteOpen(true)
      } else if (e.key === '?' && !mod && !e.shiftKey) {
        // ? 打开快捷键速查（输入框聚焦时不触发，避免误触）
        if (document.activeElement?.tagName !== 'INPUT' && document.activeElement?.tagName !== 'TEXTAREA') {
          e.preventDefault()
          setShowShortcuts(true)
        }
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [newConversation, t])

  /** Web 预览：右侧栏内嵌加载 http/https 地址（iframe），失败时提示 */
  const handleOpenPreview = () => {
    const url = previewUrl.trim()
    if (!url) return
    setItem(STORAGE_KEYS.PREVIEW_URL, url)
    setPreviewSrc(url)
    setRightTab('preview')
  }

  // Agent UI 聚焦（ui_focus 工具，对齐 OpenHands canvas_ui_control）：agent 产出后主动
  // 把用户视线引到成果——切换右侧面板/打开文件预览。文件预览复用 deveco:open-file 机制
  // （FileTreePanel 监听：自动展开目录并弹出 FilePreviewDialog），需等面板挂载后再派发。
  useEffect(() => {
    let unlisten: (() => void) | undefined
    listen<{ conversation_id: string; command: string; path: string; tab: string }>('ui-focus', (event) => {
      const { command, path, tab } = event.payload
      if (command === 'navigate_to_file' || command === 'show_preview') {
        if (!path) return
        setShowRightPanel(true)
        setRightTab('files')
        setTimeout(() => {
          window.dispatchEvent(new CustomEvent('deveco:open-file', { detail: { path } }))
        }, 80)
      } else if (command === 'open_tab' && tab) {
        const allowed = ['files', 'git', 'preview', 'terminal', 'devices', 'overview', 'symbols', 'analyze'] as const
        if ((allowed as readonly string[]).includes(tab)) {
          setShowRightPanel(true)
          setRightTab(tab as typeof rightTab)
        }
      }
    }).then((fn) => { unlisten = fn }).catch(() => {})
    return () => unlisten?.()
  }, [])

  /** 符号索引预热：项目切换/启动后延迟到浏览器空闲时预热，避免抢占首帧渲染 CPU；
   *  让符号面板与首轮对话构建工程概要时秒出结果；静默执行不阻塞界面。 */
  // 当前项目 ID/类型：符号索引预热触发条件（避免 effect 内直接引用项目对象）
  const projectId = currentProject?.id
  const projectKind = currentProject?.kind
  useEffect(() => {
    if (!projectId || projectKind === 'global') return
    // 延迟到首帧渲染完成后再启动后台预热，避免消息列表首屏渲染时竞争主线程
    // 拆开两路以保证 cleanup 拿到正确句柄类型（setTimeout 返回 number / requestIdleCallback 返回 IdleCallbackHandle）
    const hasRic = typeof window !== 'undefined' && 'requestIdleCallback' in window
    let timerId: number | null = null
    let idleId: number | null = null
    if (hasRic) {
      idleId = window.requestIdleCallback(() => {
        warmupSymbolIndex(projectId, convRoot).catch(() => {})
      })
    } else {
      timerId = window.setTimeout(() => {
        warmupSymbolIndex(projectId, convRoot).catch(() => {})
      }, 300)
    }
    return () => {
      if (idleId !== null && 'cancelIdleCallback' in window) {
        window.cancelIdleCallback(idleId)
      } else if (timerId !== null) {
        window.clearTimeout(timerId)
      }
    }
  }, [projectId, projectKind, convRoot])

  /** 自动补扫：旧项目（添加时未做工作区扫描）或新添加项目模块为空时，
   *  在首帧渲染完成后的空闲时间异步扫描一次，不阻塞界面；
   *  扫描完成后刷新列表以更新类型标签与模块卡。 */
  useEffect(() => {
    if (!currentProject || currentProject.kind === 'global') return
    const isEmpty =
      !currentProject.workspace_modules ||
      currentProject.workspace_modules === '[]' ||
      currentProject.workspace_modules === ''
    if (!isEmpty) return
    let cancelled = false
    // 延迟到首帧渲染完成后再执行，避免项目切换瞬间的重扫描与消息渲染竞争主线程
    const hasRic = typeof window !== 'undefined' && 'requestIdleCallback' in window
    let timerId: number | null = null
    let idleId: number | null = null
    const run = () => {
      if (cancelled) return
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
    }
    if (hasRic) {
      idleId = window.requestIdleCallback(run)
    } else {
      timerId = window.setTimeout(run, 500)
    }
    return () => {
      cancelled = true
      if (idleId !== null && 'cancelIdleCallback' in window) {
        window.cancelIdleCallback(idleId)
      } else if (timerId !== null) {
        window.clearTimeout(timerId)
      }
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [currentProject?.id])

  /** 打开项目根目录系统终端（cmd 窗口），供用户手动执行命令 */
  const handleOpenTerminal = async () => {
    if (!currentProject) return
    try {
      await openTerminal(convRoot ?? currentProject.path)
    } catch (e) {
      alert(`${t('home.openTerminalFail')}: ${String(e)}`)
    }
  }

  /** 打开右侧栏内置终端面板（在项目目录执行命令） */
  const handleOpenShell = () => {
    if (!currentProject) return
    setRightTab('shell')
  }

  /** 诊断卡片操作：install_deps→切到工程分析面板执行 ohpm install；其他→打开 DevEco 或提示。
   *  操作完成后唤醒等待中的 Agent，使其根据结果重新构建验证 */
  const handleDiagnoseAction = async (card: { requestId: string; action: string; conversationId: string; id: string }) => {
    let completed = false
    let note = ''
    if (card.action === 'install_deps' && currentProject) {
      setRightTab('analyze')
      const installDir = convRoot ?? currentProject.path
      try {
        const log = await runOhpmInstall(installDir)
        completed = true
        note = '依赖安装完成'
        useProjectStore.setState((s) => ({
          terminalEntries: [
            ...s.terminalEntries,
            { id: `diag-${Date.now()}`, tool: 'ohpm install', args: installDir, status: 'done' as const, output: log, startedAt: Date.now(), durationMs: 0 },
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
    getHarmonyRoot(currentProject.id, convRoot)
      .then((r) => {
        if (!cancelled) setMainRootAbs(r.root)
      })
      .catch(() => {})
    return () => {
      cancelled = true
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [currentProject?.id, convRoot, moduleScanTick])
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

  /** 任务回滚：dry_run 预览 → ConfirmDialog 确认 → git reset --hard 回起点 → 写 audit */
  const handleRollback = async () => {
    if (!currentConversation || rollbackBusy) return
    setRollbackBusy(true)
    try {
      const info = await rollbackTask(currentConversation.id, true)
      if (!info.is_repo) {
        alert(t('home.rollbackNoRepo'))
        return
      }
      const detail = t('home.rollbackConfirm', {
        date: info.commit_date || '-',
        changed: String(info.changed),
        untracked: String(info.untracked),
      })
      // 用 ConfirmDialog 替代 window.confirm（更一致 UX + 危险色调 + 详细面板）
      // 通过 capture 闭包：onConfirm 内部展开后续逻辑，onCancel 直接终止
      askConfirm({
        title: t('home.rollbackTitle'),
        body: detail,
        tone: 'danger',
        confirmLabel: t('home.rollbackDoIt'),
        onConfirm: () => {
          // 二次保护：如果 onConfirm 时当前不在流式（rollbackBusy 已置 true），则执行
          void (async () => {
            try {
              const res = await rollbackTask(currentConversation.id, false)
              useAuditStore.getState().log({
                category: 'task.rollback',
                label: t('home.rollbackTitle'),
                detail: `${res.commit.slice(0, 8)} · ${res.commit_date || '-'} · ${currentProject?.name ?? ''}`,
                projectId: currentProject?.id,
                conversationId: currentConversation.id,
              })
              alert(t('home.rollbackDone', { commit: res.commit.slice(0, 8), date: res.commit_date || '-' }))
            } catch (e) {
              alert(`${t('home.rollbackFail')}\n${String(e)}`)
            } finally {
              setRollbackBusy(false)
            }
          })()
        },
        // 关闭时（包括取消）若还没执行 → 释放 busy，避免按钮永远转圈
        onCancel: () => setRollbackBusy(false),
      })
    } catch (e) {
      alert(`${t('home.rollbackNoRepo')}\n${String(e)}`)
      setRollbackBusy(false)
    }
  }

  // ---------- 会话时间线（时间旅行）：快照点列表 → 回到历史决策点重新引导 ----------
  const handleOpenTimeline = () => {
    if (!currentConversation) return
    setTimelineOpen(true)
    void loadSnapshots(currentConversation.id)
  }

  const handleRestoreSnapshot = (snap: SnapshotInfo) => {
    if (!currentConversation) return
    // 用 ConfirmDialog 确认：warn 色调提示后续消息将被归档隐藏（工作区/账本随之重置为快照时刻）
    askConfirm({
      title: t('home.timelineRestoreTitle'),
      body: t('home.timelineRestoreConfirm', { time: formatTime(snap.created_at), label: snap.label }),
      tone: 'warn',
      confirmLabel: t('home.timelineRestoreDoIt'),
      onConfirm: () => {
        void (async () => {
          try {
            const res = await restoreToSnapshot(currentConversation.id, snap.id)
            useAuditStore.getState().log({
              category: 'task.timeline',
              label: t('home.timelineTitle'),
              detail: `${snap.label} · ${formatTime(snap.created_at)} · ${currentProject?.name ?? ''}`,
              projectId: currentProject?.id,
              conversationId: currentConversation.id,
            })
            alert(t('home.timelineRestored', { archived: String(res.archived), restored: String(res.restored) }))
          } catch (e) {
            alert(`${t('home.timelineFail')}\n${String(e)}`)
          }
        })()
      },
    })
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
        // 划词菜单仅服务对话内容：选区须位于消息正文（.md-body）内，其它界面元素（文件树/设置/终端等）不弹菜单
        let anchor: Node | null = range.commonAncestorContainer
        if (anchor.nodeType === Node.TEXT_NODE) anchor = anchor.parentElement
        const msgBody = (anchor as Element | null)?.closest?.('.md-body')
        if (!msgBody) {
          setSelectionMenu(null)
          return
        }
        // 保存选区快照（点击菜单按钮后选区可能被浏览器清除，供操作前恢复高亮）
        selectionRangeRef.current = range.cloneRange()
        // 同步保存完整文本与容器：消息 DOM 被重建后 live Range 端点归一化收缩，
        // 恢复时若检测到文本变短，按文本在容器内重新定位端点
        selectionTextRef.current = raw
        selectionContainerRef.current = msgBody
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
  const toggleSpeak = useCallback((messageId: string, text: string) => {
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
  }, [speakingId])

  /** 导出会话：下载 md/txt/html 或复制全文 */
  const exportConversationFile = async (format: 'md' | 'txt' | 'html') => {
    const store = useProjectStore.getState()
    const text = store.exportConversation(format)
    const ext = format === 'html' ? 'html' : format === 'txt' ? 'txt' : 'md'
    const filename = `${currentConversation?.title ?? '对话记录'}.${ext}`
    // Tauri 环境走原生保存对话框（WebView2 对 Blob URL + a.download 支持不稳，会静默失效）
    if ('__TAURI_INTERNALS__' in window) {
      try {
        const dest = await dialogSave({ title: t('home.export'), defaultPath: filename })
        if (dest) await writeTextFile(dest, text)
      } catch {
        // 保存失败/取消静默
      }
    } else {
      const blob = new Blob([text], { type: format === 'html' ? 'text/html;charset=utf-8' : 'text/plain;charset=utf-8' })
      const url = URL.createObjectURL(blob)
      const a = document.createElement('a')
      a.href = url
      a.download = filename
      a.click()
      setTimeout(() => URL.revokeObjectURL(url), 1000)
    }
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
  const openVersionDialog = useCallback((message: ChatMessage, userMessageId: string) => {
    setVersionDialog({ userMessageId, current: message.content })
  }, [])

  // 稳定回调（MessageItem memo 化配套）：避免每轮渲染重建内联箭头导致全部历史消息重渲染
  const openFeedbackDialog = useCallback((id: string) => setFeedbackDialog({ messageId: id }), [])
  const regenerateLatest = useCallback(() => regenerateLast(modelOptions), [modelOptions, regenerateLast])
  const branchFrom = useCallback((msg: ChatMessage) => regenerateLast(modelOptions, msg.id), [modelOptions, regenerateLast])
  const toggleOps = useCallback(() => setOpsOpen((v) => !v), [])

  // —— 消息引用（Quote）：引用内容以 > 行插入草稿（模型可见），msg:{id} 落 references_json 供点击定位 ——
  const [pendingQuote, setPendingQuote] = useState<{ id: string; preview: string } | null>(null)
  const quoteMessage = useCallback(
    (m: ChatMessage) => {
      const body = m.content.trim().slice(0, 400)
      const quoted = body
        .split('\n')
        .slice(0, 8)
        .map((l) => `> ${l}`)
        .join('\n')
      setPendingQuote({ id: m.id, preview: body.split('\n')[0].slice(0, 60) })
      setDraft((d) => (d ? `${d}\n\n${quoted}\n\n` : `${quoted}\n\n`))
      inputRef.current?.focus()
    },
    [],
  )
  /** 点击引用标签：定位并高亮被引用消息（复用搜索命中的定位机制） */
  const locateQuotedMessage = useCallback((msgId: string) => setHighlightMsgId(msgId), [])
  /** 从该 user 消息派生新会话（Fork：复制至此消息，原会话保持不动） */
  const forkFromHere = useCallback(
    (msg: ChatMessage) => {
      if (confirmDeleteMsgId) return
      void forkCurrentConversation(msg.id)
    },
    [confirmDeleteMsgId, forkCurrentConversation],
  )

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
    // 记录被删除的会话元信息（用于 audit），删除后查不到
    const target = conversations.find((c) => c.id === id)
    await deleteConversation(id)
    useAuditStore.getState().log({
      category: 'conversation.delete',
      label: t('home.auditLabelConvDelete'),
      detail: target ? `${target.title} (${target.messages_count ?? '?'} messages)` : id,
      conversationId: id,
      projectId: currentProject?.id,
    })
  }

  /** 项目移除（二次确认）：第一击进入确认态（3 秒自动恢复），第二击执行删除。
   * 仅移除软件内记录（会话/消息/权限等），不影响磁盘上的项目文件 */
  const handleDeleteProject = async (id: string) => {
    if (confirmDeleteProjectId !== id) {
      setConfirmDeleteProjectId(id)
      setTimeout(() => setConfirmDeleteProjectId((cur) => (cur === id ? null : cur)), 3000)
      return
    }
    setConfirmDeleteProjectId(null)
    const target = projects.find((p) => p.id === id)
    await removeProject(id)
    useAuditStore.getState().log({
      category: 'project.remove',
      label: t('home.auditLabelProjectRemove'),
      detail: target?.name ?? id,
      projectId: id,
    })
  }

  /** 打开项目所在文件夹：系统默认文件管理器（Windows 资源管理器 / macOS Finder） */
  const handleOpenProjectFolder = async (p: Project) => {
    try {
      await shellOpen(p.path)
    } catch (e) {
      sendNotification(t('home.openFolderFail'), String(e), 'error')
    }
  }

  /** 刷新项目所有信息：工作区模块重扫 + 项目列表刷新；
   * 当前打开的项目联动刷新文件树 / 记忆 / 统计 / 分支 / 最近任务 */
  const handleRefreshProjectAll = async (p: Project) => {
    try {
      await rescanWorkspaceModules(p.id)
    } catch {
      // 扫描失败静默：不阻塞后续刷新
    }
    await refreshProjects()
    if (currentProject?.id === p.id) {
      await useProjectStore.getState().loadFileTree()
      await loadMemories()
      await loadToolStats()
      void loadToolTokenStats()
      void useProjectStore.getState().refreshGitBranches()
      void loadRecentRuns()
    }
    sendNotification(t('home.projectRefreshed'), p.name)
  }

  /** 置顶 / 取消置顶 */
  const togglePin = async (id: string, pinned: boolean) => {
    await pinConversation(id, !pinned)
    const target = conversations.find((c) => c.id === id)
    useAuditStore.getState().log({
      category: 'conversation.pin',
      label: pinned ? t('home.auditLabelConvUnpin') : t('home.auditLabelConvPin'),
      detail: target?.title ?? id,
      conversationId: id,
      projectId: currentProject?.id,
    })
  }

  /** 归档 / 取消归档（归档后按当前视图刷新） */
  const toggleArchive = async (id: string, archived: boolean) => {
    await archiveConversation(id, !archived)
    const target = conversations.find((c) => c.id === id)
    useAuditStore.getState().log({
      category: 'conversation.archive',
      label: archived ? t('home.auditLabelConvUnarchive') : t('home.auditLabelConvArchive'),
      detail: target?.title ?? id,
      conversationId: id,
      projectId: currentProject?.id,
    })
  }

  /** 导入会话为只读预览（纯前端：解析 md/json 文件 → 弹窗显示 → 提供"复制全文"按钮）
   *  - 不写数据库（避免污染历史/触发 agent）
   *  - 用户可复制后粘贴到任意会话作为参考
   *  - 支持格式：
   *     1) Markdown 标题 + ## User/Assistant 分段（导出自家格式）
   *     2) JSON 数组：[{role, content, reasoning?}, ...]（通用格式）
   */
  const handleImport = async () => {
    try {
      const path = await dialogOpen({
        multiple: false,
        filters: [
          { name: 'Conversation', extensions: ['md', 'txt', 'json'] },
          { name: 'All', extensions: ['*'] },
        ],
      })
      if (!path || Array.isArray(path)) return
      // Tauri 2 plugin-dialog 返回 string | string[]，上面已 narrow
      const filePath = path as string
      const text = await readTextFile(filePath)
      const title = filePath.split(/[\\/]/).pop() ?? 'imported'
      const parsed = parseImportText(text)
      if (parsed.length === 0) {
        sendNotification(t('home.importFail'), t('home.importEmpty'), 'error')
        return
      }
      setImportDialog({ open: true, title, messages: parsed })
    } catch (e) {
      sendNotification(t('home.importFail'), String(e), 'error')
    }
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
    setItem(STORAGE_KEYS.SIDEBAR_WIDTH, String(sidebarWidth))
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
    setItem(STORAGE_KEYS.RIGHT_WIDTH, String(rightWidth))
  }

  const formatTime = (ts: number) => {
    const d = new Date(ts * 1000)
    return d.toLocaleString(undefined, { month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit' })
  }

  /** ID 展示与复制工具：取前 8 位作为短 ID，点击复制完整 ID 并显示 1.5s 成功反馈 */
  const [copiedId, setCopiedId] = useState<string | null>(null)
  const copyId = useCallback(async (id: string) => {
    try {
      await navigator.clipboard.writeText(id)
      setCopiedId(id)
      setTimeout(() => setCopiedId((cur) => (cur === id ? null : cur)), 1500)
    } catch {
      /* 剪贴板不可用时静默失败 */
    }
  }, [])

  /** 解析导入文件文本 → 消息列表（role + content）
   *  - 优先尝试 JSON：[{role, content, reasoning?}, ...] 或 {messages: [...]}
   *  - 回退 Markdown：按 ## User/## Assistant 分段（与 exportConversationMd 输出一致）
   *  - 返回：只读预览结构，不入库
   */
  const parseImportText = (text: string): Array<{ role: string; content: string }> => {
    const trimmed = text.trim()
    // 1) JSON 优先
    if (trimmed.startsWith('[') || trimmed.startsWith('{')) {
      try {
        const obj = JSON.parse(trimmed)
        const arr: unknown[] = Array.isArray(obj) ? obj : Array.isArray((obj as { messages?: unknown[] })?.messages) ? (obj as { messages: unknown[] }).messages : []
        const out: Array<{ role: string; content: string }> = []
        for (const item of arr) {
          if (!item || typeof item !== 'object') continue
          const o = item as { role?: unknown; content?: unknown }
          const role = typeof o.role === 'string' ? o.role : 'user'
          const content = typeof o.content === 'string' ? o.content : ''
          if (content.trim()) out.push({ role, content })
        }
        return out
      } catch {
        // JSON 解析失败 → 走 markdown
      }
    }
    // 2) Markdown：按 ## 行分段
    const lines = text.split(/\r?\n/)
    const out: Array<{ role: string; content: string }> = []
    let cur: { role: string; content: string } | null = null
    const flush = () => { if (cur && cur.content.trim()) out.push(cur) }
    for (const ln of lines) {
      const h = /^\s*#{1,3}\s*(👤\s*User|🤖\s*Assistant|⚙️\s*\S+|\*\*\s*(User|Assistant)\s*\*\*|User|Assistant)\s*$/i.exec(ln)
      if (h) {
        flush()
        const label = h[1].toLowerCase()
        const role = label.includes('user') ? 'user' : label.includes('assistant') ? 'assistant' : 'system'
        cur = { role, content: '' }
        continue
      }
      if (cur) {
        // 跳过 HTML sub 行（导出自带的元数据）
        if (/^<sub>.*<\/sub>$/i.test(ln)) continue
        // 跳过水平分割线
        if (/^---+\s*$/.test(ln)) continue
        cur.content += (cur.content ? '\n' : '') + ln
      }
    }
    flush()
    return out
  }

  // 会话列表时间分组：今天 / 昨天 / 本周 / 更早；用于左侧会话列表快速定位
  // convGroupKey 为模块级纯函数（引用稳定），label 依赖 i18n 故用 useCallback 保持稳定
  const convGroupLabel = useCallback(
    (key: string) => {
      if (key === 'today') return t('home.dayToday')
      if (key === 'yesterday') return t('home.dayYesterday')
      if (key === 'week') return t('home.dayThisWeek')
      return t('home.dayEarlier')
    },
    [t],
  )
  // 智能归档建议：7+ 天无活动且未置顶 → 建议归档；30+ 天 → 强烈建议（前端标记，后端不自动改）
  // 阈值用户可在 ConfigPage 调整（本期先用默认值 7/30）
  const ARCHIVE_SUGGEST_DAYS = 7
  const ARCHIVE_STRONG_DAYS = 30
  const suggestArchive = (c: Conversation): 'normal' | 'suggest' | 'strong' => {
    if (c.is_pinned || c.archived) return 'normal'
    const days = Math.floor((Date.now() / 1000 - c.updated_at) / 86400)
    if (days >= ARCHIVE_STRONG_DAYS) return 'strong'
    if (days >= ARCHIVE_SUGGEST_DAYS) return 'suggest'
    return 'normal'
  }
  // 按置顶 + 分组 + 时间倒序排好，并在组边界插入 {kind:'header'} 标签项
  // 置顶项按时间倒序排成"置顶"虚拟组（不显示标签）；其余按"今天/昨天/本周/更早"分组显示
  const groupedConversations = useMemo(() => {
    // 标签过滤：null 不过滤；string 时只保留 tags 包含该值的会话
    const filtered = activeTagFilter
      ? conversations.filter((c) => (c.tags ?? '').split(',').map((s) => s.trim()).includes(activeTagFilter))
      : conversations
    const sorted = [...filtered].sort((a, b) => {
      if (Number(a.is_pinned) !== Number(b.is_pinned)) return Number(b.is_pinned) - Number(a.is_pinned)
      return b.updated_at - a.updated_at
    })
    const out: Array<
      | { kind: 'header'; key: string; label: string }
      | { kind: 'item'; conv: Conversation; key: string }
    > = []
    let lastKey = ''
    for (const c of sorted) {
      if (c.is_pinned) {
        // 置顶项：直接追加，不参与分组
        out.push({ kind: 'item', conv: c, key: c.id })
        continue
      }
      const gk = convGroupKey(c.updated_at)
      if (gk !== lastKey) {
        lastKey = gk
        out.push({ kind: 'header', key: `gh-${gk}`, label: convGroupLabel(gk) })
      }
      out.push({ kind: 'item', conv: c, key: c.id })
    }
    return out
  }, [conversations, activeTagFilter, convGroupLabel])

  // token 数缩写（1.2k / 3.4w），标题下累计展示用
  const fmtTokens = (n: number) =>
    n >= 10000 ? `${(n / 10000).toFixed(1)}w` : n >= 1000 ? `${(n / 1000).toFixed(1)}k` : String(n)

  // 概览面板：切换到 overview 时刷新最近任务（task_runs 明细）
  useEffect(() => {
    if (rightTab === 'overview' && currentProject?.id) void loadRecentRuns()
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

  // ---------- 命令面板（Cmd+K）：后端静态命令 + 前端动态命令（会话/模型） ----------
  // 依赖会话/模型/当前会话 ID，变化时重建命令列表；run 回调在点击时才执行，捕获最新闭包
  const paletteCommands = useMemo<PaletteCommand[]>(() => {
    // 后端 action:<name> → 本地回调映射（未知动作静默忽略，向后兼容后端新增命令）
    const runAction = (name: string) => {
      switch (name) {
        case 'new_chat':
          void newConversation()
          break
        case 'compact':
          void handleCompact()
          break
        case 'rollback':
          void handleRollback()
          break
        case 'rules':
          void openRulesDialog()
          break
        case 'slash_build':
          setDraft(t('home.quickBuildPrompt'))
          inputRef.current?.focus()
          break
        case 'slash_deploy':
          setDraft(t('home.quickDeployPrompt'))
          inputRef.current?.focus()
          break
        case 'slash_plan':
          updateModelOptions({ ...modelOptions, plan_mode: true })
          setDraft(t('home.slashPlanPrompt'))
          inputRef.current?.focus()
          break
        case 'slash_fix':
          setDraft(t('home.slashFixPrompt'))
          inputRef.current?.focus()
          break
        case 'slash_review':
          setDraft(t('home.slashReviewPrompt'))
          inputRef.current?.focus()
          break
        case 'toggle_theme':
          toggleTheme()
          break
      }
    }
    const cmds: PaletteCommand[] = []
    // 后端静态命令：nav:<path> 跳转；action:<name> 本地执行
    for (const b of backendCmds) {
      cmds.push({
        id: b.id,
        title: b.title,
        subtitle: b.subtitle,
        group: b.group,
        icon: (b.icon || 'check') as IconName,
        run: () => {
          if (b.id.startsWith('nav:')) navigate(b.id.slice(4))
          else if (b.id.startsWith('action:')) runAction(b.id.slice(7))
        },
      })
    }
    // 动态命令：切换会话（置顶优先展示）
    const convs = [...conversations].sort((a, b) => Number(b.is_pinned) - Number(a.is_pinned))
    for (const c of convs) {
      cmds.push({
        id: `conv:${c.id}`,
        title: c.title || '(未命名会话)',
        subtitle: c.is_pinned ? '置顶会话' : new Date(c.updated_at).toLocaleString(),
        group: '切换会话',
        icon: 'chat',
        keywords: c.title,
        run: () => {
          void openConversation(c.id)
        },
      })
    }
    // 动态命令：切换模型（按 Provider 分组展示）
    for (const g of modelCatalog) {
      for (const m of g.models) {
        if (!m.enabled) continue
        cmds.push({
          id: `model:${m.id}`,
          title: m.display_name ?? m.model_id,
          subtitle: g.providerName,
          group: '切换模型',
          icon: 'spark',
          keywords: m.model_id,
          run: () => {
            // 函数式更新避免捕获过期 modelOptions
            setModelOptions((prev) => {
              const next = { ...prev, model_id: m.id }
              setJSON(STORAGE_KEYS.CHAT_OPTIONS, next)
              return next
            })
          },
        })
      }
    }
    return cmds
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [backendCmds, conversations, modelCatalog, currentConversation?.id, t])

  return (
    <div className="flex h-screen w-screen overflow-hidden bg-[var(--bg-window)]">
      {/* 全局命令面板（Cmd+K）：后端静态命令 + 会话/模型动态命令 */}
      <CommandPalette
        open={paletteOpen}
        onClose={() => setPaletteOpen(false)}
        commands={paletteCommands}
      />
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
          <Button
            variant="primary"
            size="md"
            icon="plus"
            title={t('home.addProject')}
            onClick={() => setShowAddDialog(true)}
            className={sidebarCollapsed ? 'w-9 mx-auto' : 'w-full'}
          >
            {!sidebarCollapsed && t('home.addProject')}
          </Button>
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
                    onContextMenu={(e) => {
                      e.preventDefault()
                      e.stopPropagation()
                      setProjectMenu({ x: e.clientX, y: e.clientY, project: p })
                    }}
                    title={p.name}
                    className={`group relative flex items-center gap-2 rounded-lg text-left transition-colors ${
                      sidebarCollapsed ? 'w-9 h-9 justify-center' : 'w-full pl-3 pr-2 py-[7px]'
                    } ${active ? 'bg-[var(--accent-soft)]' : 'hover:bg-[var(--bg-hover)]'}`}
                  >
                    <Icon name="folder" size={15} className={`shrink-0 ${active ? '' : 'opacity-60'}`} />
                    {!sidebarCollapsed && (
                      <>
                        <span className="min-w-0 flex-1">
                          <span
                            className={`block text-[13px] truncate leading-4 ${
                              active ? 'text-[var(--text-primary)]' : 'text-[var(--text-secondary)] group-hover:text-[var(--text-primary)]'
                            }`}
                          >
                            {p.name}
                          </span>
                          {/* 项目 ID：点击复制完整 ID，排查问题用（标题下小字） */}
                          <button
                            onClick={(e) => {
                              e.stopPropagation()
                              void copyId(p.id)
                            }}
                            className="block max-w-full truncate text-left font-mono text-[9.5px] leading-3.5 text-[var(--text-muted)]/70 hover:text-[var(--accent)] transition-colors"
                            title={`${t('home.projId')}: ${p.id}\n${t('home.clickToCopy')}`}
                          >
                            {copiedId === p.id ? <Icon name="check" size={8} className="inline text-[var(--success)]" /> : 'P:'}
                            {shortId(p.id)}
                          </button>
                        </span>
                        {total > 0 && (
                          <span
                            className="shrink-0 min-w-[16px] h-4 px-1 flex items-center justify-center rounded-full bg-[var(--accent-soft)] text-[var(--accent)] text-[10px] font-medium"
                            title={t('home.scopedCountHint', { mcp: counts!.mcp, skills: counts!.skills })}
                          >
                            {total}
                          </span>
                        )}
                        {/* 会话数：项目维度统计，与模块能力计数区分展示 */}
                        {p.conversation_count > 0 && (
                          <span
                            className="shrink-0 min-w-[16px] h-4 px-1 flex items-center justify-center rounded-full modern-card border-[var(--border)] text-[var(--text-muted)] text-[10px] font-medium tabular-nums"
                            title={t('home.conversationCount', { n: p.conversation_count })}
                          >
                            {p.conversation_count}
                          </span>
                        )}
                        {/* 分隔线：危险操作与常规操作拉开视觉距离，降低误触 */}
                        <span className="mx-0.5 w-px h-3.5 bg-[var(--border)] shrink-0" aria-hidden="true" />
                        <button
                          onClick={(e) => {
                            e.stopPropagation()
                            void toggleProjectPin(p.id)
                          }}
                          className={`p-1 ml-0.5 rounded-md transition-all shrink-0 ${
                            p.pinned
                              ? 'text-[var(--accent)] opacity-100'
                              : 'text-[var(--text-muted)] opacity-0 group-hover:opacity-100 hover:text-[var(--accent)] hover:bg-[var(--bg-hover)]'
                          }`}
                          title={p.pinned ? t('home.unpinProject') : t('home.pinProject')}
                        >
                          <Icon name="pin" size={13} />
                        </button>
                        <button
                          onClick={(e) => {
                            e.stopPropagation()
                            void handleDeleteProject(p.id)
                          }}
                          className={`p-1 ml-0.5 rounded-md transition-all shrink-0 ${
                            confirmDeleteProjectId === p.id
                              ? 'bg-[var(--danger)] text-white shadow-[0_0_0_3px_var(--danger-50)] opacity-100'
                              : 'text-[var(--text-muted)] opacity-0 group-hover:opacity-100 hover:text-[var(--danger)] hover:bg-[var(--bg-hover)]'
                          }`}
                          title={
                            confirmDeleteProjectId === p.id
                              ? t('home.confirmDeleteProject')
                              : t('home.deleteProject')
                          }
                        >
                          <Icon name="delete" size={13} white={confirmDeleteProjectId === p.id} />
                        </button>
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

        {/* 最近项目右键菜单：打开项目所在文件夹 / 刷新项目信息 */}
        {projectMenu && (
          <div
            ref={projectMenuRef}
            className="fixed z-[var(--app-z-popover)] w-52 rounded-xl modern-card shadow-2xl shadow-black/40 py-1 animate-modal-in"
            style={{
              left: Math.min(projectMenu.x, window.innerWidth - 220),
              top: Math.min(projectMenu.y, window.innerHeight - 140),
            }}
            onMouseDown={(e) => e.stopPropagation()}
          >
            <div className="px-3 py-1.5 text-[11px] font-medium text-[var(--text-muted)] truncate" title={projectMenu.project.path}>
              {projectMenu.project.name}
            </div>
            <div className="mx-2 my-1 h-px bg-[var(--border)]" aria-hidden="true" />
            <button
              onClick={() => {
                if (!projectMenu) return
                void handleOpenProjectFolder(projectMenu.project)
                setProjectMenu(null)
              }}
              className="w-full flex items-center gap-2.5 px-3 py-2 text-[12px] text-[var(--text-secondary)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] transition-colors"
            >
              <Icon name="folder" size={13} />
              {t('home.openProjectFolder')}
            </button>
            <button
              onClick={() => {
                if (!projectMenu) return
                void handleRefreshProjectAll(projectMenu.project)
                setProjectMenu(null)
              }}
              className="w-full flex items-center gap-2.5 px-3 py-2 text-[12px] text-[var(--text-secondary)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] transition-colors"
            >
              <Icon name="refresh" size={13} />
              {t('home.refreshProjectInfo')}
            </button>
          </div>
        )}

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
                <IconButton icon="plus" label={t('home.newConversation')} onClick={() => newConversation()} />
                {/* 导入会话（只读预览：解析 md/json 文件，弹窗显示 + 复制全文） */}
                <IconButton icon="file" label={t('home.import')} onClick={handleImport} />
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
                <button
                  onClick={() => {
                    setSearchMode('all')
                    void setConversationKeyword('')
                    setMsgHits([])
                  }}
                  className={`flex-1 h-6 rounded-md text-[10.5px] transition-colors ${
                    searchMode === 'all'
                      ? 'bg-[var(--accent-soft)] text-[var(--accent)] font-medium'
                      : 'text-[var(--text-muted)] hover:text-[var(--text-secondary)] hover:bg-[var(--bg-hover)]'
                  }`}
                >
                  {t('home.searchModeAll')}
                </button>
              </div>
              {/* 消息搜索范围：全部会话 / 仅本会话（有当前会话时才可限定） */}
              {searchMode === 'msg' && currentConversation && (
                <div className="flex gap-1 mt-1">
                  <button
                    onClick={() => setMsgSearchScope(msgSearchScope === 'current' ? 'all' : 'current')}
                    className={`h-5 px-2 rounded text-[10px] transition-colors ${
                      msgSearchScope === 'current'
                        ? 'bg-[var(--accent-soft)] text-[var(--accent)] font-medium'
                        : 'text-[var(--text-muted)] hover:text-[var(--text-secondary)] hover:bg-[var(--bg-hover)]'
                    }`}
                    title={t('home.msgScopeCurrent')}
                  >
                    {msgSearchScope === 'current' ? `⌖ ${t('home.msgScopeCurrent')}` : t('home.msgScopeAll')}
                  </button>
                </div>
              )}
              {/* 标签筛选下拉：拉取项目下所有出现过的标签，点击过滤会话列表 */}
              {currentProject && tagCounts.length > 0 && (
                <div className="mt-1.5 px-1 flex flex-wrap gap-1">
                  <button
                    onClick={() => setActiveTagFilter(null)}
                    className={`px-1.5 py-0.5 rounded text-[10px] font-medium transition-colors ${
                      activeTagFilter === null
                        ? 'tab-active'
                        : 'tab-inactive'
                    }`}
                  >
                    {t('home.tagAll')}
                  </button>
                  {tagCounts.slice(0, 8).map((tc) => (
                    <button
                      key={tc.tag}
                      onClick={() => setActiveTagFilter(activeTagFilter === tc.tag ? null : tc.tag)}
                      className={`px-1.5 py-0.5 rounded text-[10px] font-medium transition-colors ${
                        activeTagFilter === tc.tag
                          ? 'tab-active'
                          : 'tab-inactive'
                      }`}
                    >
                      {tc.tag} <span className="opacity-60 ml-0.5 tnum">{tc.count}</span>
                    </button>
                  ))}
                </div>
              )}
              {/* 消息全文搜索结果下拉 */}
              {searchMode === 'msg' && searchText.trim().length >= 2 && (
                <div className="mt-1.5 max-h-72 overflow-y-auto rounded-lg modern-card shadow-lg shadow-black/20 py-1">
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
              {/* 跨项目全文搜索结果下拉：按项目分组 */}
              {searchMode === 'all' && searchText.trim().length >= 2 && (
                <div className="mt-1.5 max-h-72 overflow-y-auto rounded-lg modern-card shadow-lg shadow-black/20 py-1">
                  {allProjectSearching && allProjectHits.length === 0 && (
                    <div className="px-2.5 py-2 text-[11px] text-[var(--text-muted)]">{t('home.searching')}</div>
                  )}
                  {!allProjectSearching && allProjectHits.length === 0 && (
                    <div className="px-2.5 py-2 text-[11px] text-[var(--text-muted)]">{t('home.noMessageHits')}</div>
                  )}
                  {(() => {
                    // 按项目分组
                    const groups = new Map<string, MessageSearchHit[]>()
                    for (const h of allProjectHits) {
                      const key = h.project_id ?? '_'
                      const arr = groups.get(key) ?? []
                      arr.push(h)
                      groups.set(key, arr)
                    }
                    const out: JSX.Element[] = []
                    for (const [pid, hits] of groups) {
                      const first = hits[0]
                      out.push(
                        <div key={`gh-${pid}`} className="group-label" style={{ paddingLeft: 10, paddingRight: 10, paddingTop: 6, paddingBottom: 2 }}>
                          <span className="truncate">{first.project_name ?? pid} · {hits.length}</span>
                        </div>
                      )
                      for (const hit of hits.slice(0, 5)) {
                        out.push(
                          <button
                            key={hit.message_id}
                            onClick={() => {
                              // 跨项目跳转：先切项目，再打开会话，最后定位高亮目标消息
                              const pj = useProjectStore.getState().projects.find((p) => p.id === hit.project_id)
                              if (pj) {
                                openProject(pj.id)
                                  .then(() => openConversation(hit.conversation_id))
                                  .then(() => setHighlightMsgId(hit.message_id))
                                  .catch(() => {})
                              }
                              setSearchText('')
                              setAllProjectHits([])
                            }}
                            className="w-full text-left px-2.5 py-1.5 hover:bg-[var(--bg-hover)] transition-colors"
                          >
                            <div className="flex items-center gap-1.5 text-[10.5px] text-[var(--text-muted)]">
                              <span className="truncate font-medium">{hit.conversation_title}</span>
                              <span className="shrink-0">{hit.role === 'user' ? '👤' : '✨'}</span>
                            </div>
                            <div className="text-[11px] text-[var(--text-secondary)] line-clamp-2 mt-0.5">{hit.snippet}</div>
                          </button>
                        )
                      }
                    }
                    return out
                  })()}
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
            {groupedConversations.map((row) => {
              if (row.kind === 'header') {
                return (
                  <div key={row.key} className="group-label" style={{ paddingLeft: 10, paddingRight: 10, paddingTop: 10, paddingBottom: 4 }}>
                    <span>{row.label}</span>
                  </div>
                )
              }
              const c = row.conv
              const active = c.id === currentConversation?.id
              const renaming = renamingId === c.id
              const pendingItems = pendingConfirmations[c.id] ?? []
              return (
                <div
                  key={c.id}
                  className={`list-row group w-full flex items-center rounded-lg transition-colors ${
                    active ? 'bg-[var(--bg-card)] is-active' : 'hover:bg-[var(--bg-hover)]'
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
                    <IconButton
                      icon="chat"
                      label={c.title}
                      iconSize={15}
                      className="h-8 w-8"
                      onClick={() => openConversation(c.id)}
                    />
                  ) : (
                    <>
                      <button onClick={() => openConversation(c.id)} className="flex-1 min-w-0 text-left">
                        <span className="flex items-center gap-1.5">
                          <span
                            className={`block text-[13px] truncate min-w-0 ${c.is_pinned ? 'text-[var(--accent)]' : active ? 'text-[var(--text-primary)]' : 'text-[var(--text-secondary)] group-hover:text-[var(--text-primary)]'}`}
                          >
                            {c.is_pinned && <Icon name="pin" size={11} className="mr-1 text-[var(--accent)] align-[-1px]" />}
                            {c.title}
                          </span>
                          {c.work_mode === 'worktree' && (
                            <span
                              className="shrink-0 inline-flex items-center gap-1 text-[9.5px] px-1 py-px rounded font-medium bg-[var(--accent)]/10 text-[var(--accent)]"
                              title={c.worktree_path ?? c.worktree_branch ?? 'worktree'}
                            >
                              <Icon name="git-branch" size={9} />
                              {c.worktree_branch || 'worktree'}
                            </span>
                          )}
                        </span>
                        {/* 待确认角标：审批(危险=红) / 计划(待批准=橙) / 提问(待回答=蓝)，醒目提示该会话在等你 */}
                        {pendingItems.length > 0 && (
                          <span className="flex items-center gap-1 mt-0.5 flex-wrap">
                            {pendingItems.some((p) => p.kind === 'approval') && (
                              <span
                                className="flex items-center gap-1 px-1.5 py-px rounded text-[9.5px] font-medium bg-[var(--danger-50)] text-[var(--danger)]"
                                title={t('home.pendingApprovalTip')}
                              >
                                <Icon name="bolt" size={9} />
                                {t('home.pendingApproval')}
                              </span>
                            )}
                            {pendingItems.some((p) => p.kind === 'plan') && (
                              <span
                                className="flex items-center gap-1 px-1.5 py-px rounded text-[9.5px] font-medium bg-[var(--warning-50)] text-[var(--warning-600)]"
                                title={t('home.pendingPlanTip')}
                              >
                                <Icon name="lightbulb" size={9} />
                                {t('home.pendingPlan')}
                              </span>
                            )}
                            {pendingItems.some((p) => p.kind === 'ask') && (
                              <span
                                className="flex items-center gap-1 px-1.5 py-px rounded text-[9.5px] font-medium bg-[var(--accent-soft)] text-[var(--accent)]"
                                title={t('home.pendingAskTip')}
                              >
                                <Icon name="headphones" size={9} />
                                {t('home.pendingAsk')}
                              </span>
                            )}
                          </span>
                        )}
                        {/* 标签 chip 行（最多显示 3 个，超出 +N） */}
                        {c.tags && (
                          <div className="flex items-center gap-1 mt-0.5 flex-wrap">
                            {c.tags.split(',').slice(0, 3).map((tag) => (
                              <span
                                key={tag}
                                className="px-1.5 py-px rounded text-[9.5px] font-medium bg-[var(--accent-soft)] text-[var(--accent)] border border-[var(--accent)]/20"
                              >
                                {tag.trim()}
                              </span>
                            ))}
                            {c.tags.split(',').length > 3 && (
                              <span className="text-[9.5px] text-[var(--text-muted)]">+{c.tags.split(',').length - 3}</span>
                            )}
                          </div>
                        )}
                        {runningConversationIds.has(c.id) ? (
                          <ConversationRunStatus
                            conversationId={c.id}
                            foreground={streamingConversationId === c.id}
                          />
                        ) : (
                          <span className="flex items-center gap-1.5 mt-0.5">
                            <span className="text-[11px] text-[var(--text-muted)]">{formatTime(c.updated_at)}</span>
                            {/* 会话短 ID：hover 可见，点击复制 */}
                            <button
                              onClick={(e) => { e.stopPropagation(); copyId(c.id) }}
                              className="debug-id-badge font-mono text-[8.5px] px-1 py-px rounded border border-transparent text-[var(--text-muted)]/70 hover:text-[var(--accent)] hover:border-[var(--accent)]/40 transition-all opacity-0 group-hover:opacity-100"
                              title={`${t('home.convId')}: ${c.id}\n${t('home.clickToCopy')}`}
                            >
                              {copiedId === c.id ? <Icon name="check" size={8} className="text-[var(--success)]" /> : '#'}
                              {shortId(c.id)}
                            </button>
                            {/* 智能归档建议徽章：7+ 天未活动弱提醒，30+ 天强提醒（强建议时背景更显眼） */}
                            {(() => {
                              const sug = suggestArchive(c)
                              if (sug === 'suggest') {
                                return (
                                  <span
                                    className="text-[9.5px] px-1 rounded font-medium bg-[var(--warning-50)] text-[var(--warning-600)]"
                                    title={t('home.suggestArchiveHint')}
                                  >
                                    {t('home.suggestArchive')}
                                  </span>
                                )
                              }
                              if (sug === 'strong') {
                                return (
                                  <span
                                    className="text-[9.5px] px-1 rounded font-medium bg-[var(--danger-50)] text-[var(--danger)]"
                                    title={t('home.strongArchiveHint')}
                                  >
                                    {t('home.strongArchive')}
                                  </span>
                                )
                              }
                              return null
                            })()}
                            {!active && (unreadMap[c.id] ?? 0) > 0 && (
                              <span className="min-w-[16px] h-[16px] px-1 flex items-center justify-center rounded-full btn-primary text-[9.5px] font-semibold leading-none">
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
                      <IconButton
                        icon="archive"
                        label={c.archived ? t('home.unarchive') : t('home.archive')}
                        iconSize={13}
                        className="opacity-0 group-hover:opacity-100"
                        onClick={(e) => {
                          e.stopPropagation()
                          toggleArchive(c.id, c.archived)
                        }}
                      />
                      <IconButton
                        icon="edit"
                        label={t('home.rename')}
                        iconSize={13}
                        className="opacity-0 group-hover:opacity-100"
                        onClick={(e) => {
                          e.stopPropagation()
                          setRenamingId(c.id)
                          setRenamingText(c.title)
                        }}
                      />
                      {/* 分隔线：危险操作与常规操作拉开视觉距离，降低误触 */}
                      <span className="mx-0.5 w-px h-3.5 bg-[var(--border)] shrink-0" aria-hidden="true" />
                      <button
                        onClick={(e) => {
                          e.stopPropagation()
                          handleDeleteConversation(c.id)
                        }}
                        className={`p-1 ml-0.5 rounded-md transition-all shrink-0 ${
                          confirmDeleteId === c.id
                            ? 'bg-[var(--danger)] text-white shadow-[0_0_0_3px_var(--danger-50)] opacity-100'
                            : 'text-[var(--text-muted)] opacity-0 group-hover:opacity-100 hover:text-[var(--danger)] hover:bg-[var(--bg-hover)]'
                        }`}
                        title={
                          confirmDeleteId === c.id
                            ? t('home.confirmDeleteConversation')
                            : t('home.deleteConversation')
                        }
                      >
                        <Icon name="delete" size={13} white={confirmDeleteId === c.id} />
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

        {/* 底部：设置 + 语言 + 主题 + 折叠 */}
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
                className={`absolute bottom-full mb-1.5 rounded-xl modern-card shadow-2xl shadow-black/40 py-1 z-50 animate-modal-in ${sidebarCollapsed ? 'left-0' : 'left-0'} w-52`}
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
                    {item.path === '/health' && envHealth && (
                      <span
                        className={`w-2 h-2 rounded-full shrink-0 ${
                          envHealth === 'ok'
                            ? 'bg-[var(--success)]'
                            : envHealth === 'bad'
                              ? 'bg-[var(--danger)]'
                              : 'bg-[var(--warning)]'
                        }`}
                        title={
                          envHealth === 'ok'
                            ? t('home.envHealthOk')
                            : envHealth === 'bad'
                              ? t('home.envHealthBad')
                              : t('home.envHealthWarn')
                        }
                      />
                    )}
                  </button>
                ))}
                {/* 审计日志：弹层形式打开（不走导航），与 settings 平级 */}
                <div className="border-t border-[var(--border)] mt-1 pt-1">
                  <button
                    onClick={() => {
                      setShowSettingsMenu(false)
                      setAuditOpen(true)
                    }}
                    className="w-full flex items-center gap-2.5 px-3 py-2 text-[12px] text-[var(--text-secondary)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] transition-colors"
                  >
                    <Icon name="history" size={14} />
                    <span className="flex-1 text-left">{t('home.audit')}</span>
                    <span className="text-[10px] text-[var(--text-muted)] tnum">
                      {useAuditStore.getState().entries.length}
                    </span>
                  </button>
                </div>
              </div>
            )}
          </div>
          <LangToggle />
          <IconButton
            icon={themeResolved === 'dark' ? 'sun' : 'moon'}
            label={t('home.theme')}
            pad="lg"
            iconSize={15}
            className={sidebarCollapsed ? 'h-9 w-9' : undefined}
            onClick={toggleTheme}
          />
          <NotificationBell fixed />
          <IconButton
            icon={sidebarCollapsed ? 'chevron-right' : 'chevron-left'}
            label={sidebarCollapsed ? t('home.expandSidebar') : t('home.collapseSidebar')}
            pad="lg"
            iconSize={15}
            className={sidebarCollapsed ? 'mt-1 h-9 w-9' : undefined}
            onClick={() =>
              setSidebarCollapsed((v) => {
                const next = !v
                setItem(STORAGE_KEYS.SIDEBAR_COLLAPSED, next ? '1' : '0')
                return next
              })
            }
          />
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
        <div className="sr-only" role="status" aria-live="polite" aria-atomic="true">
          {stopRequested
            ? t('home.stopping')
            : isStreaming
              ? t('home.agentWorking')
              : streamingError
                ? streamingError
                : lastTaskSummary
                  ? t(lastTaskSummary.status === 'incomplete' ? 'home.taskIncompleteTitle' : 'home.taskDoneTitle')
                  : ''}
        </div>
        {/* 顶部栏 */}
        <header className="glass-bar h-14 shrink-0 border-b border-[var(--border)] flex items-center justify-between px-4 z-20">
          <div className="flex items-center gap-2.5 min-w-0">
            <div className="w-7 h-7 rounded-lg bg-gradient-to-br from-[var(--accent)] to-[#8b5cf6] flex items-center justify-center brand-glow shrink-0">
              <Icon name="bolt" size={14} white />
            </div>
            <span className="text-[13.5px] font-medium truncate">
              {currentConversation?.title ?? (currentProject ? currentProject.name : t('home.welcome'))}
            </span>
            {/* 会话 ID：点击复制完整 ID，排查问题用 */}
            {currentConversation && (
              <button
                onClick={() => copyId(currentConversation.id)}
                className="debug-id-badge shrink-0 font-mono text-[9.5px] px-1.5 py-0.5 rounded-md border border-[var(--border)] bg-[var(--bg-secondary)]/60 text-[var(--text-muted)] hover:text-[var(--accent)] hover:border-[var(--accent)]/40 hover:bg-[var(--accent-soft)]/40 transition-colors"
                title={`${t('home.convId')}: ${currentConversation.id}\n${t('home.clickToCopy')}`}
              >
                {copiedId === currentConversation.id ? <Icon name="check" size={9} className="text-[var(--success)]" /> : '#'}
                {shortId(currentConversation.id)}
              </button>
            )}
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
          </div>
          {/* 顶栏右侧：更多操作折叠菜单 + 面板开关 */}
          <div className="flex items-center gap-1.5">
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
                  <div className="absolute right-0 top-full mt-1.5 rounded-xl modern-card shadow-2xl shadow-black/40 py-1 z-50 w-52 animate-modal-in">
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
                        <div className="absolute right-full top-0 mr-1 rounded-xl modern-card shadow-2xl shadow-black/40 py-1 w-44 animate-modal-in">
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
                        handleOpenTimeline()
                      }}
                      disabled={messages.length === 0}
                      className="w-full flex items-center gap-2.5 px-3 py-2 text-[12px] text-[var(--text-secondary)] hover:text-[var(--accent)] hover:bg-[var(--bg-hover)] transition-colors disabled:opacity-40"
                    >
                      <Icon name="history" size={14} />
                      {t('home.timeline')}
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
                          handleOpenShell()
                        }}
                        className="w-full flex items-center gap-2.5 px-3 py-2 text-[12px] text-[var(--text-secondary)] hover:text-[var(--accent)] hover:bg-[var(--bg-hover)] transition-colors"
                      >
                        <Icon name="terminal" size={14} />
                        {t('home.openShell')}
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
                  setItem(STORAGE_KEYS.RIGHT_PANEL, next ? 'expanded' : 'collapsed')
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

        {/* [66] 工具链缺失横幅：启动自动 ping 发现关键工具链缺失时展示，点击跳转体检页 */}
        {toolHealthMissing.length > 0 && (
          <button
            onClick={() => navigate('/health')}
            className="shrink-0 flex items-center justify-center gap-2 px-4 py-1.5 text-[11px] bg-[var(--warning)]/15 text-[var(--warning)] border-b border-[var(--border)] hover:bg-[var(--warning)]/25 transition-colors"
          >
            <Icon name="health" size={12} />
            {t('home.toolsHealthBanner', { missing: toolHealthMissing.join(' / ') })}
          </button>
        )}

        {/* 消息区 / 空状态 */}
        <div ref={scrollRef} onScroll={handleScroll} className="chat-scroll flex-1 overflow-y-auto px-6 py-6">
          {!currentProject ? (
            <EmptyState
              onAdd={() => setShowAddDialog(true)}
              onImport={() => void handleImport()}
              onAudit={() => setAuditOpen(true)}
              onCost={() => navigate('/cost')}
            />
          ) : messages.length === 0 && !isStreaming ? (
            switchingConv ? (
              // 会话切换中：messages 已清空、新会话消息尚未加载完成，轻量占位避免空状态闪烁
              <div className="h-full flex flex-col items-center justify-center text-center">
                <Spinner size={32} />
                <p className="text-[12px] text-[var(--text-muted)] mt-3">{t('home.loadingConv')}</p>
              </div>
            ) : (
              <ChatEmptyState onQuick={(text) => fillDraft(text)} />
            )
          ) : (
            <div className="max-w-3xl mx-auto">
              {/* Pinned 消息条：纯前端 localStorage，会话顶部钉住关键消息
               * - 只展示当前会话已 pin 且仍存在的消息
               * - 点 chip 滚动到该消息（用 scrollIntoView + 高亮 2 秒）
               * - 点 × 取消 pin */}
              {currentConversation && (
              <>
              {/* 会话笔记：用户写"这次会话的目标/约定"，纯前端 localStorage
               * - 默认收起，点编辑/查看展开
               * - 自动保存（debounce 600ms） */}
              <ConvNoteBar convId={currentConversation.id} />
              <PinnedBar convId={currentConversation.id} onJump={(msgId) => {
                const el = document.querySelector(`[data-msg-id="${msgId}"]`) as HTMLElement | null
                if (el) {
                  el.scrollIntoView({ behavior: 'smooth', block: 'center' })
                  setHighlightMsgId(msgId)
                  setTimeout(() => setHighlightMsgId((cur) => (cur === msgId ? null : cur)), 2000)
                }
              }} />
              </>
              )}
              {/* 虚拟滚动容器：只挂载可视区域附近的条目，条目绝对定位 + translateY 排列；
                  height 为全部条目累计高度，滚动容器据此产生真实滚动条；measureElement 动态测量并缓存 */}
              <div style={{ height: virtualizer.getTotalSize(), position: 'relative' }}>
                {virtualizer.getVirtualItems().map((vi) => {
                  const item = virtualItems[vi.index]
                  return (
                    <div
                      key={vi.key}
                      ref={virtualizer.measureElement}
                      data-index={vi.index}
                      data-vkey={item.key}
                      style={{
                        position: 'absolute',
                        top: 0,
                        left: 0,
                        width: '100%',
                        transform: `translateY(${vi.start}px)`,
                        paddingBottom: 24,
                      }}
                    >
                      {item.kind === 'divider' && (
                        <div className="group-label">
                          <span>{item.label}</span>
                        </div>
                      )}
                      {item.kind === 'tools' && (
                        <ToolRunGroup runs={item.runs} onRetry={retryTool} onCancel={cancelToolRun} />
                      )}
                      {item.kind === 'msg' && (
                        <MessageItem
                          message={item.message}
                          time={formatTime(item.message.created_at)}
                          userMessageId={item.userMessageId}
                          isLastAssistant={ item.message.role === 'assistant' && lastAssistantId === item.message.id }
                          onRegenerate={regenerateLatest}
                          onBranch={branchFrom}
                          onRate={rateMessage}
                          onDislike={openFeedbackDialog}
                          onOpenVersions={openVersionDialog}
                          onSpeak={toggleSpeak}
                          speaking={speakingId === item.message.id}
                          onEditMessage={setEditTarget}
                          onDeleteMessage={handleDeleteMessage}
                          confirmDeleteMsgId={confirmDeleteMsgId}
                          projectPath={convRoot ?? currentProject?.path}
                          highlighted={highlightMsgId === item.message.id}
                          onOpenFile={openCodeFile}
                          onQuoteMessage={quoteMessage}
                          onLocateMessage={locateQuotedMessage}
                          onForkFrom={forkFromHere}
                          onCopyId={copyId}
                          shortId={shortId}
                          copiedId={copiedId}
                        />
                      )}
                      {/* 尾部动态区：流式消息/计划卡/工具徽章/任务摘要等（内容变化由 ResizeObserver 重新测量高度） */}
                      {item.kind === 'tail' && (
                        <>
                          {/* 任务过程徽章（ChatGPT 式）：中间所有过程折叠为“已处理 N 个操作中”，点击展开明细，对话流不中断 */}
                          {isStreaming && currentConversation && (
                            <RunningTaskOpsBadge
                              conversationId={currentConversation.id}
                              count={toolRuns.length + agentRuns.length}
                              toolName={
                                toolRuns.find((r) => r.status === 'running')?.tool
                              }
                              open={opsOpen}
                              onToggle={toggleOps}
                              runs={toolRuns}
                              agents={agentRuns}
                            />
                          )}
                          {/* 任务进度清单（计划卡）：工具联动推进，任务结束后保留展示 */}
                          {plan && <PlanCard plan={plan} />}
                          {/* 任务清单（todo_write 工具，可收起，实时进度）：内嵌消息流，任务结束后保留展示，避免小窗口悬浮遮挡 */}
                          {todos.length > 0 && (() => {
                            const done = todos.filter((t) => t.status === 'done').length
                            const pct = Math.round((done / todos.length) * 100)
                            return (
                              <div className="overflow-hidden animate-fade-in-up">
                                <button
                                  type="button"
                                  onClick={() => setTodoOpen((v) => !v)}
                                  aria-expanded={todoOpen}
                                  className="w-full flex items-center gap-2 py-1.5 text-left hover:opacity-80 transition-opacity"
                                >
                                  <Icon name="check" size={11} className="text-[var(--accent)] shrink-0" />
                                  <span className="text-[12px] text-[var(--text-secondary)]">{t('home.todoTitle')}</span>
                                  <span className="text-[11px] text-[var(--text-muted)] tabular-nums">{done}/{todos.length}</span>
                                  <div className="ml-auto h-1 w-12 rounded-full bg-[var(--bg-hover)] overflow-hidden">
                                    <div className="h-full rounded-full bg-[var(--accent)] transition-all" style={{ width: `${pct}%` }} />
                                  </div>
                                  <Icon name="chevron-right" size={11} className={`text-[var(--text-muted)] transition-transform shrink-0 ${todoOpen ? 'rotate-90' : ''}`} />
                                </button>
                                {todoOpen && (
                                  <div className="border-t border-[var(--border)]/60 py-1.5 space-y-0.5">
                                    {todos.map((td) => (
                                      <div key={td.id} className="flex items-start gap-2 py-0.5 text-[12px] leading-relaxed">
                                        {td.status === 'done' ? (
                                          <Icon name="check" size={11} className="text-[var(--success)] shrink-0 mt-0.5" />
                                        ) : td.status === 'in_progress' ? (
                                          <Spinner variant="inline" size={10} className="mt-0.5" />
                                        ) : (
                                          <span className="w-2.5 h-2.5 mt-1 rounded-full border border-[var(--border)] shrink-0" />
                                        )}
                                        <span
                                          className={
                                            td.status === 'done'
                                              ? 'text-[var(--text-muted)] line-through'
                                              : td.status === 'in_progress'
                                                ? 'text-[var(--accent)] font-medium'
                                                : 'text-[var(--text-secondary)]'
                                          }
                                        >
                                          {td.content}
                                        </span>
                                      </div>
                                    ))}
                                  </div>
                                )}
                              </div>
                            )
                          })()}
                          {/* 任务账本（Ledger 协议）：目标/已验证/待解决/下一步，每轮实时刷新；任务中断后保留展示 */}
                          {ledgerCard && ledgerCard.ledger && <LedgerCard state={ledgerCard} />}
                          {/* 任务过程回看（ChatGPT 式）：完成后“已处理 N 个操作”徽章，点击展开全部过程 */}
                          {!isStreaming && (toolRuns.length > 0 || agentRuns.length > 0) && (
                            <TaskOpsBadge
                              count={toolRuns.length + agentRuns.length}
                              open={opsOpen}
                              onToggle={toggleOps}
                              runs={toolRuns}
                              agents={agentRuns}
                            />
                          )}
                          {/* 任务收尾摘要：明确区分正常完成与达到上限/被停止后的未完成状态。 */}
                          {lastTaskSummary && !isStreaming && (
                            <div
                              className={`md-task-summary animate-fade-in-up ${lastTaskSummary.status === 'incomplete' ? 'is-incomplete' : ''}`}
                            >
                              <div className="md-task-summary-icon">
                                <Icon name={lastTaskSummary.status === 'incomplete' ? 'info' : 'check'} size={13} white />
                              </div>
                              <div className="min-w-0">
                                <span className="md-task-summary-title">
                                  {t(lastTaskSummary.status === 'incomplete' ? 'home.taskIncompleteTitle' : 'home.taskDoneTitle')}
                                </span>
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
                          <StreamingOutput conversationId={currentConversation?.id ?? null} speed={streamSpeed} />
                          <SilentStreamHint
                            conversationId={currentConversation?.id ?? null}
                            active={isStreaming && !toolRuns.some((r) => r.status === 'running') && !pendingPlan && !askCard && toolApprovals.length === 0}
                          />
                          {streamingError && (
                            <ErrorCard
                              error={streamingError}
                              detail={streamingErrorDetail}
                              onRetry={() => regenerateLast(modelOptions)}
                              retryLabel={t('home.retry')}
                            />
                          )}
                          {/* 中断回复恢复横幅：最后一条 user 消息已提交但回复从未入库，或
                              最后一条 assistant 是占位消息（duration_ms=NULL，部分内容已保存）——
                              均为任务中断所致，一键重新生成/继续生成回复 */}
                          {orphanUserMessage && (
                            <div className="flex items-center gap-3 rounded-xl border border-dashed border-[var(--warning)]/50 bg-[var(--warning)]/8 px-3.5 py-2.5 animate-fade-in-up">
                              <Icon name="refresh" size={14} className="shrink-0 text-[var(--warning)]" />
                              <div className="min-w-0 flex-1">
                                <div className="text-[12px] font-semibold text-[var(--text-secondary)]">
                                  {orphanUserMessage.role === 'assistant'
                                    ? t('home.interruptedBannerPartial')
                                    : t('home.interruptedBanner')}
                                </div>
                                <div className="text-[11px] text-[var(--text-muted)] leading-relaxed">
                                  {orphanUserMessage.role === 'assistant'
                                    ? t('home.interruptedBannerPartialDesc')
                                    : t('home.interruptedBannerDesc')}
                                </div>
                              </div>
                              <Button
                                variant="primary"
                                size="sm"
                                icon="refresh"
                                onClick={() => void regenerateLatest()}
                              >
                                {t('home.resumeGenerate')}
                              </Button>
                            </div>
                          )}
                          <div ref={bottomRef} />
                        </>
                      )}
                    </div>
                  )
                })}
              </div>
            </div>
          )}
          {/* 回到底部：流式中或用户上滑时显示 */}
          {showScrollBottom && (
            <button
              onClick={() => scrollToBottom(true)}
              className="sticky bottom-2 left-1/2 -translate-x-1/2 flex items-center gap-1.5 pl-3 pr-2 h-8 rounded-full glass-card text-xs text-[var(--text-secondary)] hover:text-[var(--accent)] hover:border-[color-mix(in_srgb,var(--accent)_50%,var(--border))] transition-all z-10 animate-fade-in-up tnum"
            >
              <Icon name="chevron-right" size={13} className="rotate-90" />
              {isStreaming ? t('home.scrollToLatest') : t('home.scrollBottom')}
              {unreadCount > 0 && (
                <span className="ml-0.5 min-w-[18px] h-[18px] px-1 flex items-center justify-center rounded-full btn-primary text-[10px] font-semibold leading-none shadow-[0_0_0_3px_var(--accent-soft)]">
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
            {isStreaming && streamingRecoveryParentRunId && (
              <span
                className="flex items-center gap-1 px-1.5 py-px rounded-full bg-[var(--accent-soft)] text-[var(--accent)] text-[11px]"
                title={streamingRecoveryParentRunId}
              >
                <Icon name="refresh" size={10} />
                {streamingRecoveryVerificationTotal
                  ? t('home.recoveryVerifying', {
                      verified: streamingRecoveryVerificationVerified ?? 0,
                      total: streamingRecoveryVerificationTotal,
                    })
                  : t('home.recoveryRunning')}
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
                {ctxInfo.context_v2 && (
                  <span className="relative inline-flex">
                    <button
                      type="button"
                      className="px-1.5 py-px rounded-full bg-[var(--accent)]/10 text-[var(--accent)] hover:bg-[var(--accent)]/15"
                      title={t('home.ctxV2Title', {
                        facts: ctxInfo.context_v2.fact_count,
                        artifacts: ctxInfo.context_v2.artifact_count,
                        invalidations: ctxInfo.context_v2.invalidation_epoch,
                        state: ctxInfo.context_v2.task_state || '-',
                        phase: ctxInfo.context_v2.task_phase || '-',
                      })}
                      onClick={() => setCtxV2Open((open) => !open)}
                    >
                      {t('home.ctxV2Badge', { count: ctxInfo.context_v2.fact_count })}
                    </button>
                    {ctxV2Open && ctxV2Detail && (
                      <span className="absolute bottom-full left-0 z-50 mb-2 block w-96 max-h-96 overflow-auto rounded-lg border border-[var(--border)] bg-[var(--bg-elevated)] p-3 text-[11px] text-[var(--text-secondary)] shadow-xl">
                        <span className="mb-2 block font-medium text-[var(--text-primary)]">
                          {t('home.ctxV2PanelTitle')}
                        </span>
                        {ctxV2Detail.task.goal && (
                          <span className="mb-2 block text-[var(--text-primary)]">
                            {t('home.ctxV2Goal', {
                              goal: ctxV2Detail.task.goal,
                              state: ctxV2Detail.task.state || '-',
                              phase: ctxV2Detail.task.phase || '-',
                            })}
                          </span>
                        )}
                        <span className="mb-2 block rounded-md border border-[var(--border)] p-2">
                          <span className="mb-1 block font-medium text-[var(--text-primary)]">
                            {t('home.ctxPinsTitle', { count: ctxV2Detail.pins.length })}
                          </span>
                          {ctxV2Detail.pins.map((pin) => (
                            <span key={pin.id} className="mb-1 flex items-start gap-1 rounded bg-[var(--bg-hover)] px-1.5 py-1">
                              <Icon name="pin" size={10} className="mt-0.5 shrink-0" />
                              <span className="min-w-0 flex-1 break-all">[{pin.pin_kind}] {pin.label}: {pin.content}</span>
                              <button
                                type="button"
                                className="shrink-0 text-[var(--danger)]"
                                onClick={() => void changeContextPin(pin.pin_kind, pin.source_ref, pin.label, pin.content, false)}
                              >
                                {t('home.ctxPinRemove')}
                              </button>
                            </span>
                          ))}
                          {ctxV2Detail.task.required_conditions.slice(0, 5).map((condition, index) => {
                            const sourceRef = `acceptance:${index}:${condition.slice(0, 40)}`
                            const pinned = ctxV2Detail.pins.some((pin) => pin.pin_kind === 'acceptance' && pin.source_ref === sourceRef)
                            return (
                              <button
                                key={sourceRef}
                                type="button"
                                className="mr-1 mt-1 rounded bg-[var(--accent)]/10 px-1.5 py-0.5 text-[var(--accent)]"
                                onClick={() => void changeContextPin('acceptance', sourceRef, t('home.ctxPinAcceptance'), condition, !pinned)}
                              >
                                {pinned ? '✓ ' : '+ '}{condition}
                              </button>
                            )
                          })}
                          {ctxV2Detail.hot.active_files.slice(0, 5).map((file) => {
                            const sourceRef = `file:${file}`
                            const pinned = ctxV2Detail.pins.some((pin) => pin.pin_kind === 'file' && pin.source_ref === sourceRef)
                            return (
                              <button
                                key={sourceRef}
                                type="button"
                                className="mr-1 mt-1 rounded bg-[var(--accent)]/10 px-1.5 py-0.5 text-[var(--accent)]"
                                onClick={() => void changeContextPin('file', sourceRef, file, file, !pinned)}
                              >
                                {pinned ? '✓ ' : '+ '}{file}
                              </button>
                            )
                          })}
                          {ctxV2Detail.hot.recent_messages.slice(-4).map((message) => {
                            const pinned = ctxV2Detail.pins.some((pin) => pin.pin_kind === 'message' && pin.source_ref === message.source_ref)
                            return (
                              <button
                                key={message.source_ref}
                                type="button"
                                className="mt-1 block w-full truncate rounded bg-[var(--bg-hover)] px-1.5 py-1 text-left"
                                title={message.content}
                                onClick={() => void changeContextPin('message', message.source_ref, message.role, message.content, !pinned)}
                              >
                                {pinned ? '✓' : '+'} [{message.role}] {message.content}
                              </button>
                            )
                          })}
                          <span className="mt-2 flex gap-1">
                            <input
                              value={ctxDecisionDraft}
                              onChange={(event) => setCtxDecisionDraft(event.target.value)}
                              placeholder={t('home.ctxPinDecisionPlaceholder')}
                              className="min-w-0 flex-1 rounded border border-[var(--border)] bg-[var(--bg-primary)] px-1.5 py-1 outline-none"
                            />
                            <Button
                              variant="primary"
                              size="xs"
                              disabled={!ctxDecisionDraft.trim()}
                              onClick={() => {
                                const decision = ctxDecisionDraft.trim()
                                if (!decision) return
                                void changeContextPin('decision', `decision:${Date.now()}`, t('home.ctxPinDecision'), decision, true)
                                  .then(() => setCtxDecisionDraft(''))
                              }}
                            >
                              {t('home.ctxPinAdd')}
                            </Button>
                          </span>
                        </span>
                        <span className="mb-2 block">
                          {t('home.ctxV2Cursor', {
                            from: ctxV2Detail.summary_from_message_rowid,
                            to: ctxV2Detail.summary_to_message_rowid,
                            seq: ctxV2Detail.summary_event_seq,
                          })}
                        </span>
                        {ctxV2Detail.reconciliation.count > 0 && (
                          <span
                            className={`mb-2 block rounded px-2 py-1 ${
                              ctxV2Detail.reconciliation.latest_status === 'corrected'
                                ? 'bg-[var(--warning)]/10 text-[var(--warning)]'
                                : 'bg-[var(--success)]/10 text-[var(--success)]'
                            }`}
                          >
                            {t(
                              ctxV2Detail.reconciliation.latest_status === 'corrected'
                                ? 'home.ctxV2Reconciled'
                                : 'home.ctxV2Consistent',
                              { count: ctxV2Detail.reconciliation.count },
                            )}
                            {ctxV2Detail.reconciliation.latest_conflicts.length > 0 && (
                              <span className="mt-1 block break-all text-[10px]">
                                {ctxV2Detail.reconciliation.latest_conflicts.join(' · ')}
                              </span>
                            )}
                          </span>
                        )}
                        <span className="mb-2 block">
                          <span className="mb-1 flex items-center gap-1">
                            <span className="font-medium text-[var(--text-primary)]">
                              {t('home.ctxBudgetBarTitle')}
                            </span>
                            {ctxV2Detail.budget.profile && (
                              <span className="rounded bg-[var(--accent)]/10 px-1 py-px text-[10px] text-[var(--accent)]">
                                {ctxV2Detail.budget.profile}
                              </span>
                            )}
                            <span className="text-[var(--text-muted)]">
                              {t('home.ctxV2Invalidations', {
                                count: ctxV2Detail.invalidation_epoch,
                              })}
                            </span>
                          </span>
                          <span className="flex h-2 w-full overflow-hidden rounded-sm bg-[var(--bg-hover)]">
                            <span
                              className="bg-[var(--accent)]"
                              title={`system ${ctxV2Detail.budget.system_tokens.toLocaleString()}`}
                              style={{
                                width: `${(ctxV2Detail.budget.system_tokens / Math.max(ctxV2Detail.budget.total_tokens, 1)) * 100}%`,
                              }}
                            />
                            <span
                              className="bg-[var(--success)]"
                              title={`task ${ctxV2Detail.budget.task_tokens.toLocaleString()}`}
                              style={{
                                width: `${(ctxV2Detail.budget.task_tokens / Math.max(ctxV2Detail.budget.total_tokens, 1)) * 100}%`,
                              }}
                            />
                            <span
                              className="bg-[var(--warning)]"
                              title={`project ${ctxV2Detail.budget.project_tokens.toLocaleString()}`}
                              style={{
                                width: `${(ctxV2Detail.budget.project_tokens / Math.max(ctxV2Detail.budget.total_tokens, 1)) * 100}%`,
                              }}
                            />
                            <span
                              className="bg-[var(--info)]"
                              title={`archive ${ctxV2Detail.budget.archive_tokens.toLocaleString()}`}
                              style={{
                                width: `${(ctxV2Detail.budget.archive_tokens / Math.max(ctxV2Detail.budget.total_tokens, 1)) * 100}%`,
                              }}
                            />
                            <span
                              className="bg-[var(--text-muted)]/40"
                              title={`hot ${ctxV2Detail.budget.hot_tokens.toLocaleString()}`}
                              style={{
                                width: `${(ctxV2Detail.budget.hot_tokens / Math.max(ctxV2Detail.budget.total_tokens, 1)) * 100}%`,
                              }}
                            />
                          </span>
                          <span className="mt-1 block text-[10px] text-[var(--text-muted)]">
                            {t('home.ctxV2Budget', {
                              hot: ctxV2Detail.budget.hot_tokens.toLocaleString(),
                              task: ctxV2Detail.budget.task_tokens.toLocaleString(),
                              project: ctxV2Detail.budget.project_tokens.toLocaleString(),
                              archive: ctxV2Detail.budget.archive_tokens.toLocaleString(),
                            })}
                          </span>
                        </span>
                        {ctxV2Detail.facts.slice(0, 8).map((fact) => (
                          <span key={fact.id} className="mb-1 block break-all">
                            <span className="text-[var(--text-primary)]">{fact.fact_kind}/{fact.fact_key}</span>
                            {' · '}{fact.source.kind}:{fact.source.reference} · v{fact.version}
                          </span>
                        ))}
                        {ctxV2Detail.artifacts.slice(0, 5).map((artifact) => (
                          <span key={artifact.id} className="mb-1 block break-all text-[var(--text-muted)]">
                            [{artifact.artifact_kind}] {artifact.uri} · {artifact.source_ref}
                          </span>
                        ))}
                        {ctxV2Detail.facts.length === 0 && ctxV2Detail.artifacts.length === 0 && (
                          <span className="block text-[var(--text-muted)]">{t('home.ctxV2Empty')}</span>
                        )}
                        {sessionHealth && (
                          <span
                            className={`mt-1 block rounded-md border p-2 ${
                              sessionHealth.degraded
                                ? 'border-[var(--warning)]/30 bg-[var(--warning)]/5'
                                : 'border-[var(--border)]'
                            }`}
                          >
                            <span className="mb-1 flex items-center gap-1 font-medium text-[var(--text-primary)]">
                              {t('home.ctxHealthTitle')}
                              {sessionHealth.degraded && (
                                <span className="rounded bg-[var(--warning)]/15 px-1 py-px text-[10px] text-[var(--warning)]">
                                  {t('home.ctxHealthDegraded')}
                                </span>
                              )}
                            </span>
                            <span className="block text-[10px] text-[var(--text-muted)]">
                              {t('home.ctxHealthMetrics', {
                                compress: sessionHealth.compress_count,
                                flip: Math.round(sessionHealth.fact_flip_rate * 100),
                                corrected: sessionHealth.corrected_count,
                                usage: Math.round(sessionHealth.budget_usage_ratio * 100),
                              })}
                            </span>
                            {sessionHealth.recent_invalidations.length > 0 && (
                              <span className="mt-1 block">
                                <span className="block text-[10px] font-medium text-[var(--text-primary)]">
                                  {t('home.ctxHealthInvalidations')}
                                </span>
                                {sessionHealth.recent_invalidations.map((item) => (
                                  <span
                                    key={`${item.fact_kind}/${item.fact_key}/${item.invalidated_at}`}
                                    className="block break-all text-[10px] text-[var(--text-muted)]"
                                  >
                                    · {item.fact_kind}/{item.fact_key} — {item.reason}
                                  </span>
                                ))}
                              </span>
                            )}
                            {sessionHealth.advice.length > 0 && (
                              <span className="mt-1 block space-y-0.5">
                                {sessionHealth.advice.map((item) => (
                                  <span key={item} className="block break-all text-[10px] text-[var(--warning)]">
                                    · {item}
                                  </span>
                                ))}
                              </span>
                            )}
                          </span>
                        )}
                      </span>
                    )}
                  </span>
                )}
                {/* token 预算进度条：估算占用 / 模型上下文窗口（>85% 触发自动压缩） */}
                {ctxInfo.context_limit > 0 && (() => {
                  const pct = Math.min(100, Math.round((ctxInfo.estimated_tokens / ctxInfo.context_limit) * 100))
                  const level = pct > 85 ? 'danger' : pct > 60 ? 'warn' : 'normal'
                  // 上下文"剩余消息数"预测：平均 user 消息约 200 token、assistant 平均 800 token
                  // 简单估算：剩余预算 ÷ 单轮平均 (200+800=1000) = 还能发几轮
                  const remaining = Math.max(0, ctxInfo.context_limit - ctxInfo.estimated_tokens)
                  const avgPerTurn = 1000
                  const turnsLeft = Math.floor(remaining / avgPerTurn)
                  return (
                    <span
                      className="flex items-center gap-1.5"
                      title={t('home.ctxBudgetTitle', {
                        tokens: ctxInfo.estimated_tokens.toLocaleString(),
                        limit: ctxInfo.context_limit.toLocaleString(),
                        pct,
                      })}
                    >
                      <span className="context-meter w-16 shrink-0" style={{ height: 2 }}>
                        <span
                          className="context-meter-fill"
                          data-level={level}
                          style={{ width: `${Math.max(pct, 2)}%` }}
                        />
                      </span>
                      <span className="tnum">{pct}%</span>
                      {/* 剩余轮数预测：<5 轮时变橙提醒，<2 变红 */}
                      {turnsLeft > 0 && turnsLeft <= 5 && (
                        <span
                          className={`tnum text-[10.5px] px-1 rounded ${
                            turnsLeft <= 2
                              ? 'text-[var(--danger)] bg-[var(--danger-50)]'
                              : 'text-[var(--warning)] bg-[var(--warning-50)]'
                          }`}
                          title={t('home.ctxTurnsLeftHint', { count: turnsLeft })}
                        >
                          ~{turnsLeft}{t('home.ctxTurnsLeftUnit')}
                        </span>
                      )}
                    </span>
                  )
                })()}
              </div>
            )}
            {unfinishedConv && unfinishedConv.conversationId === currentConversation?.id && !isStreaming && (
              <div className="ml-auto flex items-center gap-2 min-w-0">
                {unfinishedConv.recoveryPolicy === 'manual' && (
                  <span className="truncate text-[10.5px] text-[var(--warning)]" title={unfinishedConv.error || t('home.recoveryManualHint')}>
                    {t('home.recoveryManual')}
                  </span>
                )}
                {unfinishedConv.recoveryPolicy === 'verify_effects' && (
                  <span className="truncate text-[10.5px] text-[var(--warning)]" title={unfinishedConv.error || t('home.recoveryVerifyHint')}>
                    {t('home.recoveryVerify')}
                  </span>
                )}
                <button
                  onClick={() => {
                    const affected = unfinishedConv.recoverySteps
                      ?.slice(-12)
                      .map((step) => `- ${step.title} [${step.state}; ${step.verification_state}; ${step.recovery_policy}]`)
                      .join('\n')
                    const base = unfinishedConv.recoveryPolicy === 'manual'
                      ? t('home.recoveryManualPrompt')
                      : unfinishedConv.recoveryPolicy === 'verify_effects'
                        ? t('home.recoveryVerifyPrompt')
                        : t('home.continuePrompt')
                    void sendUserMessage(
                      affected ? `${base}\n\n${t('home.recoveryAffectedSteps')}:\n${affected}` : base,
                      unfinishedConv.runId
                        ? { ...modelOptions, resume_run_id: unfinishedConv.runId }
                        : modelOptions,
                    )
                  }}
                  className="shrink-0 flex items-center gap-1.5 h-6 px-2.5 rounded-full bg-[var(--accent-soft)] text-[var(--accent)] text-[11px] font-medium hover:brightness-110 transition-all"
                >
                  <Icon name="arrow-down" size={11} />
                  {unfinishedConv.recoveryPolicy === 'manual' || unfinishedConv.recoveryPolicy === 'verify_effects'
                    ? t('home.recoverSafely')
                    : t('home.continueTask')}
                </button>
              </div>
            )}
          </div>
          {/* 排队中消息条：运行中提交的消息，任务结束后续跑；支持单条移除 */}
          {queuedList.length > 0 && currentConversation && (
            <div className="max-w-3xl mx-auto pb-1.5">
              <button
                onClick={() => setQueuedOpen((v) => !v)}
                aria-expanded={queuedOpen}
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
                      className="flex items-center gap-2 rounded-lg modern-card border-[var(--border)] px-2.5 py-1.5"
                    >
                      <span className="text-[11px] text-[var(--text-primary)] truncate flex-1" title={q.content}>
                        {q.content}
                      </span>
                      {q.agent_owned && (
                        <span className="text-[9px] px-1 py-px rounded bg-[var(--accent)]/10 text-[var(--accent)] shrink-0">
                          {t('home.queuedAgentLabel')}
                        </span>
                      )}
                      <IconButton
                        icon="close"
                        label={t('home.queuedRemove')}
                        hoverTone="danger"
                        pad="xs"
                        iconSize={11}
                        onClick={() => removeQueued(q.id)}
                      />
                    </div>
                  ))}
                </div>
              )}
            </div>
          )}
          <div
            className="relative max-w-3xl mx-auto rounded-2xl border border-[var(--border)] bg-[var(--bg-secondary)] transition-all focus-within:border-[var(--accent)] focus-within:shadow-[0_0_0_3px_var(--accent-soft)]"
            onDragOver={(e) => {
              if (Array.from(e.dataTransfer.types).includes('Files')) {
                e.preventDefault()
                setDragActive(true)
              }
            }}
            onDragLeave={(e) => {
              // 子元素间移动也会触发 leave：仅当真正离开容器时取消高亮
              if (!e.currentTarget.contains(e.relatedTarget as Node)) setDragActive(false)
            }}
            onDrop={(e) => {
              e.preventDefault()
              setDragActive(false)
              // Tauri 环境 DOM 拿不到文件（须走 onDragDropEvent）；此处仅作浏览器开发模式回退
              if (e.dataTransfer.files.length > 0) void handleDroppedBrowserFiles(e.dataTransfer.files)
            }}
          >
            {/* 拖拽悬停覆盖层：提示松手添加（文件/图片；项目外文本自动插入输入框） */}
            {dragActive && (
              <div className="absolute -inset-px rounded-2xl border-2 border-dashed border-[var(--accent)] bg-[var(--bg-secondary)]/85 backdrop-blur-sm flex items-center justify-center gap-2 z-40 pointer-events-none">
                <Icon name="file" size={15} className="text-[var(--accent)]" />
                <span className="text-[12px] font-medium text-[var(--accent)]">{t('home.dropHint')}</span>
              </div>
            )}
            {/* 斜杠快捷指令面板（行首 / 后弹出） */}
            {slashCandidates && (
              <div className="absolute bottom-full left-3 right-3 mb-1.5 rounded-xl modern-card shadow-2xl shadow-black/40 py-1 z-50 max-h-72 overflow-y-auto animate-modal-in">
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
              <div className="absolute bottom-full left-3 right-3 mb-1.5 rounded-xl modern-card shadow-2xl shadow-black/40 py-1 z-50 max-h-60 overflow-y-auto animate-modal-in">
                <div className="px-3 py-1.5 text-[10px] font-medium text-[var(--text-muted)] border-b border-[var(--border)]">
                  {t('home.refHint')}「@{refQuery}」
                </div>
                {refCandidates.length === 0 && (
                  <div className="px-3 py-2.5 text-[11px] text-[var(--text-muted)]">{t('home.refNoMatch')}</div>
                )}
                {refCandidates.map((c, i) => {
                  const isConv = c.path.startsWith('conv:')
                  return (
                    <button
                      key={c.path}
                      onMouseEnter={() => setRefIdx(i)}
                      onClick={() => pickReference(c.path)}
                      className={`w-full flex items-center gap-2 px-3 py-1.5 text-left transition-colors ${
                        i === refIdx ? 'bg-[var(--bg-hover)]' : 'hover:bg-[var(--bg-hover)]'
                      }`}
                    >
                      <Icon name={isConv ? 'chat' : 'file'} size={12} className="text-[var(--text-muted)] shrink-0" />
                      <span className="text-[11.5px] text-[var(--text-primary)] truncate">{isConv ? `会话：${c.name}` : c.path}</span>
                    </button>
                  )
                })}
              </div>
            )}
            <textarea
              ref={inputRef}
              value={draft}
              onChange={(e) => handleDraftChange(e.target.value)}
              onKeyDown={(e) => {
                // IME 候选确认也会发出 Enter；组合输入阶段不处理快捷键，避免中文输入误发送。
                if (e.nativeEvent.isComposing || e.key === 'Process') return
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
                if (shouldSubmitComposerKey(e.key, e.shiftKey, e.nativeEvent.isComposing)) {
                  e.preventDefault()
                  void handleSend()
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
            {/* 生成媒体状态条（图片/视频/音频任务进行中；含停止按钮，复用 stopGeneration） */}
            {currentGen && (
              <div className="flex items-center gap-2 mx-3 mt-1.5 px-2.5 py-1.5 rounded-lg bg-[var(--accent-soft)] text-[11px] text-[var(--accent)]">
                <Icon name="spark" size={11} className="animate-pulse shrink-0" />
                <span className="truncate">
                  {t('home.genProgress', '正在生成{{kind}}', {
                    kind: t(`home.genKind.${currentGen.kind}`),
                  })}
                  {currentGen.kind === 'video' && currentGen.waitedSecs > 0 &&
                    t('home.genWaited', '（已等待 {{secs}} 秒）', { secs: Math.floor(currentGen.waitedSecs) })}
                </span>
                <button
                  onClick={() => {
                    setStopRequested(true)
                    void stopGeneration()
                  }}
                  className="ml-auto shrink-0 flex items-center gap-1 px-2 py-0.5 rounded-full bg-[var(--danger)]/12 text-[var(--danger)] hover:bg-[var(--danger)]/20 transition-colors"
                >
                  <Icon name="close" size={9} />
                  {t('home.stopGenerating')}
                </button>
              </div>
            )}
            {/* 消息引用条（Quote 后展示；× 仅移除 msg: 标记，正文引用行可手动编辑） */}
            {pendingQuote && (
              <div className="flex items-center gap-1.5 mx-3 mt-1 px-2 py-1 rounded-md bg-[var(--accent-soft)] text-[10.5px] text-[var(--accent)]">
                <Icon name="quote" size={10} className="shrink-0" />
                <button
                  onClick={() => locateQuotedMessage(pendingQuote.id)}
                  className="truncate hover:text-[var(--accent-hover)]"
                  title={t('home.locateQuoted')}
                >
                  {t('home.quotingLabel')}: {pendingQuote.preview}
                </button>
                <button
                  onClick={() => setPendingQuote(null)}
                  className="ml-auto shrink-0 hover:text-[var(--danger)] transition-colors"
                  title={t('home.removeQuote')}
                >
                  <Icon name="close" size={10} />
                </button>
              </div>
            )}
            {/* 引用标签（@ 选择后展示，可移除） */}
            {references.length > 0 && (
              <div className="flex flex-wrap gap-1 px-3 pt-1">
                {references.map((p) => {
                  const convTitle = p.startsWith('conv:')
                    ? (conversations.find((c) => c.id === p.slice(5))?.title ?? null)
                    : null
                  return (
                    <span
                      key={p}
                      className="flex items-center gap-1 px-2 py-0.5 rounded-md bg-[var(--accent-soft)] text-[10.5px] text-[var(--accent)] max-w-56"
                    >
                      <Icon name={convTitle ? 'chat' : 'file'} size={10} className="shrink-0" />
                      <span className="truncate" title={p}>
                        {convTitle
                          ? `会话：${convTitle}`
                          : // 绝对路径（拖拽加入）只显示文件名，完整路径放 title 悬浮
                            /^[a-zA-Z]:[\\/]/.test(p) || p.startsWith('/')
                            ? (p.split(/[\\/]/).pop() ?? p)
                            : p}
                      </span>
                      <button
                        onClick={() => setReferences((r) => r.filter((x) => x !== p))}
                        className="shrink-0 hover:text-[var(--danger)] transition-colors"
                        title={t('home.removeReference')}
                      >
                        <Icon name="close" size={10} />
                      </button>
                    </span>
                  )
                })}
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
                      className="absolute -top-1.5 -right-1.5 w-4 h-4 rounded-full modern-card border-[var(--border)] text-[var(--text-muted)] hover:text-[var(--danger)] flex items-center justify-center opacity-0 group-hover/img:opacity-100 transition-opacity"
                      title={t('home.removeImage')}
                    >
                      <Icon name="close" size={9} />
                    </button>
                  </div>
                ))}
              </div>
            )}
            {/* 工具栏：sm 以下允许换行（左侧状态+右侧按钮分两行），md+ 单行排布 */}
            <div className="flex flex-wrap items-center justify-between gap-y-1.5 px-3 pb-2.5 pt-1">
              <div className="flex items-center gap-1.5 min-w-0 flex-wrap">
                {/* 生成媒体：图片/视频/音频（走对应服务商生成模型） */}
                <div className="relative shrink-0" ref={genMenuRef}>
                  <button
                    onClick={() => setGenMenuOpen((v) => !v)}
                    title={t('home.generate')}
                    aria-label={t('home.generate')}
                    aria-expanded={genMenuOpen}
                    className={`flex items-center gap-1 pl-1.5 pr-2 py-1 rounded-lg text-[11px] transition-colors ${
                      genMenuOpen || genMode
                        ? 'text-[var(--accent)] bg-[var(--accent-soft)]'
                        : 'text-[var(--text-secondary)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)]'
                    }`}
                  >
                    <Icon name="spark" size={13} />
                    {genMode ? (
                      <span>{t(`home.genKind.${genMode}`)}</span>
                    ) : (
                      <Icon name="chevron-right" size={10} className="rotate-90 opacity-60" />
                    )}
                  </button>
                  {genMenuOpen && (
                    <div className="absolute left-0 bottom-full mb-1.5 rounded-xl modern-card shadow-2xl shadow-black/40 py-1 z-50 w-40 animate-modal-in">
                      {GEN_ITEMS.map((item) => (
                        <button
                          key={item.kind}
                          onClick={() => {
                            setGenMode(item.kind)
                            setGenMenuOpen(false)
                            inputRef.current?.focus()
                          }}
                          className={`w-full flex items-center gap-2.5 px-3 py-2 text-[12px] transition-colors ${
                            genMode === item.kind
                              ? 'text-[var(--accent)] bg-[var(--accent-soft)]'
                              : 'text-[var(--text-secondary)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)]'
                          }`}
                        >
                          <Icon name={item.icon} size={14} className="shrink-0" />
                          {t(`home.genKind.${item.kind}`)}
                          {genMode === item.kind && <Icon name="check" size={12} className="ml-auto shrink-0" />}
                        </button>
                      ))}
                      {genMode && (
                        <>
                          <div className="h-px bg-[var(--border)] mx-2 my-0.5" />
                          <button
                            onClick={() => {
                              setGenMode(null)
                              setGenMenuOpen(false)
                            }}
                            className="w-full flex items-center gap-2.5 px-3 py-2 text-[12px] text-[var(--text-muted)] hover:bg-[var(--bg-hover)] transition-colors"
                          >
                            <Icon name="close" size={14} className="shrink-0" />
                            {t('home.genCancel')}
                          </button>
                        </>
                      )}
                    </div>
                  )}
                </div>
                {/* Rules 编辑：全局指令 + 项目级 rules（注入 system_prompt） */}
                <IconButton
                  icon="settings"
                  label={t('home.rules')}
                  pad="md"
                  iconSize={13}
                  onClick={() => void openRulesDialog()}
                />
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
                    <span className="max-w-24 md:max-w-32 truncate">{currentModelLabel}</span>
                    <Icon name="chevron-right" size={10} className="rotate-90 opacity-60" />
                  </button>
                  {showModelSettings && (
                    <ModelSettingsPopover catalog={modelCatalog} options={modelOptions} onChange={updateModelOptions} />
                  )}
                </div>
                {/* Web 预览已移至右侧栏 Preview 面板 */}
                {isStreaming && (
                  <span
                    className="flex items-center gap-1.5 text-[11px] text-[var(--text-secondary)] shrink-0"
                    title={t('home.queuedHint')}
                  >
                    <span className="w-1.5 h-1.5 rounded-full bg-[var(--accent)] animate-pulse shrink-0" />
                    {/* 状态文字：xl 以下折叠成"工作中"三字，xl+ 才显示完整 "Agent 正在工作..." */}
                    <span className="inline xl:hidden whitespace-nowrap">{t('home.working')}</span>
                    <span className="hidden xl:inline whitespace-nowrap">{t('home.agentWorking')}</span>
                  </span>
                )}
                {/* 快捷键提示：仅 2xl 显示完整说明，避免窄屏挤压 */}
                <span className="hidden 2xl:inline whitespace-nowrap text-[11px] text-[var(--text-muted)] shrink-0">
                  {isStreaming
                    ? t('home.queuedHint')
                    : `Enter ${t('home.send')} · Shift+Enter ${t('home.newline')}`}
                </span>
              </div>
              {/* 右侧操作按钮：sm 以下也允许换行（避免与左侧状态挤一行） */}
              <div className="flex items-center gap-1.5 flex-wrap justify-end">
                {!isStreaming && (
                  <span className="hidden sm:inline text-[11px] tabular-nums text-[var(--text-muted)]">{draft.length}</span>
                )}
                {isStreaming ? (
                  <>
                    {/* 停止当前工具：xl 以下折叠到更多菜单，xl+ 展开（危险操作，但仅在工具运行时显示，频次低） */}
                    {toolRuns.some((r) => r.status === 'running') && (
                      <button
                        onClick={() => stopCurrentTool()}
                        className="hidden xl:flex h-8 xl:px-3 rounded-full bg-[var(--warning)]/12 text-[var(--warning)] items-center justify-start gap-1.5 hover:bg-[var(--warning)]/20 active:scale-95 transition-all text-[12px] font-medium"
                        title={t('home.stopTool')}
                      >
                        <Icon name="bolt" size={12} className="shrink-0" />
                        <span>{t('home.stopTool')}</span>
                      </button>
                    )}
                    {/* 流式速度切换：xl 以下折叠到更多菜单，xl+ 展开 */}
                    <button
                      onClick={() => {
                        const cycle = [1, 2, 4, 0.5]
                        const next = cycle[(cycle.indexOf(streamSpeed) + 1) % cycle.length]
                        setStreamSpeed(next)
                      }}
                      className="hidden xl:flex h-8 xl:px-3 rounded-full bg-[var(--bg-card)] text-[var(--text-secondary)] items-center gap-1 hover:bg-[var(--bg-hover)] active:scale-95 transition-all text-[12px] font-mono font-medium tnum"
                      title={t('home.streamSpeedHint')}
                    >
                      {streamSpeed}x
                    </button>
                    {/* 停止生成：危险操作，xl 以下也常驻（用户必须能随时停） */}
                    <button
                      onClick={() => {
                        setStopRequested(true)
                        void stopGeneration()
                        useAuditStore.getState().log({
                          category: 'task.stop',
                          label: t('home.auditLabelTaskStop'),
                          detail: currentConversation?.title,
                          conversationId: currentConversation?.id,
                          projectId: currentProject?.id,
                        })
                      }}
                      disabled={stopRequested}
                      aria-label={t(stopRequested ? 'home.stopping' : 'home.stopGenerating')}
                      className="h-8 px-2.5 md:px-3 rounded-full bg-[var(--danger)]/12 text-[var(--danger)] flex items-center gap-1.5 hover:bg-[var(--danger)]/20 active:scale-95 disabled:opacity-60 disabled:cursor-wait transition-all text-[12px] font-medium"
                      title={t(stopRequested ? 'home.stopping' : 'home.stopGenerating')}
                    >
                      <span className={`w-2.5 h-2.5 rounded-[3px] bg-[var(--danger)] shrink-0 ${stopRequested ? 'animate-pulse' : ''}`} />
                      <span className="hidden md:inline">{t(stopRequested ? 'home.stopping' : 'home.stopGenerating')}</span>
                    </button>
                    {/* 发给 Agent：折叠到更多菜单，xl+ 展开 */}
                    <button
                      onClick={handleSendToAgent}
                      disabled={!draft.trim() || stopRequested}
                      className="hidden xl:flex h-8 xl:px-3 rounded-full bg-[var(--accent)]/12 text-[var(--accent)] items-center justify-start gap-1.5 hover:bg-[var(--accent)]/20 active:scale-95 disabled:opacity-35 disabled:cursor-not-allowed transition-all text-[12px] font-medium"
                      title={t('home.sendToAgent')}
                    >
                      <Icon name="bolt" size={12} className="shrink-0" />
                      <span>{t('home.sendToAgent')}</span>
                    </button>
                    {/* 发送：主操作，xl 以下也常驻 */}
                    <button
                      onClick={handleSend}
                      disabled={!draft.trim() || stopRequested || !!currentGen}
                      aria-label={t('home.queueSend')}
                      className="w-8 h-8 rounded-full text-white flex items-center justify-center active:scale-95 disabled:opacity-35 disabled:cursor-not-allowed transition-all shadow-lg shadow-[var(--accent)]/30 bg-[var(--accent-600)] hover:bg-[var(--accent-500)] hover:shadow-[0_4px_16px_var(--accent-glow)]"
                      title={t('home.queueSend')}
                    >
                      <Icon name="send" size={14} white />
                    </button>
                    {/* 批量任务：折叠到更多菜单，xl+ 展开 */}
                    <button
                      onClick={() => setBatchOpen(true)}
                      disabled={!draft.trim() || stopRequested}
                      className="hidden xl:flex h-8 xl:px-2.5 rounded-full bg-[var(--bg-card)] text-[var(--text-secondary)] items-center justify-start gap-1 hover:bg-[var(--bg-hover)] active:scale-95 disabled:opacity-35 disabled:cursor-not-allowed transition-all text-[12px] font-medium"
                      title={t('home.batchSend')}
                    >
                      <Icon name="package" size={12} />
                      <span>{t('home.batch')}</span>
                    </button>
                    {/* 工具栏"更多"菜单：xl 以下显示，xl+ 隐藏（其他次要按钮已展开） */}
                    <div ref={toolbarMoreRef} className="relative xl:hidden">
                      <button
                        onClick={() => setToolbarMoreOpen((v) => !v)}
                        title={t('home.moreActions')}
                        aria-label={t('home.moreActions')}
                        aria-expanded={toolbarMoreOpen}
                        className={`h-8 w-8 rounded-full flex items-center justify-center transition-colors ${
                          toolbarMoreOpen
                            ? 'text-[var(--accent)] bg-[var(--accent-soft)]'
                            : 'text-[var(--text-secondary)] bg-[var(--bg-card)] hover:bg-[var(--bg-hover)]'
                        }`}
                      >
                        <Icon name="more-vert" size={14} />
                      </button>
                      {toolbarMoreOpen && (
                        <div className="absolute right-0 bottom-full mb-1.5 rounded-xl modern-card shadow-2xl shadow-black/40 py-1 z-50 w-44 animate-modal-in">
                          {/* 停止当前工具：仅在工具运行时显示 */}
                          {toolRuns.some((r) => r.status === 'running') && (
                            <button
                              onClick={() => {
                                stopCurrentTool()
                                setToolbarMoreOpen(false)
                              }}
                              className="w-full flex items-center gap-2.5 px-3 py-2 text-[12px] text-[var(--warning)] hover:bg-[var(--bg-hover)] transition-colors"
                            >
                              <Icon name="bolt" size={14} className="shrink-0" />
                              {t('home.stopTool')}
                            </button>
                          )}
                          {/* 流式速度：弹层里做成横向 chip 选择 */}
                          <div className="px-3 py-2">
                            <div className="text-[10px] text-[var(--text-muted)] mb-1.5 flex items-center gap-1.5">
                              <Icon name="bolt" size={11} />
                              {t('home.streamSpeedHint')}
                            </div>
                            <div className="flex items-center gap-1">
                              {[0.5, 1, 2, 4].map((s) => (
                                <button
                                  key={s}
                                  onClick={() => {
                                    setStreamSpeed(s)
                                    setToolbarMoreOpen(false)
                                  }}
                                  className={`flex-1 h-7 rounded-md text-[11px] font-mono font-medium tnum transition-colors ${
                                    streamSpeed === s
                                      ? 'bg-[var(--accent)] text-white'
                                      : 'bg-[var(--bg-hover)] text-[var(--text-secondary)] hover:text-[var(--text-primary)]'
                                  }`}
                                >
                                  {s}x
                                </button>
                              ))}
                            </div>
                          </div>
                          <div className="h-px bg-[var(--border)] mx-2 my-0.5" />
                          {/* 发给 Agent */}
                          <button
                            onClick={() => {
                              if (draft.trim()) {
                                handleSendToAgent()
                                setToolbarMoreOpen(false)
                              }
                            }}
                            disabled={!draft.trim() || stopRequested}
                            className="w-full flex items-center gap-2.5 px-3 py-2 text-[12px] text-[var(--accent)] hover:bg-[var(--bg-hover)] transition-colors disabled:opacity-40 disabled:cursor-not-allowed"
                          >
                            <Icon name="bolt" size={14} className="shrink-0" />
                            {t('home.sendToAgent')}
                          </button>
                          {/* 批量任务 */}
                          <button
                            onClick={() => {
                              setBatchOpen(true)
                              setToolbarMoreOpen(false)
                            }}
                            disabled={!draft.trim() || stopRequested}
                            className="w-full flex items-center gap-2.5 px-3 py-2 text-[12px] text-[var(--text-secondary)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] transition-colors disabled:opacity-40 disabled:cursor-not-allowed"
                          >
                            <Icon name="package" size={14} className="shrink-0" />
                            {t('home.batchSend')}
                          </button>
                        </div>
                      )}
                    </div>
                  </>
                ) : (
                  <>
                    {/* 生成媒体模式 chip（可点击取消，回到普通对话发送） */}
                    {genMode && !currentGen && (
                      <button
                        onClick={() => setGenMode(null)}
                        title={t('home.genCancelHint', '取消生成模式')}
                        className="h-8 px-2.5 rounded-full bg-[var(--accent)]/12 text-[var(--accent)] flex items-center gap-1.5 hover:bg-[var(--accent)]/20 active:scale-95 transition-all text-[12px] font-medium"
                      >
                        <Icon name={GEN_ITEMS.find((g) => g.kind === genMode)?.icon ?? 'spark'} size={12} className="shrink-0" />
                        <span className="hidden sm:inline">{t(`home.genKind.${genMode}`)}</span>
                        <Icon name="close" size={10} />
                      </button>
                    )}
                    <button
                      onClick={handleSend}
                      disabled={!draft.trim() || !currentProject || !!currentGen}
                      aria-label={t('home.send')}
                      className="w-8 h-8 rounded-full text-white flex items-center justify-center active:scale-95 disabled:opacity-35 disabled:cursor-not-allowed transition-all shadow-lg shadow-[var(--accent)]/30 bg-[var(--accent-600)] hover:bg-[var(--accent-500)] hover:shadow-[0_4px_16px_var(--accent-glow)]"
                      title={t('home.send')}
                    >
                      <Icon name="send" size={14} white />
                    </button>
                  </>
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
              { key: 'shell', icon: 'terminal', label: t('home.terminal') },
              { key: 'terminal', icon: 'receipt', label: t('home.terminalLogs') },
              { key: 'timeline', icon: 'archive', label: t('home.timeline') },
            ]
            return (
              <div
                className={`right-tabbar flex flex-wrap items-center gap-1 border-b border-[var(--border)] shrink-0 ${
                  rightCompact ? 'px-2 py-1.5' : 'px-3 py-1.5'
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
                          : 'text-[var(--text-secondary)] hover:bg-[var(--bg-hover)] hover:text-[var(--text-primary)]'
                      } px-3.5`}
                    >
                      <Icon name={tb.icon} size={15} />
                      <span className="whitespace-nowrap">{tb.label}</span>
                    </button>
                  )
                })}
              </div>
            )
          })()}

          {/* Tab 内容 */}
          <div className="flex-1 overflow-y-auto">
            <Suspense
              fallback={(
                <div className="h-full min-h-40 flex items-center justify-center" role="status" aria-live="polite">
                  <span className="w-6 h-6 rounded-full border-2 border-[var(--border)] border-t-[var(--accent)] animate-spin" />
                </div>
              )}
            >
            {rightTab === 'overview' ? (
              <div className="p-3 space-y-2.5">
                {/* 项目信息卡 */}
                <div className="rounded-xl modern-card p-3.5">
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
                    {convRoot && (
                      <OverviewRow icon="folder" label={t('home.worktreePath')} value={convRoot} mono />
                    )}
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
                      onClick={handleOpenShell}
                      className="w-full flex items-center justify-center gap-1.5 mt-1 px-2.5 py-2 rounded-lg text-[11px] text-[var(--text-secondary)] bg-[var(--bg-hover)]/60 border border-[var(--border)] hover:text-[var(--accent)] hover:border-[var(--accent)]/40 transition-colors"
                    >
                      <Icon name="terminal" size={12} />
                      {t('home.openShell')}
                    </button>
                  </div>
                </div>

                {/* Git 变更摘要：当前工作区状态入口（点击跳转 Git 面板） */}
                <OverviewGitSummary
                  projectPath={convRoot ?? currentProject.path}
                  branches={gitBranches?.branches ?? null}
                  onSwitchBranch={switchBranch}
                  onOpenGit={() => setRightTab('git')}
                />

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
                  const workBase = (convRoot ?? currentProject.path).replace(/[\\/]+$/, '')
                  const mainRel = (() => {
                    if (!mainRootAbs || !workBase || mainRootAbs === workBase) return null
                    const norm = mainRootAbs.replace(/\\/g, '/')
                    const normBase = workBase.replace(/\\/g, '/')
                    return norm.startsWith(normBase + '/') ? norm.slice(normBase.length + 1) : null
                  })()
                  const openInExplorer = (rel: string) => {
                    void shellOpen((workBase + '/' + rel).replace(/\\/g, '/')).catch(() => {})
                  }
                  return (
                    <div className="rounded-xl modern-card p-3">
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
                              className="px-2.5 h-7 rounded-md text-[11px] font-medium btn-primary  disabled:opacity-50"
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
                <div className="rounded-xl modern-card p-3">
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
                key={`${currentProject.id}:${convRoot ?? ''}`}
                tree={fileTree}
                building={indexBuilding}
                projectId={currentProject.id}
                projectPath={convRoot ?? currentProject.path}
                root={convRoot}
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
              <GitPanel
                project={currentProject}
                sessionWorktree={convRoot ? { path: convRoot, branch: currentConversation?.worktree_branch ?? null } : null}
                onNewWorktreeConversation={(wt) => newConversation({ work_mode: 'worktree', worktree_path: wt.path, worktree_branch: wt.branch })}
              />
            ) : rightTab === 'preview' ? (
              <PreviewPanel
                url={previewUrl}
                setUrl={setPreviewUrl}
                src={previewSrc}
                onOpen={handleOpenPreview}
              />
            ) : rightTab === 'devices' ? (
              <DevicesPanel
                key={currentProject.id}
                projectId={currentProject?.id}
                projectName={currentProject?.name}
                onChanged={() => {}}
                onSendImage={(url) => {
                  // 设备截图原图可能很大（PNG 未压缩），发送前走统一压缩，控制 token 与供应商请求体限制
                  void compressImage(url).then((u) => setPickedImages((cur) => (cur.length >= 4 ? cur : [...cur, u])))
                }}
              />
            ) : rightTab === 'shell' ? (
              <ShellPanel key={`${currentProject.id}:${convRoot ?? ''}`} projectId={currentProject.id} projectPath={convRoot ?? currentProject.path} />
            ) : rightTab === 'analyze' ? (
              <AnalyzePanel
                key={`${currentProject.id}:${convRoot ?? ''}`}
                projectPath={convRoot ?? currentProject.path}
                projectId={currentProject.id}
                projectName={currentProject.name}
                root={convRoot}
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
                key={`${currentProject.id}:${convRoot ?? ''}`}
                projectId={currentProject.id}
                projectName={currentProject.name}
                root={convRoot}
                onReference={handleReference}
              />
            ) : rightTab === 'stats' ? (
              <ToolStatsPanel
                stats={toolStats}
                tokenStats={toolTokenStats}
                toolGroupMap={toolGroupMap}
                onRefresh={() => {
                  void loadToolStats()
                  void loadToolTokenStats()
                }}
              />
            ) : rightTab === 'timeline' ? (
              <TimelinePanel conversationId={convId ?? null} refreshTick={timelineTick} />
            ) : (
              <TerminalPanel
                entries={terminalEntries}
                onClear={clearTerminal}
                buildLogs={buildLogs}
                onClearBuild={clearBuildLogs}
              />
            )}
            </Suspense>
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
        <div className="fixed bottom-24 left-1/2 -translate-x-1/2 z-[var(--app-z-overlay)] w-[640px] max-w-[calc(100vw-2rem)]">
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
                {planEditing ? t('home.planEditDone') : t('home.planEdit')}
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
              <Button
                variant="primary"
                size="md"
                icon="check"
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
              >
                {t('home.planApprove')}
              </Button>
            </div>
          </div>
        </div>
      )}

      {/* ============ 已批准任务计划（执行中锚点，可收起） ============ */}
      {approvedPlan && approvedPlan.conversationId === currentConversation?.id && (
        <div className="fixed bottom-24 left-1/2 -translate-x-1/2 z-[var(--app-z-overlay)] w-[640px] max-w-[calc(100vw-2rem)]">
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
          <div key={card.id} className={`fixed bottom-24 right-4 z-[var(--app-z-overlay)] w-[320px] rounded-xl border bg-[var(--bg-elevated)]/95 backdrop-blur shadow-lg shadow-black/10 animate-modal-in overflow-hidden ${color}`}>
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
                <Button variant="primary" size="sm" onClick={() => void handleDiagnoseAction(card)}>
                  {actionLabel}
                </Button>
              </div>
            )}
          </div>
        )
      })}



      {/* ============ Agent 提问卡（ask_user 工具，自由文本回答闭环） ============ */}
      {askCard && askCard.conversationId === currentConversation?.id && (
        <div className="fixed inset-0 z-[var(--app-z-modal-blocking)] flex items-center justify-center bg-black/30 backdrop-blur-[2px]">
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
              className="mt-3 w-full rounded-lg modern-card border-[var(--border)] px-3 py-2 text-[12px] outline-none resize-none placeholder:text-[var(--text-muted)]/60 focus:border-[var(--accent)] transition-colors"
            />
            <div className="mt-3 flex items-center justify-end gap-2">
              <span className="text-[10.5px] text-[var(--text-muted)] mr-auto">Ctrl+Enter ↵</span>
              <Button variant="secondary" size="md" onClick={handleAskSkip}>
                {t('home.askSkip')}
              </Button>
              <Button variant="primary" size="md" icon="send" onClick={handleAskSubmit}>
                {t('home.askSubmit')}
              </Button>
            </div>
          </div>
        </div>
      )}

      {/* ============ 工具权限审核浮层（自动审核模式，逐个确认） ============ */}
      {toolApprovals.length > 0 && (() => {
        const risk = approvalRisk(toolApprovals[0].tool, toolApprovals[0].level)
        return (
        <div className="fixed inset-0 z-[var(--app-z-modal-blocking)] flex items-center justify-center bg-black/30 backdrop-blur-[2px]">
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
              {toolApprovals[0].desc && (
                <p className="rounded-lg border border-[var(--border)] bg-[var(--bg-tertiary)]/60 px-3 py-2 text-[11px] leading-relaxed text-[var(--text-secondary)]">
                  {toolApprovals[0].desc}
                </p>
              )}
              <pre className="tool-output max-h-32 overflow-y-auto rounded-lg modern-card border-[var(--border)] p-2.5 text-[11px] font-mono whitespace-pre-wrap break-all text-[var(--text-primary)]">
                {toolApprovals[0].args || '{}'}
              </pre>
              <textarea
                value={approvalFeedback}
                onChange={(e) => setApprovalFeedback(e.target.value)}
                placeholder={t('home.toolApprovalFeedbackPlaceholder')}
                rows={2}
                className="w-full rounded-lg modern-card border-[var(--border)] px-3 py-2 text-[11px] outline-none resize-none placeholder:text-[var(--text-muted)]/60 focus:border-[var(--accent)] transition-colors"
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
              <Button
                variant="danger"
                size="md"
                onClick={() => resolveToolApproval(toolApprovals[0].requestId, false, false, approvalFeedback || undefined)}
              >
                {t('home.toolApprovalReject')}
              </Button>
              <Button
                variant="primary"
                size="md"
                onClick={() => resolveToolApproval(toolApprovals[0].requestId, true, approvalScope !== '', undefined, approvalScope || undefined)}
              >
                {t('home.toolApprovalAllow')}
              </Button>
            </div>
          </div>
        </div>
        )
      })()}

      {/* ============ 划词菜单（选中文本弹出） ============ */}
      {selectionMenu && (
        <div
          className="fixed z-[var(--app-z-popover)] animate-modal-in"
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
          <div className="flex items-center gap-0.5 rounded-xl modern-card shadow-2xl shadow-black/40 py-1 px-1">
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

      {/* ============ 会话时间线弹窗（快照点列表 → 回到历史决策点） ============ */}
      {timelineOpen && currentConversation && (
        <div className="fixed inset-0 z-[var(--app-z-modal)] flex items-center justify-center bg-black/30 backdrop-blur-[2px]">
          <div className="w-[520px] max-w-[92vw] rounded-2xl border border-[var(--border)] bg-[var(--bg-secondary)] shadow-2xl p-4 animate-modal-in">
            <div className="flex items-center gap-2">
              <Icon name="history" size={15} />
              <span className="text-[13px] font-semibold">{t('home.timelineTitle')}</span>
              <IconButton
                icon="close"
                label={t('common.close')}
                iconSize={13}
                className="ml-auto"
                onClick={() => setTimelineOpen(false)}
              />
            </div>
            <div className="mt-2 text-[11px] text-[var(--text-muted)] leading-relaxed">{t('home.timelineDesc')}</div>
            {branchParentId && (
              <button
                onClick={() => {
                  if (!currentConversation) return
                  void mergeConversationBranch(currentConversation.id, branchParentId)
                    .then((result) => alert(t('home.branchMergeDone', {
                      decisions: String(result.decisions_merged),
                      artifacts: String(result.artifacts_merged),
                      evidence: String(result.evidence_merged),
                    })))
                    .catch((error) => alert(String(error)))
                }}
                className="mt-2 w-full px-2.5 py-1.5 rounded-lg text-[11px] text-[var(--accent)] bg-[var(--accent-soft)] hover:opacity-80 transition-opacity"
              >
                {t('home.branchMergeStructured')}
              </button>
            )}
            <div className="mt-3 space-y-1 max-h-80 overflow-y-auto">
              {loadingSnapshots && snapshots.length === 0 ? (
                <div className="text-[11px] text-[var(--text-muted)] py-4 text-center">{t('common.loading')}</div>
              ) : snapshots.length === 0 ? (
                <div className="text-[11px] text-[var(--text-muted)] py-4 text-center">{t('home.timelineEmpty')}</div>
              ) : (
                snapshots.map((snap) => {
                  const isCurrent = snap.is_current
                  return (
                    <div
                      key={snap.id}
                      className={`flex items-center gap-2 rounded-lg modern-card border-[var(--border)] px-2.5 py-2 ${isCurrent ? 'opacity-75' : ''}`}
                    >
                      <Icon name="history" size={13} className="mt-0.5 shrink-0 text-[var(--text-muted)]" />
                      <div className="flex-1 min-w-0">
                        <div className="text-[12px] text-[var(--text-primary)] truncate">
                          {snap.label || t('home.timelineToolRound')}
                        </div>
                        <div className="text-[10px] text-[var(--text-muted)] mt-0.5">
                          {formatTime(snap.created_at)} · {t('home.timelineTools', { count: String(snap.tool_count) })}
                          {isCurrent && <span className="ml-1.5 text-[var(--success)]">{t('home.timelineCurrent')}</span>}
                        </div>
                      </div>
                      <button
                        onClick={() => {
                          setTimelineOpen(false)
                          void forkCurrentConversation(undefined, { kind: 'checkpoint', ref: snap.id })
                        }}
                        className="shrink-0 px-2 py-1 rounded-lg text-[11px] text-[var(--text-secondary)] hover:bg-[var(--bg-hover)] transition-colors"
                      >
                        {t('home.timelineFork')}
                      </button>
                      <button
                        onClick={() => {
                          setTimelineOpen(false)
                          handleRestoreSnapshot(snap)
                        }}
                        disabled={isCurrent}
                        className="shrink-0 px-2.5 py-1 rounded-lg text-[11px] text-[var(--accent)] bg-[var(--accent-soft)] hover:opacity-80 transition-opacity disabled:opacity-40 disabled:cursor-not-allowed"
                      >
                        {t('home.timelineRestore')}
                      </button>
                    </div>
                  )
                })
              )}
            </div>
          </div>
        </div>
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
        <div className="fixed inset-0 z-[var(--app-z-modal)] flex items-center justify-center bg-black/30 backdrop-blur-[2px]">
          <div className="w-[460px] max-w-[92vw] rounded-2xl border border-[var(--border)] bg-[var(--bg-secondary)] shadow-2xl p-4 animate-modal-in">
            <div className="flex items-center gap-2">
              <Icon name="check" size={15} />
              <span className="text-[13px] font-semibold">{t('home.whitelistTitle')}</span>
              <IconButton
                icon="close"
                label={t('common.close')}
                iconSize={13}
                className="ml-auto"
                onClick={() => setWhitelistOpen(false)}
              />
            </div>
            <div className="mt-3 space-y-1 max-h-72 overflow-y-auto">
              {whitelist.length === 0 && (
                <div className="text-[11px] text-[var(--text-muted)] py-4 text-center">{t('home.whitelistEmpty')}</div>
              )}
              {whitelist.map((w) => (
                <div
                  key={w.tool}
                  className="flex items-center gap-2 rounded-lg modern-card border-[var(--border)] px-2.5 py-1.5"
                >
                  <span className="text-[11px] font-mono text-[var(--text-primary)] flex-1 truncate">{w.tool}</span>
                  <span className="text-[10px] text-[var(--text-muted)] shrink-0">{formatTime(w.created_at)}</span>
                  <IconButton
                    icon="delete"
                    label={t('home.whitelistRemove')}
                    hoverTone="danger"
                    pad="xs"
                    iconSize={12}
                    onClick={() => {
                      removeToolWhitelist(currentProject.id, w.tool)
                        .then(() => setWhitelist((l) => l.filter((x) => x.tool !== w.tool)))
                        .catch(() => {})
                    }}
                  />
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

      {/* ============ 快捷键速查（? 触发） ============
       * 增强：顶部搜索框过滤；点击键位行复制组合到剪贴板（方便用户写培训文档） */}
      {showShortcuts && <ShortcutsPanel onClose={() => setShowShortcuts(false)} />}

      {/* ============ 批量任务浮层：每行一条入队 ============ */}
      {batchOpen && (
        <BatchSendDialog
          initial={draft}
          onClose={() => setBatchOpen(false)}
          onSubmit={async (lines) => {
            setBatchOpen(false)
            if (!currentProject || !currentConversation) return
            let ok = 0
            for (const line of lines) {
              try {
                await queueMessage(currentConversation.id, line, false)
                ok++
              } catch {
                // 跳过失败
              }
            }
            setDraft('')
            useNotificationStore.getState().push({
              tone: ok === lines.length ? 'success' : 'warn',
              title: t('home.batchSent'),
              body: t('home.batchSentBody', { ok, total: lines.length }),
            })
            void refreshQueued(currentConversation.id)
          }}
        />
      )}

      {/* ============ 导入会话预览（只读 + 复制全文） ============ */}
      {importDialog && (
        <ImportDialog
          data={{ title: importDialog.title, messages: importDialog.messages }}
          onClose={() => setImportDialog(null)}
        />
      )}

      {/* ============ 审计日志查看 ============ */}
      {auditOpen && <AuditDialog onClose={() => setAuditOpen(false)} />}

      {/* ============ 通用确认弹层（替代 window.confirm） ============ */}
      {confirmCfg.open && (
        <ConfirmDialog
          open={confirmCfg.open}
          title={confirmCfg.title}
          body={confirmCfg.body}
          tone={confirmCfg.tone}
          requireInput={confirmCfg.requireInput}
          onConfirm={() => {
            confirmCfg.onConfirm()
            setConfirmCfg((c) => ({ ...c, open: false }))
          }}
          onCancel={() => setConfirmCfg((c) => ({ ...c, open: false }))}
        />
      )}

      {/* ============ 项目快速切换器（Ctrl+Shift+P） ============ */}
      {projectSwitcherOpen && (
        <div className="cmdk-backdrop" onClick={() => setProjectSwitcherOpen(false)}>
          <ProjectSwitcher
            onClose={() => setProjectSwitcherOpen(false)}
            onSelect={(id) => {
              setProjectSwitcherOpen(false)
              openProject(id)
            }}
          />
        </div>
      )}

      {/* ============ 运行时异常提示（部署后监听捕获，一键修复） ============ */}
      {runtimeAnomaly && (
        <div className="fixed bottom-6 right-6 z-50 w-[400px] max-w-[92vw] rounded-xl modern-card border-red-500/60 shadow-2xl p-4 space-y-3">
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
            <Button variant="secondary" size="md" onClick={() => setRuntimeAnomaly(null)}>
              {t('runtime.dismiss')}
            </Button>
            <Button
              variant="danger"
              size="md"
              confirm
              confirmLabel={t('common.confirmAgain')}
              onClick={() => void fixRuntimeAnomaly()}
            >
              {t('runtime.fixNow')}
            </Button>
          </div>
        </div>
      )}

      {/* ============ 项目类型自动重新分类提示（框架标志文件变更时） ============ */}
      {projectMetaToast && (
        <div className="fixed bottom-6 right-6 z-50 w-[360px] max-w-[92vw] rounded-xl modern-card border-[var(--accent)]/50 shadow-2xl p-4 space-y-2.5">
          <div className="flex items-start gap-2">
            <span className="text-[var(--accent)] text-[16px] leading-none mt-0.5">🔄</span>
            <div className="flex-1 min-w-0">
              <h3 className="text-[13px] font-semibold">{t('projectMeta.title')}</h3>
              <p className="text-[11px] text-[var(--text-muted)] mt-0.5">{t('projectMeta.body')}</p>
            </div>
            <button
              onClick={() => setProjectMetaToast(null)}
              className="text-[var(--text-muted)] hover:text-[var(--text-primary)] text-[14px] leading-none"
            >
              ×
            </button>
          </div>
          <div className="flex items-center gap-2">
            <span className="px-2 py-0.5 rounded-md bg-[var(--bg-hover)] text-[12px] font-medium">
              {t(`projectMeta.kind.${projectMetaToast.old_kind}`)}
            </span>
            <span className="text-[var(--text-muted)] text-[12px]">→</span>
            <span className="px-2 py-0.5 rounded-md bg-[var(--accent)]/15 text-[var(--accent)] text-[12px] font-medium">
              {t(`projectMeta.kind.${projectMetaToast.new_kind}`)}
            </span>
          </div>
        </div>
      )}

      {/* ============ 修复经验候选弹窗（构建/部署由失败转成功时） ============ */}
      {knowledgeCandidate && (
        <div
          className="fixed bottom-6 right-6 z-50 w-[380px] max-w-[92vw] rounded-xl modern-card border-[var(--border)] shadow-2xl p-4 space-y-3"
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
            <Field
              fieldSize="md"
              value={knowledgeCandidate.title}
              onChange={(e) => setKnowledgeCandidate({ ...knowledgeCandidate, title: e.target.value })}
              placeholder={t('knowledge.entryTitle')}
            />
            <TextArea
              fieldSize="md"
              value={knowledgeCandidate.error_text}
              onChange={(e) => setKnowledgeCandidate({ ...knowledgeCandidate, error_text: e.target.value })}
              rows={3}
              placeholder={t('knowledge.candidateErrorPh')}
            />
            <TextArea
              fieldSize="md"
              value={knowledgeCandidate.fix}
              onChange={(e) => setKnowledgeCandidate({ ...knowledgeCandidate, fix: e.target.value })}
              rows={4}
              placeholder={t('knowledge.candidateFixPh')}
            />
          </div>
          <div className="flex justify-end gap-2">
            <Button variant="secondary" size="md" onClick={() => setKnowledgeCandidate(null)}>
              {t('knowledge.candidateDismiss')}
            </Button>
            <Button variant="primary" size="md" loading={candidateSaving} onClick={saveKnowledgeCandidate}>
              {t('knowledge.candidateSave')}
            </Button>
          </div>
        </div>
      )}

      {/* ============ Web 预览已迁移至右侧栏 Preview 面板 ============ */}
    </div>
  )
}


/* ============ 消息（无气泡，Claude/Qoder 风格） ============ */
/** 无版本历史时的稳定空数组引用：避免在 MessageItem 内每次渲染创建新数组破坏子组件 memo */
const EMPTY_VERSIONS: ChatMessage[] = []
const MessageItem = memo(function MessageItem({
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
  onQuoteMessage,
  onLocateMessage,
  onForkFrom,
  onCopyId,
  shortId,
  copiedId,
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
  onOpenVersions: (message: ChatMessage, userMessageId: string) => void
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
  /** 引用该消息到输入框（Quote） */
  onQuoteMessage?: (message: ChatMessage) => void
  /** 点击消息引用标签：定位并高亮被引用消息 */
  onLocateMessage?: (msgId: string) => void
  /** 从该 user 消息派生新会话（Fork：复制至此的消息，原会话不动） */
  onForkFrom?: (message: ChatMessage) => void
  /** 复制 ID 到剪贴板（排查问题用） */
  onCopyId?: (id: string) => void
  shortId: (id: string) => string
  copiedId: string | null
}) {
  const { t } = useTranslation()
  // 按消息粒度订阅：整表订阅会在任意一条消息点赞/版本更新时触发全部历史消息重渲染
  const feedback = useProjectStore((s) => s.feedbackMap[message.id])
  const versions = useProjectStore((s) => s.versionMap[userMessageId] ?? EMPTY_VERSIONS)
  const { role, content, reasoning, model } = message
  const [copied, setCopied] = useState(false)
  // 右键菜单：x/y 定位
  const [ctxMenu, setCtxMenu] = useState<{ x: number; y: number } | null>(null)

  // 右键菜单：x/y + 菜单项动作（直接挂在 onContextMenu）
  const onContextMenu = useCallback(
    (e: React.MouseEvent) => {
      e.preventDefault()
      setCtxMenu({ x: e.clientX, y: e.clientY })
    },
    [],
  )
  // 点击其他位置/滚动/失焦时关闭菜单
  useEffect(() => {
    if (!ctxMenu) return
    const close = () => setCtxMenu(null)
    window.addEventListener('click', close)
    window.addEventListener('scroll', close, true)
    return () => {
      window.removeEventListener('click', close)
      window.removeEventListener('scroll', close, true)
    }
  }, [ctxMenu])
  // 超长回复折叠：超过 2 万字符时默认收起，仅渲染纯文本摘要（避免单条消息数千行拖慢列表渲染与滚动）
  const [expanded, setExpanded] = useState(false)
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

  // 本条 user 消息的 @ 引用列表（references_json，气泡下方标签展示）；
  // "msg:{id}" 前缀条目为消息引用（Quote 功能），与文件引用分开渲染
  const { fileRefs, quotedMsgIds } = useMemo(() => {
    const fileRefs: string[] = []
    const quotedMsgIds: string[] = []
    if (!message.references_json) return { fileRefs, quotedMsgIds }
    try {
      const v = JSON.parse(message.references_json)
      if (!Array.isArray(v)) return { fileRefs, quotedMsgIds }
      for (const x of v) {
        if (typeof x !== 'string') continue
        if (x.startsWith('msg:')) quotedMsgIds.push(x.slice(4))
        else fileRefs.push(x)
      }
    } catch {
      /* 忽略损坏数据 */
    }
    return { fileRefs, quotedMsgIds }
  }, [message.references_json])

  // 注：role === 'tool' 的消息已在 renderItems 中合并为 ToolRunGroup 折叠组，不会进入本组件

  const isUser = role === 'user'
  const previewVersion = previewVersionId ? versions.find((v) => v.id === previewVersionId) ?? null : null
  const displayContent = previewVersion ? previewVersion.content : content
  const displayReasoning = previewVersion ? (previewVersion.reasoning ?? undefined) : reasoning
  const isLong = displayContent.length > 20000
  // 缓存 sanitizeToolMarkers 结果：长文本正则替换是 O(n) 开销，memoise 避免 MessageItem 因无关
  // props 变化（如 speaking/highlighted/confirmDeleteMsgId）重渲染时重复执行
  const sanitizedContent = useMemo(() => sanitizeToolMarkers(displayContent), [displayContent])

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
        onContextMenu={onContextMenu}
        className={`flex justify-end gap-3 group msg-row ${highlighted ? 'msg-highlight' : ''}`}
      >
        <div className="max-w-[80%] rounded-2xl rounded-tr-md bg-[var(--bg-secondary)] px-4 py-2 transition-colors">
          <div className="flex items-center gap-2 mb-1">
            <span className="text-[11px] font-medium text-[var(--text-secondary)]">{t('home.you')}</span>
            <span className="text-[10px] text-[var(--text-muted)] group-hover:text-[var(--text-secondary)] transition-colors">{time}</span>
            {/* 消息 ID：默认隐藏，悬浮时显示（排查问题用） */}
            {onCopyId && (
              <button
                onClick={() => onCopyId(message.id)}
                className="font-mono text-[9px] px-1 py-px rounded text-[var(--text-muted)] hover:text-[var(--accent)] transition-colors opacity-0 group-hover:opacity-100"
                title={`${t('home.msgId')}: ${message.id}\n${t('home.clickToCopy')}`}
              >
                {copiedId === message.id ? <Icon name="check" size={8} className="text-[var(--success)]" /> : '#'}
                {shortId(message.id)}
              </button>
            )}
            {queued && (
              <span className="text-[10px] px-1.5 py-0.5 rounded-md bg-[var(--accent)]/10 text-[var(--accent)]">
                {message.agent_owned === 1 ? t('home.queuedAgentLabel') : t('home.queuedLabel')}
              </span>
            )}
            {/* @ 引用标签（references_json 落库展示） */}
            {fileRefs.length > 0 && (
              <span
                className="flex items-center gap-1 text-[10px] text-[var(--text-muted)] max-w-52 overflow-hidden"
                title={fileRefs.join('\n')}
              >
                <Icon name="file" size={10} className="shrink-0" />
                <span className="truncate">{fileRefs.join(', ')}</span>
              </span>
            )}
            {/* 消息引用标签（Quote 落库 msg:{id}；点击定位被引用消息） */}
            {quotedMsgIds.length > 0 && onLocateMessage && quotedMsgIds.map((mid) => (
              <button
                key={mid}
                onClick={() => onLocateMessage(mid)}
                className="flex items-center gap-1 text-[10px] text-[var(--accent)] hover:text-[var(--accent-hover)] max-w-40"
                title={t('home.locateQuoted')}
              >
                <Icon name="quote" size={10} className="shrink-0" />
                <span className="truncate">{t('home.quotedMsgLabel')}</span>
              </button>
            ))}
            <div className="ml-auto flex items-center gap-0.5 opacity-0 group-hover:opacity-100 max-md:opacity-100 transition-opacity">
              {onQuoteMessage && (
                <button
                  onClick={() => onQuoteMessage(message)}
                  className="text-[10px] text-[var(--text-muted)] hover:text-[var(--accent)] hover:bg-[var(--bg-hover)] rounded-md px-1.5 py-0.5 flex items-center gap-0.5"
                  title={t('home.quoteMessage')}
                >
                  <Icon name="quote" size={11} />{t('home.quoteMessage')}
                </button>
              )}
              {onForkFrom && (
                <button
                  onClick={() => onForkFrom(message)}
                  className="text-[10px] text-[var(--text-muted)] hover:text-[var(--accent)] hover:bg-[var(--bg-hover)] rounded-md px-1.5 py-0.5 flex items-center gap-0.5"
                  title={t('home.forkFromHere')}
                >
                  <Icon name="git-branch" size={11} />{t('home.forkFromHere')}
                </button>
              )}
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
                  className={`text-[10px] px-1.5 py-0.5 rounded-md transition-all ${
                    confirmDeleteMsgId === message.id
                      ? 'text-white bg-[var(--danger)] shadow-[0_0_0_3px_var(--danger-50)]'
                      : 'text-[var(--text-muted)] hover:text-[var(--danger)] hover:bg-[var(--bg-hover)]'
                  }`}
                  title={
                    confirmDeleteMsgId === message.id
                      ? t('home.deleteMessageConfirm')
                      : t('home.deleteMessage')
                  }
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
            {/* 超长消息折叠（粘贴大段日志等）：与助手分支同规则 */}
            {isLong && !expanded ? (
              <div>
                <pre className="whitespace-pre-wrap break-all font-mono text-[11px] leading-relaxed text-[var(--text-secondary)] max-h-48 overflow-y-auto">
                  {content.slice(0, 2000)}
                </pre>
                <span className="text-[10.5px] text-[var(--text-muted)]">…</span>
                <button
                  onClick={() => setExpanded(true)}
                  className="ml-1 text-[11px] text-[var(--accent)] hover:underline"
                >
                  {t('home.expandFullMessage', { count: content.length.toLocaleString() })}
                </button>
              </div>
            ) : (
              <Markdown onOpenFile={onOpenFile}>{content}</Markdown>
            )}
          </div>
        </div>
        <div className="w-7 h-7 shrink-0 rounded-lg modern-card border-[var(--border)] flex items-center justify-center text-[11px] font-medium text-[var(--text-secondary)]">
          {t('home.you').charAt(0)}
        </div>
      </div>
    )
  }

  return (
    <div
      data-msg-id={message.id}
      onContextMenu={onContextMenu}
      className={`flex gap-3 group msg-row ${highlighted ? 'msg-highlight' : ''}`}
    >
      <div className="w-6 h-6 rounded-full bg-[var(--bg-hover)] flex items-center justify-center shrink-0 mt-0.5">
        <Icon name="spark" size={12} className="text-[var(--text-muted)]" />
      </div>
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-2 mb-1">
          <span className="text-[12px] font-medium text-[var(--text-primary)]">{t('home.agent')}</span>
          <span className="text-[11px] text-[var(--text-muted)] group-hover:text-[var(--text-secondary)] transition-colors">{time}</span>
          {/* 次要元信息：默认隐藏，悬浮消息时显示，保持清爽 */}
          <div className="flex items-center gap-1.5 opacity-0 group-hover:opacity-100 transition-opacity">
            {onCopyId && (
              <button
                onClick={() => onCopyId(message.id)}
                className="font-mono text-[9px] px-1 py-px rounded text-[var(--text-muted)] hover:text-[var(--accent)] transition-colors"
                title={`${t('home.msgId')}: ${message.id}\n${t('home.clickToCopy')}`}
              >
                {copiedId === message.id ? <Icon name="check" size={8} className="text-[var(--success)]" /> : '#'}
                {shortId(message.id)}
              </button>
            )}
            {model && <span className="text-[10px] text-[var(--text-muted)]">{model}</span>}
            {message.duration_ms != null && message.duration_ms > 0 && (
              <span
                className="text-[10px] tabular-nums text-[var(--text-muted)]"
                title={t('home.replyDurationHint')}
              >
                {fmtElapsed(message.duration_ms / 1000)}
              </span>
            )}
            {(message.tokens_in != null || message.tokens_out != null) && (
              <span
                className="text-[10px] tabular-nums text-[var(--text-muted)]"
                title={`${t('home.tokenHint')}：${t('home.tokenIn')} ${message.tokens_in ?? 0} / ${t('home.tokenOut')} ${message.tokens_out ?? 0}`}
              >
                ↑{(message.tokens_in ?? 0).toLocaleString()} ↓{(message.tokens_out ?? 0).toLocaleString()}
              </span>
            )}
          </div>
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
          {isLong && !expanded ? (
            <div>
              <pre className="whitespace-pre-wrap break-all font-mono text-[11px] leading-relaxed text-[var(--text-secondary)] max-h-48 overflow-y-auto">
                {displayContent.slice(0, 2000)}
              </pre>
              <span className="text-[10.5px] text-[var(--text-muted)]">…</span>
              <button
                onClick={() => setExpanded(true)}
                className="ml-1 text-[11px] text-[var(--accent)] hover:underline"
              >
                {t('home.expandFullMessage', { count: displayContent.length.toLocaleString() })}
              </button>
            </div>
          ) : (
            <Markdown onOpenFile={onOpenFile}>{sanitizedContent}</Markdown>
          )}
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
              <IconButton
                icon="close"
                label={t('home.close')}
                iconSize={12}
                className="ml-auto"
                onClick={() => setBranchOpen(false)}
              />
              <button
                onClick={() => onOpenVersions(message, userMessageId)}
                className="px-2.5 py-1 rounded-lg text-[11px] border border-[var(--border)] text-[var(--text-secondary)] hover:text-[var(--accent)] hover:border-[var(--accent)] transition-colors"
              >
                {t('home.branchCompareDiff')}
              </button>
            </div>
          </div>
        )}
        {/* 操作栏：复制 / 引用 / 重新生成 / 点赞 / 点踩 / 朗读 / 版本对比 */}
        <div className="flex items-center gap-0.5 mt-1.5 opacity-0 group-hover:opacity-100 max-md:opacity-100 transition-opacity">
          <IconButton
            icon={copied ? 'check' : 'copy'}
            label={t('home.copyMessage')}
            iconSize={13}
            onClick={copyMessage}
          />
          {onQuoteMessage && (
            <IconButton
              icon="quote"
              label={t('home.quoteMessage')}
              iconSize={13}
              onClick={() => onQuoteMessage(message)}
            />
          )}
          <IconButton
            icon="lightbulb"
            label={t('knowledge.rememberThisFix')}
            iconSize={13}
            onClick={openRemember}
          />
          {isLastAssistant && onRegenerate && (
            <IconButton
              icon="refresh"
              label={t('home.regenerate')}
              iconSize={13}
              onClick={onRegenerate}
            />
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
              className={`p-1 rounded-md transition-all ${confirmDeleteMsgId === message.id ? 'bg-[var(--danger)] text-white shadow-[0_0_0_3px_var(--danger-50)]' : 'text-[var(--text-muted)] hover:text-[var(--danger)] hover:bg-[var(--bg-hover)]'}`}
              title={confirmDeleteMsgId === message.id ? t('home.deleteMessageConfirm') : t('home.deleteMessage')}
            >
              <Icon name="delete" size={13} white={confirmDeleteMsgId === message.id} />
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
            className="w-[480px] max-w-[92vw] rounded-xl modern-card border-[var(--border)] shadow-2xl p-4 space-y-3"
            onClick={(e) => e.stopPropagation()}
          >
            <h3 className="text-[14px] font-semibold">{t('knowledge.rememberTitle')}</h3>
            <p className="text-[11px] text-[var(--text-muted)]">{t('knowledge.rememberHint')}</p>
            <Field
              fieldSize="md"
              label={t('knowledge.entryTitle')}
              value={rememberForm.title}
              onChange={(e) => setRememberForm({ ...rememberForm, title: e.target.value })}
              placeholder={t('knowledge.rememberTitlePh')}
            />
            <TextArea
              fieldSize="md"
              label={t('knowledge.rememberError')}
              value={rememberForm.error_text}
              onChange={(e) => setRememberForm({ ...rememberForm, error_text: e.target.value })}
              rows={4}
              placeholder={t('knowledge.rememberErrorPh')}
            />
            <TextArea
              fieldSize="md"
              label={t('knowledge.fix')}
              value={rememberForm.fix}
              onChange={(e) => setRememberForm({ ...rememberForm, fix: e.target.value })}
              rows={5}
            />
            <div className="flex justify-end gap-2 pt-1">
              <Button variant="secondary" size="md" onClick={() => setRememberOpen(false)}>
                {t('mcp.cancel')}
              </Button>
              <Button variant="primary" size="md" loading={rememberSaving} onClick={saveRemember}>
                {t('knowledge.save')}
              </Button>
            </div>
          </div>
        </div>
      )}

      {/* 右键菜单：复制 / 重新生成（仅 assistant 最后一条）/ 编辑（仅 user）/ 删除 */}
      {ctxMenu && (
        <div
          role="menu"
          onClick={(e) => e.stopPropagation()}
          className="fixed z-[var(--app-z-popover)] min-w-[180px] glass-card rounded-lg py-1 animate-modal-in text-[12.5px]"
          style={{ left: ctxMenu.x, top: ctxMenu.y }}
        >
          <button
            onClick={() => {
              copyMessage()
              setCtxMenu(null)
            }}
            className="w-full flex items-center gap-2.5 px-3 py-1.5 text-left text-[var(--text-primary)] hover:bg-[var(--bg-hover)] transition-colors"
          >
            <Icon name={copied ? 'check' : 'copy'} size={13} />
            {copied ? t('home.copied') : t('home.copyMessage')}
          </button>
          {message.role === 'user' && onEditMessage && (
            <button
              onClick={() => {
                onEditMessage(message)
                setCtxMenu(null)
              }}
              className="w-full flex items-center gap-2.5 px-3 py-1.5 text-left text-[var(--text-primary)] hover:bg-[var(--bg-hover)] transition-colors"
            >
              <Icon name="edit" size={13} />
              {t('home.edit')}
            </button>
          )}
          {message.role === 'user' && onDeleteMessage && (
            <button
              onClick={() => {
                onDeleteMessage(message)
                setCtxMenu(null)
              }}
              className="w-full flex items-center gap-2.5 px-3 py-1.5 text-left text-[var(--danger)] hover:bg-[var(--danger-50)] transition-colors"
            >
              <Icon name="delete" size={13} />
              {t('home.delete')}
            </button>
          )}
          {message.role === 'assistant' && isLastAssistant && onRegenerate && (
            <button
              onClick={() => {
                onRegenerate()
                setCtxMenu(null)
              }}
              className="w-full flex items-center gap-2.5 px-3 py-1.5 text-left text-[var(--text-primary)] hover:bg-[var(--bg-hover)] transition-colors"
            >
              <Icon name="refresh" size={13} />
              {t('home.regenerate')}
            </button>
          )}
          {message.role === 'assistant' && onBranch && (
            <button
              onClick={() => {
                onBranch(message)
                setCtxMenu(null)
              }}
              className="w-full flex items-center gap-2.5 px-3 py-1.5 text-left text-[var(--text-primary)] hover:bg-[var(--bg-hover)] transition-colors"
            >
              <Icon name="git-branch" size={13} />
              {t('home.forkFromHere')}
            </button>
          )}
          {/* Pin 到会话顶部，同时持久化为 Context V2 权威上下文。 */}
          <button
            onClick={() => {
              const convId = message.conversation_id
              const isP = usePinStore.getState().isPinned(convId, message.id)
              usePinStore.getState().toggle(convId, message.id)
              void setConversationContextPin({
                conversation_id: convId,
                pin_kind: 'message',
                source_ref: `message:${message.id}`,
                label: message.role,
                content: message.content,
                pinned: !isP,
              }).catch(() => {})
              // toast 反馈
              useNotificationStore.getState().push({
                tone: isP ? 'info' : 'success',
                title: isP ? t('home.unpinned') : t('home.pinned'),
                body: isP
                  ? t('home.unpinnedBody')
                  : t('home.pinnedBody', { count: (usePinStore.getState().pins[convId]?.length ?? 0), max: PIN_MAX_PER_CONV }),
              })
              setCtxMenu(null)
            }}
            className="w-full flex items-center gap-2.5 px-3 py-1.5 text-left text-[var(--text-primary)] hover:bg-[var(--bg-hover)] transition-colors"
          >
            <Icon name={usePinStore.getState().isPinned(message.conversation_id, message.id) ? 'check' : 'pin'} size={13} />
            {usePinStore.getState().isPinned(message.conversation_id, message.id) ? t('home.unpinFromTop') : t('home.pinToTop')}
          </button>
          {/* 复制消息链接：hash 锚点格式 #msg={id}，贴到任意位置再点 → 切到该会话并滚动高亮 */}
          <button
            onClick={async () => {
              const link = `${window.location.origin}${window.location.pathname}#msg=${message.id}`
              try {
                await navigator.clipboard.writeText(link)
                useNotificationStore.getState().push({
                  tone: 'success',
                  title: t('home.linkCopied'),
                  body: t('home.linkCopiedBody'),
                })
              } catch {
                useNotificationStore.getState().push({
                  tone: 'error',
                  title: t('home.linkCopyFail'),
                  body: '',
                })
              }
              setCtxMenu(null)
            }}
            className="w-full flex items-center gap-2.5 px-3 py-1.5 text-left text-[var(--text-primary)] hover:bg-[var(--bg-hover)] transition-colors"
          >
            <Icon name="quote" size={13} />
            {t('home.copyMessageLink')}
          </button>
          {message.role === 'assistant' && onOpenVersions && versions.length > 0 && (
            <button
              onClick={() => {
                onOpenVersions(message, userMessageId)
                setCtxMenu(null)
              }}
              className="w-full flex items-center gap-2.5 px-3 py-1.5 text-left text-[var(--text-primary)] hover:bg-[var(--bg-hover)] transition-colors"
            >
              <Icon name="history" size={13} />
              {t('home.viewVersions')} ({versions.length})
            </button>
          )}
          {/* 1-5 星评分（纯前端 localStorage；与 like/dislike 互不干扰）—— 仅 assistant 消息 */}
          {message.role === 'assistant' && (
            <div className="border-t border-[var(--border)] mt-1 pt-1">
              <div className="px-3 py-1 text-[10px] font-medium text-[var(--text-muted)] uppercase tracking-wider flex items-center justify-between">
                <span>{t('home.rateTitle')}</span>
                {useRatingStore.getState().get(message.id) && (
                  <button
                    onClick={() => {
                      useRatingStore.getState().remove(message.id)
                      useNotificationStore.getState().push({ tone: 'info', title: t('home.rateCleared'), body: '' })
                      setCtxMenu(null)
                    }}
                    className="text-[10px] text-[var(--text-muted)] hover:text-[var(--danger)] normal-case tracking-normal"
                  >
                    {t('home.rateClear')}
                  </button>
                )}
              </div>
              <div className="flex items-center justify-around px-2 py-1.5">
                {[1, 2, 3, 4, 5].map((n) => {
                  const cur = useRatingStore.getState().get(message.id)
                  const active = cur?.score === n
                  return (
                    <button
                      key={n}
                      onClick={() => {
                        useRatingStore.getState().set(message.id, { score: n, comment: null })
                        useNotificationStore.getState().push({
                          tone: 'success',
                          title: t('home.rateSaved'),
                          body: t('home.rateSavedBody', { n }),
                        })
                        setCtxMenu(null)
                      }}
                      className={`w-7 h-7 rounded-md flex items-center justify-center text-[14px] font-semibold transition-all active:scale-90 ${
                        active
                          ? 'bg-[var(--warning)] text-white shadow-sm'
                          : 'text-[var(--text-muted)] hover:bg-[var(--bg-hover)] hover:text-[var(--warning)]'
                      }`}
                      title={t('home.rateStar', { n })}
                    >
                      {n}
                    </button>
                  )
                })}
              </div>
            </div>
          )}
        </div>
      )}
    </div>
  )
})


/* ============ 快捷键速查：键帽行（键 + 描述） ============ */
export function ShortcutRow({ keys, desc }: { keys: string[]; desc: string }) {
  return (
    <div className="flex items-center justify-between gap-2 py-1">
      <span className="text-[var(--text-secondary)] text-[12px]">{desc}</span>
      <span className="flex items-center gap-1 shrink-0">
        {keys.map((k, i) => (
          <span key={i} className="inline-flex items-center">
            <kbd className="px-1.5 py-0.5 text-[10.5px] font-mono font-medium rounded border border-[var(--border)] bg-[var(--bg-card)] text-[var(--text-primary)] min-w-[22px] text-center tnum">
              {k}
            </kbd>
            {i < keys.length - 1 && <span className="text-[var(--text-muted)] text-[10.5px] mx-0.5">+</span>}
          </span>
        ))}
      </span>
    </div>
  )
}


/* ============ 快捷键速查面板（? 触发）：分组 + 搜索 + 一键复制键位 ============
 * - 数据结构硬编码在组件内：每组若干 { keys, desc, groupKey }
 * - 顶部搜索框：模糊匹配 desc / keys
 * - 点击行复制键位组合到剪贴板（如 "Ctrl+Shift+P"），toast 反馈
 * - Esc 关闭
 */
function ShortcutsPanel({ onClose }: { onClose: () => void }) {
  const { t } = useTranslation()
  const [query, setQuery] = useState('')
  const [copiedKey, setCopiedKey] = useState<string | null>(null)
  const inputRef = useRef<HTMLInputElement>(null)

  // 全部快捷键（硬编码）：分组 → [{ keys, desc, descKey }]
  const groups: { titleKey: string; items: { keys: string[]; descKey: string; copyKey: string }[] }[] = [
    {
      titleKey: 'home.shortcutGroupNav',
      items: [
        { keys: ['Ctrl', 'K'], descKey: 'home.shortcutCommandPalette', copyKey: 'Ctrl+K' },
        { keys: ['Ctrl', 'Shift', 'K'], descKey: 'home.shortcutFocusSearch', copyKey: 'Ctrl+Shift+K' },
        { keys: ['Ctrl', 'Shift', 'P'], descKey: 'home.shortcutProjectSwitcher', copyKey: 'Ctrl+Shift+P' },
        { keys: ['Ctrl', 'Shift', 'N'], descKey: 'home.shortcutNewConv', copyKey: 'Ctrl+Shift+N' },
        { keys: ['?'], descKey: 'home.shortcutThisPanel', copyKey: '?' },
        { keys: ['Esc'], descKey: 'home.shortcutClose', copyKey: 'Esc' },
      ],
    },
    {
      titleKey: 'home.shortcutGroupQuick',
      items: [
        { keys: ['Ctrl', 'Shift', 'B'], descKey: 'home.shortcutBuild', copyKey: 'Ctrl+Shift+B' },
        { keys: ['Ctrl', 'Shift', 'D'], descKey: 'home.shortcutDeploy', copyKey: 'Ctrl+Shift+D' },
        { keys: ['Ctrl', 'Shift', 'S'], descKey: 'home.shortcutScreenshot', copyKey: 'Ctrl+Shift+S' },
        { keys: ['Ctrl', 'Shift', 'R'], descKey: 'home.shortcutRunCommand', copyKey: 'Ctrl+Shift+R' },
      ],
    },
    {
      titleKey: 'home.shortcutGroupInput',
      items: [
        { keys: ['Enter'], descKey: 'home.shortcutSend', copyKey: 'Enter' },
        { keys: ['Shift', 'Enter'], descKey: 'home.shortcutNewline', copyKey: 'Shift+Enter' },
        { keys: ['Ctrl', 'Enter'], descKey: 'home.shortcutSendCmd', copyKey: 'Ctrl+Enter' },
      ],
    },
    {
      titleKey: 'home.shortcutGroupMessage',
      items: [
        { keys: ['右键'], descKey: 'home.shortcutContextMenu', copyKey: 'RightClick' },
      ],
    },
  ]

  // 过滤：query 命中 keys 或 desc
  const q = query.trim().toLowerCase()
  const filtered = q
    ? groups
        .map((g) => ({
          ...g,
          items: g.items.filter((it) => {
            const desc = t(it.descKey).toLowerCase()
            const keyStr = it.copyKey.toLowerCase()
            return desc.includes(q) || keyStr.includes(q)
          }),
        }))
        .filter((g) => g.items.length > 0)
    : groups

  // 复制键位
  const copyKey = async (text: string) => {
    try {
      await navigator.clipboard.writeText(text)
      setCopiedKey(text)
      setTimeout(() => setCopiedKey((cur) => (cur === text ? null : cur)), 1200)
    } catch {
      // 剪贴板不可用 → 静默
    }
  }

  // 自动聚焦搜索框
  useEffect(() => {
    inputRef.current?.focus()
  }, [])
  useEscapeKey(onClose)

  return (
    <div className="cmdk-backdrop" onClick={onClose}>
      <div
        className="w-[560px] max-w-[92vw] max-h-[82vh] flex flex-col glass-card p-5 animate-modal-in"
        onClick={(e) => e.stopPropagation()}
      >
        {/* 标题 + 关闭 */}
        <div className="flex items-center justify-between mb-3">
          <div className="flex items-center gap-2">
            <Icon name="bolt" size={15} />
            <h2 className="text-[14px] font-semibold">{t('home.shortcuts')}</h2>
          </div>
          <IconButton icon="close" label={t('common.close')} onClick={onClose} />
        </div>

        {/* 搜索框 */}
        <div className="mb-3 flex items-center gap-2 px-2.5 h-8 rounded-lg bg-[var(--bg-primary)] border border-[var(--border)] focus-within:border-[var(--accent)] transition-colors">
          <Icon name="search" size={12} className="text-[var(--text-muted)]" />
          <input
            ref={inputRef}
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder={t('home.shortcutSearchPlaceholder')}
            spellCheck={false}
            className="flex-1 bg-transparent outline-none text-[12.5px] placeholder:text-[var(--text-muted)]"
          />
          {query && (
            <button
              onClick={() => setQuery('')}
              className="text-[10px] text-[var(--text-muted)] hover:text-[var(--text-primary)]"
            >
              <Icon name="close" size={11} />
            </button>
          )}
        </div>

        {/* 分组列表 */}
        <div className="space-y-3 text-[12.5px] overflow-y-auto min-h-0 flex-1">
          {filtered.length === 0 ? (
            <p className="text-center text-[12px] text-[var(--text-muted)] py-6">{t('home.shortcutNoResults')}</p>
          ) : (
            filtered.map((g) => (
              <div key={g.titleKey}>
                <p className="text-[10.5px] font-medium text-[var(--text-muted)] uppercase tracking-wider mb-1.5">
                  {t(g.titleKey)}
                </p>
                {g.items.map((it) => {
                  const isCopied = copiedKey === it.copyKey
                  return (
                    <div
                      key={it.copyKey}
                      onClick={() => void copyKey(it.copyKey)}
                      title={t('home.shortcutCopyHint')}
                      className="group flex items-center justify-between gap-2 py-1 px-1.5 -mx-1.5 rounded-md hover:bg-[var(--bg-hover)] cursor-pointer transition-colors"
                    >
                      <span className="text-[var(--text-secondary)] text-[12px]">{t(it.descKey)}</span>
                      <span className="flex items-center gap-0.5 shrink-0">
                        {it.keys.map((k, i) => (
                          <span key={i} className="flex items-center gap-0.5">
                            {i > 0 && <span className="text-[10px] text-[var(--text-muted)]">+</span>}
                            <kbd className="text-[10px] px-1.5 py-0.5 rounded border border-[var(--border)] bg-[var(--bg-hover)] text-[var(--text-muted)] tnum">
                              {k}
                            </kbd>
                          </span>
                        ))}
                        <Icon
                          name={isCopied ? 'check' : 'copy'}
                          size={10}
                          className={`ml-1.5 transition-all ${isCopied ? 'text-[var(--success)] opacity-100' : 'text-[var(--text-muted)] opacity-0 group-hover:opacity-100'}`}
                        />
                      </span>
                    </div>
                  )
                })}
              </div>
            ))
          )}
        </div>

        {/* 底部提示 */}
        <p className="mt-3 pt-2.5 border-t border-[var(--border)] text-[10.5px] text-[var(--text-muted)]">
          {t('home.shortcutFooter')}
        </p>
      </div>
    </div>
  )
}


/* ============ 项目快速切换器：Ctrl+Shift+P 触发，fuzzy 搜项目 + 最近 5 个置顶 ============ */
function ProjectSwitcher({ onClose, onSelect }: { onClose: () => void; onSelect: (id: string) => void }) {
  const { t } = useTranslation()
  const projects = useProjectStore((s) => s.projects) as Project[]
  const currentProject = useProjectStore((s) => s.currentProject)
  const [query, setQuery] = useState('')
  const [sel, setSel] = useState(0)
  const listRef = useRef<HTMLDivElement>(null)
  const inputRef = useRef<HTMLInputElement>(null)

  // fuzzy 过滤：项目名 / 路径包含 query
  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase()
    const list = !q
      ? projects
      : projects.filter((p) => p.name.toLowerCase().includes(q) || p.path.toLowerCase().includes(q))
    // 当前项目置顶 + 最近 5 个靠前
    return [...list].sort((a, b) => {
      if (a.id === currentProject?.id) return -1
      if (b.id === currentProject?.id) return 1
      return (b.last_opened_at ?? 0) - (a.last_opened_at ?? 0)
    })
  }, [projects, query, currentProject?.id])

  // 打开时聚焦输入框
  useEffect(() => {
    const h = setTimeout(() => inputRef.current?.focus(), 30)
    return () => clearTimeout(h)
  }, [])

  // 键盘：↑↓ 移动 / Enter 选择（Esc 走全局栈，见下）
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'ArrowDown') {
        e.preventDefault()
        setSel((s) => Math.min(filtered.length - 1, s + 1))
      } else if (e.key === 'ArrowUp') {
        e.preventDefault()
        setSel((s) => Math.max(0, s - 1))
      } else if (e.key === 'Enter') {
        e.preventDefault()
        const p = filtered[sel]
        if (p) onSelect(p.id)
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [filtered, sel, onSelect])
  useEscapeKey(onClose)

  return (
    <div
      className="w-[520px] max-w-[92vw] rounded-2xl glass-card overflow-hidden animate-modal-in"
      onClick={(e) => e.stopPropagation()}
    >
      <div className="flex items-center gap-2.5 px-4 h-12 border-b border-[var(--border)]">
        <Icon name="folder" size={16} className="text-[var(--text-muted)] shrink-0" />
        <input
          ref={inputRef}
          value={query}
          onChange={(e) => {
            setQuery(e.target.value)
            setSel(0)
          }}
          placeholder={t('home.projectSwitcherPlaceholder')}
          className="flex-1 bg-transparent outline-none text-[14px] placeholder:text-[var(--text-muted)]"
          spellCheck={false}
        />
        <kbd className="shrink-0 text-[10px] px-1.5 py-0.5 rounded border border-[var(--border)] bg-[var(--bg-hover)] text-[var(--text-muted)] tnum">Esc</kbd>
      </div>
      <div ref={listRef} className="max-h-[50vh] overflow-y-auto py-1.5">
        {filtered.length === 0 ? (
          <div className="px-4 py-8 text-center text-[13px] text-[var(--text-muted)]">{t('home.projectSwitcherEmpty')}</div>
        ) : (
          filtered.map((p, i) => {
            const active = i === sel
            const isCurrent = p.id === currentProject?.id
            return (
              <button
                key={p.id}
                onClick={() => onSelect(p.id)}
                onMouseEnter={() => setSel(i)}
                className={`w-full flex items-center gap-2.5 px-4 py-2 text-left transition-colors ${
                  active ? 'bg-[var(--accent-soft)]' : 'hover:bg-[var(--bg-hover)]'
                }`}
              >
                <div className="w-7 h-7 rounded-lg bg-gradient-to-br from-[var(--accent)] to-[#8b5cf6] flex items-center justify-center shrink-0">
                  <Icon name="folder" size={13} white />
                </div>
                <div className="min-w-0 flex-1">
                  <div className="text-[13px] truncate flex items-center gap-1.5">
                    {p.name}
                    {isCurrent && (
                      <span className="text-[10px] px-1 rounded bg-[var(--accent)]/15 text-[var(--accent)]">{t('home.projectSwitcherCurrent')}</span>
                    )}
                  </div>
                  <div className="text-[10.5px] text-[var(--text-muted)] truncate font-mono">{p.path}</div>
                </div>
                {p.index_state && (
                  <span className="text-[10px] text-[var(--text-muted)] tnum shrink-0">
                    {p.index_state === 'ready' ? '✓' : '…'}
                  </span>
                )}
              </button>
            )
          })
        )}
      </div>
      <div className="flex items-center gap-3 px-4 h-9 border-t border-[var(--border)] text-[11px] text-[var(--text-muted)] tnum">
        <span><kbd className="px-1 py-0.5 rounded border border-[var(--border)] bg-[var(--bg-hover)]">↑</kbd> <kbd className="px-1 py-0.5 rounded border border-[var(--border)] bg-[var(--bg-hover)]">↓</kbd> {t('home.projectSwitcherNavigate')}</span>
        <span><kbd className="px-1 py-0.5 rounded border border-[var(--border)] bg-[var(--bg-hover)]">Enter</kbd> {t('home.projectSwitcherOpen')}</span>
      </div>
    </div>
  )
}


/* ============ 批量任务浮层：每行一条入队（agentOwned=false 让任务结束后自动续跑） ============
 * - 多行文本 → 逐行入队 → Agent 串行处理
 * - 入参 initial：当前输入框草稿（默认填入，用户可继续编辑）
 * - 入参 onSubmit：用户点"入队"时触发，把行数组交给外层
 * - Esc 关闭、Cmd/Ctrl+Enter 直接入队
 */
function BatchSendDialog({ initial, onClose, onSubmit }: {
  initial: string
  onClose: () => void
  onSubmit: (lines: string[]) => void | Promise<void>
}) {
  const { t } = useTranslation()
  const [text, setText] = useState(initial)
  // 拆行：去空白、去空行；保留用户原顺序
  const lines = useMemo(
    () => text.split('\n').map((s) => s.trim()).filter(Boolean),
    [text]
  )
  const submittingRef = useRef(false)

  // 提交：避免重复触发（点按钮 / 按快捷键同时按）
  const submit = async () => {
    if (submittingRef.current) return
    if (lines.length === 0) return
    submittingRef.current = true
    try {
      await onSubmit(lines)
    } finally {
      submittingRef.current = false
    }
  }

  // 快捷键：Ctrl+Enter 提交（Esc 走全局栈，见下）
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) {
        e.preventDefault()
        void submit()
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [lines.length]) // submit 是稳定闭包，但依赖 lines 让 Enter 触发时拿到最新行数
  useEscapeKey(onClose)

  return (
    <div className="cmdk-backdrop" onClick={onClose}>
      <div
        className="w-[600px] max-w-[92vw] glass-card p-4 animate-modal-in"
        onClick={(e) => e.stopPropagation()}
      >
        {/* 标题栏 */}
        <div className="flex items-center justify-between mb-2">
          <div className="flex items-center gap-2">
            <div className="w-6 h-6 rounded-md bg-[var(--accent)]/15 text-[var(--accent)] flex items-center justify-center">
              <Icon name="package" size={13} />
            </div>
            <h2 className="text-[14px] font-semibold">{t('home.batch')}</h2>
          </div>
          {/* h-6 w-6 锁死 24px 盒：裸 IconButton 只有 13px 图标 + 8px padding = 21px */}
          <IconButton icon="close" label={t('home.cancel')} iconSize={13} className="h-6 w-6" onClick={onClose} />
        </div>

        <p className="text-[11.5px] text-[var(--text-muted)] mb-2.5 leading-relaxed">
          {t('home.batchHint')}
        </p>

        {/* 多行输入框：等宽字体，按行解析 */}
        <textarea
          value={text}
          onChange={(e) => setText(e.target.value)}
          rows={10}
          autoFocus
          spellCheck={false}
          className="w-full rounded-lg modern-card p-3 text-[13px] font-mono leading-relaxed outline-none focus:border-[var(--accent)] resize-y min-h-[180px] max-h-[50vh]"
          placeholder={t('home.batchPlaceholder')}
        />

        {/* 底部状态行 + 操作按钮 */}
        <div className="flex items-center justify-between mt-3">
          <div className="flex items-center gap-3 text-[11px] text-[var(--text-muted)] tnum">
            <span className="flex items-center gap-1">
              {t('home.batchLines', { count: lines.length })}
            </span>
            {lines.length > 1 && (
              <span className="text-[10.5px] px-1.5 py-0.5 rounded bg-[var(--accent)]/10 text-[var(--accent)]">
                {t('home.batchQueue', { count: lines.length })}
              </span>
            )}
          </div>
          <div className="flex items-center gap-2">
            <kbd className="text-[10px] px-1.5 py-0.5 rounded border border-[var(--border)] bg-[var(--bg-hover)] text-[var(--text-muted)] tnum">
              Ctrl+Enter
            </kbd>
            <Button variant="secondary" size="sm" onClick={onClose}>
              {t('home.cancel')}
            </Button>
            <Button variant="primary" size="sm" disabled={lines.length === 0} onClick={submit}>
              {t('home.batchSend')}
            </Button>
          </div>
        </div>
      </div>
    </div>
  )
}


/* ============ 会话导入预览弹层：解析 md/json → 只读预览 → 复制全文 ============
 * - 不写数据库（避免污染历史/触发 agent）
 * - 用户可复制后粘贴到任意会话作为参考上下文
 * - 顶部展示元信息（文件名 / 消息数 / 角色分布）+ 中间消息列表 + 底部"复制全文"按钮
 */
function ImportDialog({ data, onClose }: {
  data: { title: string; messages: Array<{ role: string; content: string }> }
  onClose: () => void
}) {
  const { t } = useTranslation()
  const [copied, setCopied] = useState(false)
  const copyRef = useRef<HTMLButtonElement>(null)

  // 复制全文为 markdown（与 exportConversationMd 输出格式兼容）
  const copyAll = async () => {
    const md = data.messages
      .map((m) => {
        const role = m.role === 'user' ? '👤 User' : m.role === 'assistant' ? '🤖 Assistant' : `⚙️ ${m.role}`
        return `## ${role}\n\n${m.content}`
      })
      .join('\n\n---\n\n')
    try {
      await navigator.clipboard.writeText(md)
      setCopied(true)
      setTimeout(() => setCopied(false), 1500)
    } catch (e) {
      console.error('copy failed', e)
    }
  }

  // 统计
  const stats = useMemo(() => {
    let user = 0, assistant = 0, other = 0
    for (const m of data.messages) {
      if (m.role === 'user') user++
      else if (m.role === 'assistant') assistant++
      else other++
    }
    return { user, assistant, other }
  }, [data.messages])

  useEscapeKey(onClose)

  return (
    <div className="cmdk-backdrop" onClick={onClose}>
      <div
        className="w-[720px] max-w-[94vw] max-h-[86vh] flex flex-col glass-card animate-modal-in"
        onClick={(e) => e.stopPropagation()}
      >
        {/* 标题栏 */}
        <div className="flex items-center justify-between px-4 h-12 border-b border-[var(--border)] shrink-0">
          <div className="flex items-center gap-2 min-w-0">
            <div className="w-6 h-6 rounded-md bg-[var(--accent)]/15 text-[var(--accent)] flex items-center justify-center shrink-0">
              <Icon name="file" size={13} />
            </div>
            <h2 className="text-[14px] font-semibold truncate" title={data.title}>{data.title}</h2>
            <span className="text-[10.5px] text-[var(--text-muted)] shrink-0 tnum">
              {t('home.importMsgCount', { count: data.messages.length })}
            </span>
          </div>
          <IconButton icon="close" label={t('home.cancel')} iconSize={13} className="h-6 w-6" onClick={onClose} />
        </div>

        {/* 角色分布小条 */}
        <div className="flex items-center gap-3 px-4 py-2 border-b border-[var(--border)] text-[10.5px] text-[var(--text-muted)] tnum shrink-0">
          <span className="flex items-center gap-1">
            <span className="w-1.5 h-1.5 rounded-full bg-[var(--accent)]" />
            {t('home.importRoleUser')}: {stats.user}
          </span>
          <span className="flex items-center gap-1">
            <span className="w-1.5 h-1.5 rounded-full bg-[var(--success)]" />
            {t('home.importRoleAssistant')}: {stats.assistant}
          </span>
          {stats.other > 0 && (
            <span className="flex items-center gap-1">
              <span className="w-1.5 h-1.5 rounded-full bg-[var(--text-muted)]" />
              {t('home.importRoleOther')}: {stats.other}
            </span>
          )}
          <span className="ml-auto text-[10px] text-[var(--text-muted)]">{t('home.importPreviewHint')}</span>
        </div>

        {/* 消息列表（只读） */}
        <div className="flex-1 overflow-y-auto p-4 space-y-3 min-h-0">
          {data.messages.map((m, i) => {
            const isUser = m.role === 'user'
            return (
              <div key={i} className={`flex gap-2.5 ${isUser ? 'flex-row-reverse' : ''}`}>
                <div
                  className={`shrink-0 w-6 h-6 rounded-md flex items-center justify-center text-[11px] ${
                    isUser
                      ? 'bg-[var(--accent)]/15 text-[var(--accent)]'
                      : 'bg-[var(--success)]/15 text-[var(--success)]'
                  }`}
                  title={m.role}
                >
                  {isUser ? '👤' : '🤖'}
                </div>
                <div
                  className={`flex-1 min-w-0 rounded-lg p-2.5 text-[12.5px] leading-relaxed whitespace-pre-wrap break-words modern-card ${
                    isUser ? 'bg-[var(--accent)]/5' : ''
                  }`}
                >
                  {m.content || <span className="text-[var(--text-muted)] italic">（空消息）</span>}
                </div>
              </div>
            )
          })}
        </div>

        {/* 底部操作栏 */}
        <div className="flex items-center justify-between px-4 h-12 border-t border-[var(--border)] shrink-0">
          <span className="text-[11px] text-[var(--text-muted)]">{t('home.importReadOnly')}</span>
          <div className="flex items-center gap-2">
            <Button variant="secondary" size="sm" onClick={onClose}>
              {t('home.cancel')}
            </Button>
            <Button
              variant="primary"
              size="sm"
              ref={copyRef}
              icon={copied ? 'check' : 'copy'}
              onClick={copyAll}
            >
              {copied ? t('home.importCopied') : t('home.importCopyAll')}
            </Button>
          </div>
        </div>
      </div>
    </div>
  )
}


/* ============ Pinned 消息条：会话顶部钉住关键消息（仅前端 localStorage） ============
 * - 会话长了之后找回"报错信息/关键决策/成功命令"很痛 → 让用户手动 pin
 * - 数据结构：usePinStore（per conversationId，localStorage 持久化，最多 8 条）
 * - 点 chip 滚动到该消息（onJump 回调）；点 × 取消 pin
 * - 静默跳过"已 pin 但消息被删"的孤儿 id
 */


/* ============ 会话笔记（顶部笔记区，per conversation 持久化） ============
 * - 默认收起（点 "📝 写笔记" 展开）
 * - 自动保存（debounce 600ms）
 * - 上限 4000 字（NOTE_MAX_LEN）
 * - 写过的会话：标题行显示 "📝" 标识 + 摘要预览
 * - 切换会话时表单值正确同步
 */
function ConvNoteBar({ convId }: { convId: string }) {
  const { t } = useTranslation()
  const note = useNoteStore((s) => s.notes[convId] ?? null)
  const setNote = useNoteStore((s) => s.set)
  const [editing, setEditing] = useState(false)
  const [draft, setDraft] = useState(note?.text ?? '')
  const debounceRef = useRef<number | null>(null)

  // 切换会话：同步当前 draft 到最新值
  useEffect(() => {
    setDraft(note?.text ?? '')
    setEditing(false)
  }, [convId, note?.text])

  // debounce 自动保存
  useEffect(() => {
    if (!editing) return
    if (draft === (note?.text ?? '')) return
    if (debounceRef.current) window.clearTimeout(debounceRef.current)
    debounceRef.current = window.setTimeout(() => {
      setNote(convId, draft)
    }, 600)
    return () => {
      if (debounceRef.current) window.clearTimeout(debounceRef.current)
    }
  }, [draft, editing, convId, note?.text, setNote])

  // 编辑模式：全屏 textarea
  if (editing) {
    return (
      <div className="mb-3 rounded-lg glass-card border border-[var(--accent)]/30 p-2.5 animate-fade-in">
        <div className="flex items-center justify-between mb-1.5">
          <div className="flex items-center gap-1.5 text-[10.5px] font-medium text-[var(--accent)]">
            <Icon name="edit" size={11} />
            {t('home.noteEditingTitle')}
          </div>
          <div className="flex items-center gap-2">
            <span className={`text-[10px] tnum ${draft.length > NOTE_MAX_LEN * 0.9 ? 'text-[var(--warning)]' : 'text-[var(--text-muted)]'}`}>
              {draft.length}/{NOTE_MAX_LEN}
            </span>
            <Button
              variant="primary"
              size="xs"
              onClick={() => {
                setNote(convId, draft)
                setEditing(false)
              }}
            >
              {t('home.noteDone')}
            </Button>
          </div>
        </div>
        <textarea
          value={draft}
          onChange={(e) => setDraft(e.target.value.slice(0, NOTE_MAX_LEN))}
          rows={4}
          autoFocus
          spellCheck={false}
          className="w-full resize-y rounded-md bg-[var(--bg-primary)] border border-[var(--border)] px-2.5 py-1.5 text-[12.5px] leading-relaxed outline-none focus:border-[var(--accent)] min-h-[80px] max-h-[200px]"
          placeholder={t('home.notePlaceholder')}
        />
      </div>
    )
  }

  // 折叠态：未写过 → 灰提示；写过 → 显示摘要 + 时间
  return (
    <div
      className={`mb-2 rounded-lg border transition-colors ${
        note
          ? 'border-[var(--accent)]/20 bg-[var(--accent)]/5'
          : 'border-dashed border-[var(--border)] bg-[var(--bg-secondary)]/40 hover:border-[var(--accent)]/30'
      }`}
    >
      <button
        onClick={() => setEditing(true)}
        className="w-full flex items-center gap-2 px-2.5 py-1.5 text-left"
      >
        <Icon name="edit" size={11} className={note ? 'text-[var(--accent)]' : 'text-[var(--text-muted)]'} />
        {note ? (
          <div className="flex-1 min-w-0">
            <div className="text-[11.5px] text-[var(--text-primary)] truncate">{note.text.split('\n')[0]}</div>
            <div className="text-[10px] text-[var(--text-muted)] tnum mt-0.5">
              {t('home.noteUpdated', { time: new Date(note.updatedAt).toLocaleString() })}
            </div>
          </div>
        ) : (
          <span className="text-[11.5px] text-[var(--text-muted)] flex-1">{t('home.noteAdd')}</span>
        )}
        {note && (
          <span
            onClick={(e) => {
              e.stopPropagation()
              if (window.confirm(t('home.noteClearConfirm'))) {
                useNoteStore.getState().clear(convId)
              }
            }}
            className="p-1 text-[var(--text-muted)] hover:text-[var(--danger)] opacity-0 group-hover:opacity-100"
            title={t('home.noteClear')}
          >
            <Icon name="close" size={11} />
          </span>
        )}
      </button>
    </div>
  )
}
function PinnedBar({ convId, onJump }: { convId: string; onJump: (msgId: string) => void }) {
  const { t } = useTranslation()
  // ⚠️ ?? [] 必须在 selector 外：写在 selector 内时无 pin 会话每次返回新数组引用，
  // useSyncExternalStore 判定快照持续变化 → 无限重渲染（Maximum update depth exceeded）
  const pins = usePinStore((s) => s.pins[convId]) ?? []
  const unpin = usePinStore((s) => s.unpin)
  // 从 store 取消息（避免 prop drilling）
  const messages = useProjectStore((s) => s.messages)
  if (pins.length === 0) return null
  // 过滤已删除的消息 + 按 pin 顺序展示
  const items = pins
    .map((id) => messages.find((m) => m.id === id))
    .filter((m): m is NonNullable<typeof m> => m != null)
  if (items.length === 0) return null
  return (
    <div className="mb-3 rounded-lg glass-card p-2 animate-fade-in">
      <div className="flex items-center gap-1.5 px-1 mb-1.5">
        <Icon name="pin" size={11} className="text-[var(--accent)]" />
        <span className="text-[10.5px] font-medium text-[var(--text-muted)] uppercase tracking-wider">
          {t('home.pinnedTitle')} · {items.length}
        </span>
      </div>
      <div className="flex flex-wrap gap-1.5">
        {items.map((m) => {
          const isUser = m.role === 'user'
          const preview = m.content.replace(/[#*`>_~\n]+/g, ' ').replace(/\s+/g, ' ').trim().slice(0, 60)
          const tone = isUser ? 'var(--accent)' : 'var(--success)'
          return (
            <div
              key={m.id}
              className="group flex items-center gap-1.5 max-w-[280px] rounded-md bg-[var(--bg-card)] border border-[var(--border)] hover:border-[var(--accent)]/40 transition-colors"
            >
              <button
                onClick={() => onJump(m.id)}
                className="flex-1 min-w-0 flex items-center gap-1.5 px-2 py-1 text-left"
                title={m.content}
              >
                <span
                  className="shrink-0 w-1.5 h-1.5 rounded-full"
                  style={{ background: tone }}
                />
                <span className="text-[10.5px] text-[var(--text-muted)] shrink-0">
                  {isUser ? '👤' : '🤖'}
                </span>
                <span className="text-[11.5px] text-[var(--text-primary)] truncate">
                  {preview || t('home.pinnedEmpty')}
                </span>
              </button>
              <button
                onClick={() => {
                  unpin(convId, m.id)
                  void setConversationContextPin({
                    conversation_id: convId,
                    pin_kind: 'message',
                    source_ref: `message:${m.id}`,
                    label: m.role,
                    content: m.content,
                    pinned: false,
                  }).catch(() => {})
                }}
                className="opacity-0 group-hover:opacity-100 p-1 text-[var(--text-muted)] hover:text-[var(--danger)] transition-all shrink-0"
                title={t('home.unpinFromTop')}
              >
                <Icon name="close" size={10} />
              </button>
            </div>
          )
        })}
      </div>
    </div>
  )
}


/* ============ 通用确认弹层：替代 window.confirm，支持危险级别 + 自定义文案 ============
 * - tone: danger（红）/ warn（黄）/ info（蓝）
 * - confirmLabel: 主按钮文案（默认"确定"）
 * - requireInput: 需要用户输入指定短语才解锁确认按钮（最严级）
 * - 用法：调用方用 state 持有回调函数 + 参数 → 渲染时挂 onConfirm/onCancel
 */
function ConfirmDialog({ open, title, body, tone = 'danger', confirmLabel, cancelLabel, requireInput, onConfirm, onCancel }: {
  open: boolean
  title: string
  body: string | React.ReactNode
  tone?: 'danger' | 'warn' | 'info'
  confirmLabel?: string
  cancelLabel?: string
  /** 若提供此短语，用户必须在输入框中键入完全匹配的字符串才解锁确认 */
  requireInput?: string
  onConfirm: () => void
  onCancel: () => void
}) {
  const { t } = useTranslation()
  const [typed, setTyped] = useState('')
  const inputRef = useRef<HTMLInputElement>(null)
  const confirmBtnRef = useRef<HTMLButtonElement>(null)

  // 重置输入态
  useEffect(() => {
    if (open) {
      setTyped('')
      setTimeout(() => (requireInput ? inputRef.current?.focus() : confirmBtnRef.current?.focus()), 30)
    }
  }, [open, requireInput])

  // Esc 取消 / Enter 确认
  useEffect(() => {
    if (!open) return
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.preventDefault()
        onCancel()
      } else if (e.key === 'Enter' && !requireInput) {
        e.preventDefault()
        onConfirm()
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [open, onConfirm, onCancel, requireInput])

  if (!open) return null
  const canConfirm = !requireInput || typed === requireInput
  const accent = tone === 'danger' ? 'var(--danger)' : tone === 'warn' ? 'var(--warning)' : 'var(--accent)'
  const confirmStyle = canConfirm
    ? { background: accent, color: '#fff' }
    : { background: 'var(--bg-hover)', color: 'var(--text-muted)' }

  return (
    <div className="cmdk-backdrop" onClick={onCancel}>
      <div
        className="w-[440px] max-w-[92vw] glass-card p-4 animate-modal-in"
        onClick={(e) => e.stopPropagation()}
      >
        {/* 图标 + 标题 */}
        <div className="flex items-start gap-3 mb-3">
          <div
            className="w-9 h-9 rounded-lg flex items-center justify-center shrink-0"
            style={{ background: `${accent}20`, color: accent }}
          >
            <Icon name={tone === 'info' ? 'info' : 'archive'} size={18} />
          </div>
          <div className="flex-1 min-w-0">
            <h3 className="text-[14px] font-semibold leading-snug">{title}</h3>
            <div className="mt-1.5 text-[12.5px] text-[var(--text-secondary)] leading-relaxed">{body}</div>
          </div>
        </div>

        {/* 危险操作需要用户输入确认短语（防误触） */}
        {requireInput && (
          <div className="mb-3">
            <label className="block text-[11px] text-[var(--text-muted)] mb-1.5">
              {t('home.confirmTypePhrase', { phrase: requireInput })}
            </label>
            <input
              ref={inputRef}
              value={typed}
              onChange={(e) => setTyped(e.target.value)}
              spellCheck={false}
              autoComplete="off"
              className="w-full rounded-lg modern-card px-2.5 py-1.5 text-[12.5px] font-mono outline-none focus:border-[var(--accent)]"
              placeholder={requireInput}
            />
          </div>
        )}

        {/* 操作按钮 */}
        <div className="flex items-center justify-end gap-2 mt-1">
          <button
            onClick={onCancel}
            className="h-8 px-3 rounded-lg border border-[var(--border)] text-[12.5px] hover:bg-[var(--bg-hover)] transition-colors"
          >
            {cancelLabel ?? t('home.cancel')}
          </button>
          <button
            ref={confirmBtnRef}
            onClick={onConfirm}
            disabled={!canConfirm}
            style={confirmStyle}
            className="h-8 px-3 rounded-lg text-[12.5px] font-medium disabled:cursor-not-allowed transition-colors"
          >
            {confirmLabel ?? t('home.confirm')}
          </button>
        </div>
      </div>
    </div>
  )
}


/* ============ 审计日志查看页：表格列出所有敏感操作（localStorage 持久化，上限 200） ============
 * - 顶部：标题 + 清空按钮 + 分类过滤 chips
 * - 主体：时间 / 分类 / 名称 / 详情（按时间倒序）
 * - 底部：记录条数
 */
function AuditDialog({ onClose }: { onClose: () => void }) {
  const { t } = useTranslation()
  const entries = useAuditStore((s) => s.entries)
  const clear = useAuditStore((s) => s.clear)
  const [filter, setFilter] = useState<'all' | AuditCategory>('all')

  useEscapeKey(onClose)

  const filtered = useMemo(
    () => (filter === 'all' ? entries : entries.filter((e) => e.category === filter)).slice().sort((a, b) => b.ts - a.ts),
    [entries, filter],
  )

  // 分类 chips 计数
  const counts = useMemo(() => {
    const map: Record<string, number> = { all: entries.length }
    for (const e of entries) map[e.category] = (map[e.category] ?? 0) + 1
    return map
  }, [entries])

  // 时间格式：YYYY-MM-DD HH:mm:ss
  const fmtTs = (ts: number) => {
    const d = new Date(ts)
    const pad = (n: number) => String(n).padStart(2, '0')
    return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`
  }

  // 分类 → i18n label
  const catLabel = (c: AuditCategory) => t(`home.auditCat.${c}`)

  return (
    <div className="cmdk-backdrop" onClick={onClose}>
      <div
        className="w-[820px] max-w-[94vw] max-h-[86vh] flex flex-col glass-card animate-modal-in"
        onClick={(e) => e.stopPropagation()}
      >
        {/* 标题栏 */}
        <div className="flex items-center justify-between px-4 h-12 border-b border-[var(--border)] shrink-0">
          <div className="flex items-center gap-2">
            <div className="w-6 h-6 rounded-md bg-[var(--accent)]/15 text-[var(--accent)] flex items-center justify-center">
              <Icon name="history" size={13} />
            </div>
            <h2 className="text-[14px] font-semibold">{t('home.auditTitle')}</h2>
            <span className="text-[10.5px] text-[var(--text-muted)] tnum">
              {t('home.auditCount', { count: entries.length })}
            </span>
          </div>
          <div className="flex items-center gap-2">
            <Button
              variant="danger"
              size="sm"
              confirm
              confirmLabel={t('common.confirmAgain')}
              title={t('home.auditClearConfirm')}
              disabled={entries.length === 0}
              onClick={() => {
                if (entries.length === 0) return
                clear()
              }}
            >
              {t('home.auditClear')}
            </Button>
            <IconButton icon="close" label={t('home.cancel')} iconSize={13} className="h-6 w-6" onClick={onClose} />
          </div>
        </div>

        {/* 分类过滤 chips */}
        <div className="flex items-center gap-1.5 px-4 py-2 border-b border-[var(--border)] overflow-x-auto shrink-0">
          <button
            onClick={() => setFilter('all')}
            className={`shrink-0 px-2 py-0.5 rounded-md text-[11px] transition-colors ${
              filter === 'all' ? 'tab-active' : 'tab-inactive'
            }`}
          >
            {t('home.auditCatAll')} ({counts.all ?? 0})
          </button>
          {(Object.keys(counts) as string[]).filter((c) => c !== 'all' && counts[c] > 0).map((c) => (
            <button
              key={c}
              onClick={() => setFilter(c as AuditCategory)}
              className={`shrink-0 px-2 py-0.5 rounded-md text-[11px] transition-colors ${
                filter === c ? 'tab-active' : 'tab-inactive'
              }`}
            >
              {catLabel(c as AuditCategory)} ({counts[c]})
            </button>
          ))}
        </div>

        {/* 列表 */}
        <div className="flex-1 overflow-y-auto min-h-0">
          {filtered.length === 0 ? (
            <p className="px-4 py-12 text-center text-[13px] text-[var(--text-muted)]">
              {t('home.auditEmpty')}
            </p>
          ) : (
            <table className="w-full text-[12px]">
              <thead className="sticky top-0 bg-[var(--bg-card)] z-10">
                <tr className="border-b border-[var(--border)] text-[var(--text-muted)]">
                  <th className="text-left px-3 py-2 font-medium">{t('home.auditTime')}</th>
                  <th className="text-left px-3 py-2 font-medium">{t('home.auditCategory')}</th>
                  <th className="text-left px-3 py-2 font-medium">{t('home.auditName')}</th>
                  <th className="text-left px-3 py-2 font-medium">{t('home.auditDetail')}</th>
                </tr>
              </thead>
              <tbody>
                {filtered.map((e) => (
                  <tr key={e.id} className="border-b border-[var(--border)] last:border-0 align-top hover:bg-[var(--bg-hover)]">
                    <td className="px-3 py-2 whitespace-nowrap font-mono text-[11px] text-[var(--text-secondary)] tnum">{fmtTs(e.ts)}</td>
                    <td className="px-3 py-2">
                      <span className="px-1.5 py-0.5 rounded text-[10.5px] bg-[var(--accent)]/10 text-[var(--accent)]">
                        {catLabel(e.category)}
                      </span>
                    </td>
                    <td className="px-3 py-2 max-w-[180px] truncate" title={e.label}>{e.label}</td>
                    <td className="px-3 py-2 max-w-[320px] text-[var(--text-secondary)]">
                      {e.detail ? (
                        <span className="line-clamp-2" title={e.detail}>{e.detail}</span>
                      ) : (
                        <span className="text-[var(--text-muted)]">—</span>
                      )}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </div>
      </div>
    </div>
  )
}


/* ============ 设置菜单项 ============ */
const settingsItems: { path: string; labelKey: string; icon: IconName }[] = [
  { path: '/lan', labelKey: 'nav.lan', icon: 'devices' },
  { path: '/providers', labelKey: 'nav.provider', icon: 'bolt' },
  { path: '/versions', labelKey: 'nav.version', icon: 'package' },
  { path: '/config', labelKey: 'nav.config', icon: 'settings' },
  { path: '/limits', labelKey: 'nav.limits', icon: 'tune' },
  { path: '/cost', labelKey: 'nav.cost', icon: 'payments' },
  { path: '/proxy', labelKey: 'nav.proxy', icon: 'proxy' },
  { path: '/mcp', labelKey: 'nav.mcp', icon: 'mcp' },
  { path: '/skills', labelKey: 'nav.skill', icon: 'skill' },
  { path: '/team-sharing', labelKey: 'nav.teamSharing', icon: 'skill' },
  { path: '/reproduction-bundles', labelKey: 'nav.reproductionBundles', icon: 'archive' },
  { path: '/knowledge', labelKey: 'nav.knowledge', icon: 'skill' },
  { path: '/api-knowledge', labelKey: 'nav.apiKnowledge', icon: 'package' },
  { path: '/health', labelKey: 'nav.health', icon: 'health' },
  { path: '/ohpm', labelKey: 'nav.ohpm', icon: 'apps' },
]
