import { listen } from '@tauri-apps/api/event'
import { sendNotification } from '../../api/desktop'
import {
  listConversations,
  getConversation,
  createConversation,
  forkConversation,
  listMessages,
  listMessagesPage,
  streamChat as streamChatApi,
  stopChat as stopChatApi,
  stopTool as stopToolApi,
  queueMessage as queueMessageApi,
  listQueuedMessages as listQueuedMessagesApi,
  removeQueuedMessage as removeQueuedMessageApi,
  updateMessage as updateMessageApi,
  deleteMessage as deleteMessageApi,
  resolveToolApproval as resolveToolApprovalApi,
  resolvePlanReview as resolvePlanReviewApi,
  getTodos as getTodosApi,
  getTaskLedger as getTaskLedgerApi,
  getAsk as getAskApi,
  listPendingConfirmations as listPendingConfirmationsApi,
  resolveAskUser as resolveAskUserApi,
  renameConversation as renameConversationApi,
  deleteConversation as deleteConversationApi,
  pinConversation as pinConversationApi,
  archiveConversation as archiveConversationApi,
  rollbackConversation,
  conversationRoot,
  listSnapshots as listSnapshotsApi,
  restoreSnapshot as restoreSnapshotApi,
} from '../../api/project'
import type { ChatMessage, TodoItem, PendingConfirmation, TaskLedger } from '../../api/project'
import type { StateCreator } from 'zustand'
import type { ChatSlice, DiagnoseCard, ProjectState, StreamingState } from '../projectStoreTypes'
import { acceptsRunEvent, advancePlan, firstRunningIndex, reconcileRunUserMessage, upsertMessageById } from './chatUtils'
import { startPerfTrace, waitForNextPaint } from '../../utils/perfTrace'
import { setItem } from '../../utils/storage'
import { STORAGE_KEYS } from '../../constants'

/**
 * agent:log 高频日志的 rAF 批处理缓冲。
 * hvigor/hdc 在构建/部署期可每秒输出数百上千行，若逐行 set 三份大状态并牵连 Home
 * 整树重渲染，会把主线程占满导致交互卡死。这里把日志行攒到缓冲，用 requestAnimationFrame
 * 每帧（约 16ms）合并一次 set，把每秒数百次更新压到约 60 次，且一次只做一次字符串拼接。
 */
type PendingLogLine = { stream: string; line: string; ts: number; convId: string }
let pendingLogLines: PendingLogLine[] = []
let logFlushScheduled = false
let logFlushTimer: ReturnType<typeof setTimeout> | null = null
/** buildLogs 全局自增 id，用作稳定 React key（清屏后归零无妨，key 仅在当前列表内唯一） */
let buildLogSeq = 0
const nextBuildLogId = () => ++buildLogSeq

/** localStorage key 前缀：持久化每个项目上次打开的会话 ID（统一见 src/constants.ts 的 STORAGE_KEYS.LAST_CONV_PREFIX） */

/** 看门狗静默阈值：超过该时长无内容/思考增量且无运行中工具/挂起审批，判定后端挂起 */
const STREAM_WATCHDOG_MS = 4 * 60 * 1000
/** 已有内容/思考的桶放宽阈值：后端 60s 静默判死+主循环续写会刷新 lastDeltaAt，
 * 8 分钟无事件必为后端 join 永不返回（invoke 永不 reject），前端必须自行释放 */
const STREAM_WATCHDOG_LONG_MS = 8 * 60 * 1000
/** 定时器自身停顿超过该值通常意味着系统休眠或 WebView 被操作系统冻结。 */
const WATCHDOG_RESUME_GAP_MS = 90 * 1000
let lastWatchdogSweepAt = Date.now()

// Agent 工具 / 子 Agent 事件自增序号（模块级）
let toolSeq = 0
let agentSeq = 0
/** 快速连续切换会话时，只允许最后一次 openConversation 请求落地。 */
let openConversationSeq = 0

/** Provider 往往按 token 发送 Tauri 事件；按动画帧合并状态更新，避免长回答压满 WebView2 主线程。 */
type PendingStreamDelta = { content: string; reasoning: string }
const pendingStreamDeltas = new Map<string, PendingStreamDelta>()
let streamFlushScheduled = false
let streamFlushTimer: ReturnType<typeof setTimeout> | null = null

/** 后台标签页的 rAF 会被限频；给尚未刷新的日志设硬上限，防事件洪峰无限占用内存。 */
const MAX_PENDING_LOG_LINES = 4000

/** 发送消息性能追踪：convId → trace（首个 delta 标记 TTFB，chat-done 结束） */
const sendTraces = new Map<string, ReturnType<typeof startPerfTrace>>()
/** 普通发送的乐观 user 占位；终态用真实 DB id 精确替换，不能误改随后排队的 local 消息。 */
const optimisticRunUserIds = new Map<string, string>()
/** 最近终态代次墓碑：拦截监管线程/IPC 队列在 done/error/stopped 后迟到的 started/heartbeat。 */
const terminalRunIds = new Map<string, string>()

const markRunTerminal = (conversationId: string, runId?: string) => {
  if (!runId) return
  terminalRunIds.set(conversationId, runId)
  setTimeout(() => {
    if (terminalRunIds.get(conversationId) === runId) terminalRunIds.delete(conversationId)
  }, 60 * 1000)
}

/** 空流式状态 */
const emptyStreaming = (): StreamingState => ({
  conversationId: null,
  runId: null,
  content: '',
  reasoning: '',
  error: null,
  errorDetail: null,
  startedAt: null,
  lastDeltaAt: null,
  toolRunning: 0,
})

/** 会话/消息/流式/审批/计划切片实现（含全局事件监听注册，store 创建时执行一次） */
export const createChatSlice: StateCreator<ProjectState, [], [], ChatSlice> = (set, get) => {
  /**
   * 流式分桶更新：真源 streamings[convId]，streaming 派生为当前会话的桶（无则空态）。
   * patch=null 删除分桶（任务结束）。事件监听不再依赖"当前流式会话"判断——
   * 后台会话（用户已切走）的增量/结束也写入自己的桶，切回时内容不丢。
   */
  const setBucket = (convId: string, patch: Partial<StreamingState> | null) =>
    set((s) => {
      const streamings = { ...s.streamings }
      if (patch === null) {
        delete streamings[convId]
      } else {
        const prev = streamings[convId]
        // 出错时视为流式结束：清空 conversationId（isStreaming 变 false）与 startedAt
        // （停止计时），但保留已生成 content/reasoning 与 error，供错误卡和部分内容共存展示，
        // 避免打字光标/三点动画与错误卡同时出现。
        const isError = patch.error !== undefined && patch.error !== null
        streamings[convId] = {
          ...(prev ?? { ...emptyStreaming(), startedAt: Date.now() }),
          ...patch,
          conversationId: isError ? null : convId,
          startedAt: isError ? null : (patch.startedAt ?? prev?.startedAt ?? null),
          lastDeltaAt: isError ? (prev?.lastDeltaAt ?? null) : Date.now(),
        }
      }
      return {
        streamings,
        streaming: s.currentConversation ? streamings[s.currentConversation.id] ?? emptyStreaming() : emptyStreaming(),
      }
    })

  /** 把一个或全部会话积攒的正文/思考增量一次性写入 Zustand。 */
  const flushStreamDeltas = (onlyConversationId?: string) => {
    if (!onlyConversationId) {
      streamFlushScheduled = false
      if (streamFlushTimer !== null) {
        clearTimeout(streamFlushTimer)
        streamFlushTimer = null
      }
    }
    if (pendingStreamDeltas.size === 0) return
    const batch = new Map<string, PendingStreamDelta>()
    if (onlyConversationId) {
      const delta = pendingStreamDeltas.get(onlyConversationId)
      if (!delta) return
      batch.set(onlyConversationId, delta)
      pendingStreamDeltas.delete(onlyConversationId)
    } else {
      for (const entry of pendingStreamDeltas) batch.set(...entry)
      pendingStreamDeltas.clear()
    }
    set((s) => {
      let changed = false
      const streamings = { ...s.streamings }
      const now = Date.now()
      for (const [convId, delta] of batch) {
        const bucket = streamings[convId]
        if (!bucket) continue
        streamings[convId] = {
          ...bucket,
          content: bucket.content + delta.content,
          reasoning: bucket.reasoning + delta.reasoning,
          lastDeltaAt: now,
        }
        changed = true
      }
      if (!changed) return {}
      return {
        streamings,
        streaming: s.currentConversation ? streamings[s.currentConversation.id] ?? emptyStreaming() : emptyStreaming(),
      }
    })
  }

  const queueStreamDelta = (convId: string, kind: keyof PendingStreamDelta, delta: string) => {
    const prev = pendingStreamDeltas.get(convId) ?? { content: '', reasoning: '' }
    pendingStreamDeltas.set(convId, { ...prev, [kind]: prev[kind] + delta })
    if (!streamFlushScheduled) {
      streamFlushScheduled = true
      requestAnimationFrame(() => flushStreamDeltas())
      // macOS/Windows 窗口最小化或失焦后 rAF 可能降到极低频率；定时器保证流式
      // 状态仍持续收敛，避免缓冲一直增长、切回窗口时一次性大渲染。
      streamFlushTimer = setTimeout(() => flushStreamDeltas(), 50)
    }
  }

  /** 新任务开始后，任何旧 run_id 的延迟事件都不得修改当前桶。空 run_id 兼容旧 LAN 客户端。 */
  const acceptsRun = (convId: string, runId?: string) => {
    const bucket = get().streamings[convId]
    if (!bucket) return false
    return acceptsRunEvent(bucket.runId, runId)
  }

  /**
   * 按会话维护待确认项（审批/计划/提问）：后台会话事件不再丢弃，统一记录到
   * pendingConfirmations，会话列表据此渲染“待确认”角标，切回会话时据此恢复弹窗/卡片。
   */
  const upsertPending = (item: PendingConfirmation) =>
    set((s) => {
      const arr = s.pendingConfirmations[item.conversation_id] ?? []
      const exists = arr.some((p) => p.request_id === item.request_id)
      const next = exists
        ? arr.map((p) => (p.request_id === item.request_id ? item : p))
        : [...arr, item]
      return { pendingConfirmations: { ...s.pendingConfirmations, [item.conversation_id]: next } }
    })

  /** 按 request_id 从待确认表中移除一项（审批/计划/提问答复后调用） */
  const removePendingByRequestId = (requestId: string) =>
    set((s) => {
      let changed = false
      const next: Record<string, PendingConfirmation[]> = {}
      for (const [cid, arr] of Object.entries(s.pendingConfirmations)) {
        const filtered = arr.filter((p) => p.request_id !== requestId)
        if (filtered.length !== arr.length) changed = true
        if (filtered.length > 0) next[cid] = filtered
      }
      return changed ? { pendingConfirmations: next } : {}
    })

  /**
   * 后端异常兜底看门狗（多会话安全）：周期巡检所有流式分桶，静默超阈值
   * （无内容/思考增量、无运行中工具、当前会话无挂起审批）的桶判定后端挂起，
   * 自动置错释放——防止会话永久停留在"正在输入/后台生成中"。
   * chat-tool-start/done 按会话维护 toolRunning 计数，长任务/后台会话不误杀；
   * chat-done/error/stopped 清桶后自然不再触发。
   */
  setInterval(() => {
    const s = get()
    const now = Date.now()
    const sweepGap = now - lastWatchdogSweepAt
    lastWatchdogSweepAt = now
    // 系统休眠/窗口冻结期间 Date.now 继续前进，恢复后不能把所有任务立刻判死。
    // 后端独立看门狗会继续做权威判断；前端仅重置展示层活性宽限。
    if (sweepGap > WATCHDOG_RESUME_GAP_MS) {
      set((current) => {
        const streamings = Object.fromEntries(
          Object.entries(current.streamings).map(([id, bucket]) => [
            id,
            bucket.error ? bucket : { ...bucket, lastDeltaAt: now },
          ]),
        )
        return {
          streamings,
          streaming: current.currentConversation
            ? streamings[current.currentConversation.id] ?? emptyStreaming()
            : emptyStreaming(),
        }
      })
      return
    }
    for (const [convId, bucket] of Object.entries(s.streamings)) {
      if (bucket.error) continue
      const ref = bucket.lastDeltaAt ?? bucket.startedAt
      if (!ref) continue
      // 已有内容/思考的桶：后端 60s 静默判死+续写会刷新 lastDeltaAt，8 分钟无事件
      // 必为后端 join 永不返回（invoke 永不 reject，前端永久转圈），须放宽阈值兜底释放；
      // 无内容桶保持 4 分钟（首字节前的卡死更早暴露）
      const threshold = bucket.content || bucket.reasoning ? STREAM_WATCHDOG_LONG_MS : STREAM_WATCHDOG_MS
      if (now - ref < threshold) continue
      // 工具执行中（含后台会话）：后端仍在工作
      if (bucket.toolRunning > 0) continue
      const isCurrent = s.currentConversation?.id === convId
      // 任意会话（含后台）有挂起的审批/计划/提问：在等用户操作，不判挂起。
      // pendingConfirmations 按会话聚合，后台会话弹窗也会记录，避免误杀。
      const pending = s.pendingConfirmations[convId]
      if (pending && pending.length > 0) continue
      // 当前会话视图级挂起（兜底，历史数据无 pendingConfirmations 时）
      if (isCurrent && s.toolApprovals.length > 0) continue
      setBucket(convId, {
        ...emptyStreaming(),
        error: '后端长时间无响应，已自动停止等待。请检查模型配置与网络后重试',
      })
      // UI 释放必须与后端任务收敛同步；否则用户重试时旧任务仍持有项目锁，
      // 只会得到“已有任务进行中”，形成假恢复。停止命令失败仍由后端看门狗兜底。
      void stopChatApi(convId).catch(() => {})
      const trace = sendTraces.get(convId)
      if (trace) {
        sendTraces.delete(convId)
        trace.mark('watchdog-timeout')
        trace.end()
      }
      if (isCurrent) {
        set({
          toolRuns: [],
          agentRuns: [],
          // 超时停止：进度卡定档保留（展示已完成部分）
          plan: s.plan && s.plan.phase === 'running' ? { ...s.plan, phase: 'error' } : s.plan,
        })
      }
    }
    // 周期同步待确认项：后端超时会自动清除审批/计划/提问，前端角标需跟随刷新避免残留。
    // 仅在存在待确认项时刷新（无待确认时不可能有残留角标），查询开销可忽略。
    if (Object.keys(s.pendingConfirmations).length > 0) {
      get().refreshPendingConfirmations().catch(() => {})
    }
  }, 30 * 1000)

  // ---------- 流式事件监听（全局一次性注册） ----------
  listen<{ conversation_id: string; run_id: string }>('chat-run-started', (event) => {
    const { conversation_id, run_id } = event.payload
    if (terminalRunIds.get(conversation_id) === run_id) return
    // 不同代次是真正的新任务，旧墓碑不应阻挡。
    terminalRunIds.delete(conversation_id)
    const bucket = get().streamings[conversation_id]
    if (bucket) {
      // 活跃桶已有代次时，延迟/重复的 started 不能接管当前运行。
      if (bucket.runId && bucket.runId !== run_id) return
      // 新代次是权威边界：清除可能由已终止旧任务残留在帧队列中的增量。
      pendingStreamDeltas.delete(conversation_id)
      setBucket(conversation_id, { runId: run_id })
      return
    }
    pendingStreamDeltas.delete(conversation_id)
    // 排队消息由同一个后端任务壳自动续跑：上一轮 chat-done 已清掉分桶，下一轮
    // started 必须重建分桶，否则模型真实在后台继续工作而 UI 永远看不到输出。
    const fresh: StreamingState = {
      ...emptyStreaming(),
      conversationId: conversation_id,
      runId: run_id,
      startedAt: Date.now(),
      lastDeltaAt: Date.now(),
    }
    setBucket(conversation_id, fresh)
    if (get().currentConversation?.id === conversation_id) {
      set({
        toolRuns: [],
        terminalEntries: [],
        buildLogs: [],
        agentRuns: [],
        plan: null,
        todos: [],
        askCard: null,
        lastTaskSummary: null,
      })
    }
  }).catch(() => {})

  // 后端监管线程每 5 秒发一次轻量心跳：刷新前端看门狗，并让 WebView 重载后
  // 自动重建活跃桶继续接收后续增量/终态，不要求用户重启或手动“继续任务”。
  listen<{ conversation_id: string; run_id?: string; phase: string; started_at: number }>('chat-heartbeat', (event) => {
    const { conversation_id, run_id, started_at } = event.payload
    if (run_id && terminalRunIds.get(conversation_id) === run_id) return
    const bucket = get().streamings[conversation_id]
    if (bucket?.runId && run_id && bucket.runId !== run_id) return
    if (bucket) {
      setBucket(conversation_id, {
        runId: run_id || bucket.runId,
        error: null,
        errorDetail: null,
      })
      return
    }
    setBucket(conversation_id, {
      ...emptyStreaming(),
      conversationId: conversation_id,
      runId: run_id || null,
      startedAt: started_at || Date.now(),
      lastDeltaAt: Date.now(),
    })
  }).catch(() => {})

  // 增量写入分桶（不要求是当前会话）：用户切走再切回，流式内容不丢
  listen<{ conversation_id: string; run_id?: string; delta: string }>('chat-stream', (event) => {
    const { conversation_id, run_id, delta } = event.payload
    if (!acceptsRun(conversation_id, run_id)) return
    const bucket = get().streamings[conversation_id]
    if (!bucket) return
    const pending = pendingStreamDeltas.get(conversation_id)
    // 首个内容 delta：标记 TTFB（首个 token 到达时间）
    if (!bucket.content && !bucket.reasoning && !pending?.content && !pending?.reasoning) {
      const t = sendTraces.get(conversation_id)
      t?.mark('ttfb')
    }
    queueStreamDelta(conversation_id, 'content', delta)
  }).catch(() => {})

  // Rust 侧已按 32ms/8KB 合并模型 token，进一步降低 Tauri IPC 压力；
  // 仍进入同一个前端帧级队列，统一保证后台窗口和多会话行为。
  listen<{ conversation_id: string; run_id?: string; content: string; reasoning: string }>('chat-stream-batch', (event) => {
    const { conversation_id, run_id, content, reasoning } = event.payload
    if (!acceptsRun(conversation_id, run_id)) return
    const bucket = get().streamings[conversation_id]
    if (!bucket) return
    const pending = pendingStreamDeltas.get(conversation_id)
    if (!bucket.content && !bucket.reasoning && !pending?.content && !pending?.reasoning && (content || reasoning)) {
      sendTraces.get(conversation_id)?.mark('ttfb')
    }
    if (content) queueStreamDelta(conversation_id, 'content', content)
    if (reasoning) queueStreamDelta(conversation_id, 'reasoning', reasoning)
  }).catch(() => {})

  // 思考过程流式增量（推理模型 reasoning_content 透传；同样分桶）
  listen<{ conversation_id: string; run_id?: string; delta: string }>('chat-reasoning', (event) => {
    const { conversation_id, run_id, delta } = event.payload
    if (!acceptsRun(conversation_id, run_id)) return
    const bucket = get().streamings[conversation_id]
    if (!bucket) return
    const pending = pendingStreamDeltas.get(conversation_id)
    // 首个思考 delta 也算 TTFB（推理模型可能先吐 thinking 再吐 content）
    if (!bucket.content && !bucket.reasoning && !pending?.content && !pending?.reasoning) {
      const t = sendTraces.get(conversation_id)
      t?.mark('ttfb')
    }
    queueStreamDelta(conversation_id, 'reasoning', delta)
  }).catch(() => {})

  listen<{ conversation_id: string; run_id?: string; message: ChatMessage; unfinished: boolean; user_message_id?: string | null }>('chat-done', (event) => {
    const { conversation_id, run_id, message, unfinished, user_message_id } = event.payload
    if (!acceptsRun(conversation_id, run_id)) return
    markRunTerminal(conversation_id, run_id)
    flushStreamDeltas(conversation_id)
    const state = get()
    // 分桶清理无条件执行（后台会话结束也释放流式槽位）；视图状态仅当前会话更新
    const bucket = state.streamings[conversation_id]
    if (!bucket) return
    const isCurrent = state.currentConversation?.id === conversation_id
    const durationMs = bucket.startedAt ? Date.now() - bucket.startedAt : 0
    const fileCount = (() => {
      try {
        const v = message.modified_files_json ? JSON.parse(message.modified_files_json) : []
        return Array.isArray(v) ? v.length : 0
      } catch {
        return 0
      }
    })()
    setBucket(conversation_id, null)
    // 结束发送性能追踪
    const sendTrace = sendTraces.get(conversation_id)
    if (sendTrace) {
      sendTraces.delete(conversation_id)
      sendTrace.mark('done')
      sendTrace.end()
    }
    if (isCurrent) {
      // 任务结束摘要（ChatGPT 式收尾统计）：耗时 + 工具调用数 + 修改文件数
      const toolCount = state.toolRuns.length
      // 用后端返回的真实 user 消息 ID 替换乐观插入的 local- 占位，
      // 避免编辑/删除/分支重生成在当前会话周期内拿到不存在的本地 ID
      let nextMessages = state.messages
      if (user_message_id) {
        nextMessages = reconcileRunUserMessage(
          state.messages,
          user_message_id,
          optimisticRunUserIds.get(conversation_id),
        )
      }
      optimisticRunUserIds.delete(conversation_id)
      set({
        messages: upsertMessageById(nextMessages, message),
        // 完成后保留过程记录：顶部"已处理 N 个操作"徽章可展开回看（新任务/切会话时清空）
        lastTaskSummary: {
          status: unfinished ? 'incomplete' : 'completed',
          durationMs,
          toolCount,
          fileCount,
          // 后端把任务累计 token 持久化到结束消息（每轮求和），此处直接取用
          tokensIn: message.tokens_in ?? 0,
          tokensOut: message.tokens_out ?? 0,
        },
        // 只有通过后端完成验收才标记 done；护栏收尾/复核未通过保持 error，
        // 避免“继续任务”按钮与绿色完成进度卡相互矛盾。
        plan:
          state.plan && state.plan.phase === 'running'
            ? { ...state.plan, phase: unfinished ? 'error' : 'done' }
            : state.plan,
        // 任务未完成（上限中止/用户停止/中途失败，有工具成果无最终总结）：
        // 保留"继续任务"按钮断点续跑；正常完成则清空
        unfinishedConv: unfinished ? { conversationId: conversation_id } : null,
      })
    }
    // 刷新会话列表（标题/时间变化；保持搜索关键字过滤）。
    // 以本轮完成的会话所属项目为准（后台会话完成时，当前会话可能属于另一项目），
    // 从已加载会话列表中反查 project_id；查不到说明不在当前项目，跳过。
    const completedConv = get().conversations.find((c) => c.id === conversation_id)
    const projectId = completedConv?.project_id ?? state.currentConversation?.project_id
    if (projectId) {
      const kw = get().conversationKeyword.trim()
      const includeArchived = get().conversations.some((c) => c.archived)
      listConversations(projectId, includeArchived, kw)
        .then((conversations) => set({ conversations }))
        .catch(() => {})
    }
    // 刷新 token/成本累计（assistant 消息已入库）
    get().loadTokenStats(conversation_id).catch(() => {})
    // 任务改了文件：失效文件树懒加载缓存并重载（感知 Agent/外部工具的改动，避免旧目录视图）
    if (fileCount > 0) {
      get().rebuildIndex().catch(() => {})
    }
  }).catch(() => {})

  // 后台经济模型生成的精炼标题就绪：更新侧栏与标题栏（conversation-renamed 事件）
  listen<{ conversation_id: string; title: string }>('conversation-renamed', (event) => {
    const { conversation_id, title } = event.payload
    const state = get()
    set({
      conversations: state.conversations.map((c) => (c.id === conversation_id ? { ...c, title } : c)),
      currentConversation:
        state.currentConversation?.id === conversation_id ? { ...state.currentConversation, title } : state.currentConversation,
    })
  }).catch(() => {})

  // 会话被删除（含另一客户端/LAN、或删除运行中会话后端清理后）：同步清理前端状态
  listen<{ conversation_id: string }>('conversation-deleted', (event) => {
    const { conversation_id } = event.payload
    const state = get()
    const streamings = { ...state.streamings }
    delete streamings[conversation_id]
    pendingStreamDeltas.delete(conversation_id)
    const deletedTrace = sendTraces.get(conversation_id)
    sendTraces.delete(conversation_id)
    deletedTrace?.end()
    const pendingConfirmations = { ...state.pendingConfirmations }
    delete pendingConfirmations[conversation_id]
    if (state.currentConversation?.id === conversation_id) {
      // 删除的是当前会话：清空视图，打开同项目第一个会话
      const remaining = state.conversations.filter((c) => c.id !== conversation_id)
      set({
        streamings,
        pendingConfirmations,
        conversations: remaining,
        currentConversation: null,
        messages: [],
        streaming: emptyStreaming(),
        toolRuns: [],
        agentRuns: [],
        plan: null,
        todos: [],
        askCard: null,
        unfinishedConv: null,
      })
      if (remaining.length > 0) {
        get().openConversation(remaining[0].id).catch(() => {})
      }
    } else {
      set({
        streamings,
        pendingConfirmations,
        conversations: state.conversations.filter((c) => c.id !== conversation_id),
      })
    }
  }).catch(() => {})

  listen<{ conversation_id: string; run_id?: string; error: string; kind: string; title: string; reason: string; suggestion: string; retryable: boolean; status_code?: number | null }>(
    'chat-error',
    (event) => {
      const { conversation_id, run_id, error, kind, title, reason, suggestion, retryable, status_code } = event.payload
      if (!acceptsRun(conversation_id, run_id)) return
      markRunTerminal(conversation_id, run_id)
      flushStreamDeltas(conversation_id)
      const state = get()
      const bucket = state.streamings[conversation_id]
      if (!bucket) return
      const trace = sendTraces.get(conversation_id)
      if (trace) {
        sendTraces.delete(conversation_id)
        trace.mark('backend-error')
        trace.end()
      }
      const isCurrent = state.currentConversation?.id === conversation_id
      setBucket(conversation_id, {
        error,
        errorDetail: { kind, title, reason, suggestion, retryable, statusCode: status_code ?? null },
      })
      set((s) => {
        const pendingConfirmations = { ...s.pendingConfirmations }
        delete pendingConfirmations[conversation_id]
        const current = s.currentConversation?.id === conversation_id
        return {
          pendingConfirmations,
          ...(current
            ? {
                toolApprovals: [],
                pendingPlan: s.pendingPlan?.conversationId === conversation_id ? null : s.pendingPlan,
              }
            : {}),
        }
      })
      if (isCurrent) {
        set({
          // 任务出错：进度卡立即定档（保留已完成步骤）
          plan: state.plan && state.plan.phase === 'running' ? { ...state.plan, phase: 'error' } : state.plan,
          // 任务结束：挂起的提问卡无意义，一并关闭
          askCard: null,
        })
      }
    },
  ).catch(() => {})

  // 用户停止且无内容可入库：清空流式状态（有内容时后端走 chat-done）
  listen<{ conversation_id: string; run_id?: string; unfinished: boolean; user_message_id?: string | null }>('chat-stopped', (event) => {
    const { conversation_id, run_id, unfinished, user_message_id } = event.payload
    if (!acceptsRun(conversation_id, run_id)) return
    markRunTerminal(conversation_id, run_id)
    flushStreamDeltas(conversation_id)
    const state = get()
    const bucket = state.streamings[conversation_id]
    if (!bucket) return
    const trace = sendTraces.get(conversation_id)
    if (trace) {
      sendTraces.delete(conversation_id)
      trace.mark('stopped')
      trace.end()
    }
    const isCurrent = state.currentConversation?.id === conversation_id
    const partial = bucket.content.trim()
    setBucket(conversation_id, null)
    set((s) => {
      const pendingConfirmations = { ...s.pendingConfirmations }
      delete pendingConfirmations[conversation_id]
      return {
        pendingConfirmations,
        ...(isCurrent
          ? {
              toolApprovals: [],
              pendingPlan: s.pendingPlan?.conversationId === conversation_id ? null : s.pendingPlan,
            }
          : {}),
      }
    })
    if (isCurrent) {
      // 已流式输出的内容不因无正文入库而消失：作为临时消息追加展示（不落库，刷新会话后回到真实历史）
      let baseMessages = state.messages
      if (user_message_id) {
        baseMessages = reconcileRunUserMessage(
          baseMessages,
          user_message_id,
          optimisticRunUserIds.get(conversation_id),
        )
      }
      optimisticRunUserIds.delete(conversation_id)
      const messages =
        partial.length > 0
          ? [
              ...baseMessages,
              {
                id: `local-stop-${Date.now()}`,
                conversation_id,
                role: 'assistant' as const,
                content: partial,
                references_json: null,
                model: null,
                tokens_in: null,
                tokens_out: null,
                reasoning: bucket.reasoning || null,
                queued: 0,
                agent_owned: 0,
                modified_files_json: null,
                duration_ms: null,
                created_at: Math.floor(Date.now() / 1000),
              } satisfies ChatMessage,
            ]
          : baseMessages
      set({
        messages,
        // 停止后同样保留过程记录：已执行部分可在徽章中展开回看
        // 用户停止：进度卡定档保留（展示已完成部分）
        plan: state.plan && state.plan.phase === 'running' ? { ...state.plan, phase: 'error' } : state.plan,
        // 停止且未完成：展示"继续任务"按钮断点续跑（有已执行工具成果可接续）
        unfinishedConv: unfinished ? { conversationId: conversation_id } : state.unfinishedConv,
        // 停止任务：挂起的提问卡同步关闭（后端已关闭通道）
        askCard: null,
      })
    }
  }).catch(() => {})

  // ---------- Agent 工具事件 ----------
  listen<{ conversation_id: string; run_id?: string; call_id?: string; tool: string; args: string; round?: number; total?: number; level?: string; desc?: string }>(
    'chat-tool-start',
    (event) => {
      const { conversation_id, run_id, call_id, tool, args, round, total, level, desc } = event.payload
      if (!acceptsRun(conversation_id, run_id)) return
      flushStreamDeltas(conversation_id)
      const state = get()
      // 工具活动按会话计数（看门狗判据，后台会话同样生效）
      const bucket = state.streamings[conversation_id]
      if (bucket) setBucket(conversation_id, { toolRunning: bucket.toolRunning + 1 })
      // 视图级过程状态（toolRuns/plan/todos/ask…）仅反映当前会话；后台会话事件不污染当前视图
    if (state.currentConversation?.id !== conversation_id) return
      set({
        toolRuns: [
          ...state.toolRuns,
          {
            id: call_id ? `tool-call-${call_id}` : `tool-${Date.now()}-${toolSeq++}`,
            tool,
            args,
            status: 'running',
            output: '',
            round,
            total,
            level,
            desc,
            startedAt: Date.now(),
          },
        ],
        // 终端面板同步追加：工具执行实时记录
        terminalEntries: [
          ...state.terminalEntries,
          {
            id: call_id ? `term-call-${call_id}` : `term-${Date.now()}-${toolSeq}`,
            tool,
            args,
            status: 'running',
            output: '',
            startedAt: Date.now(),
          },
        ],
        // 任务进度清单联动：首个工具→解析计划；后续工具→无进行中步骤时推进下一个待办
        plan: advancePlan(state.plan, state.streaming.content, state.messages, { start: true }),
      })
    },
  ).catch(() => {})

  listen<{ conversation_id: string; run_id?: string; call_id?: string; tool: string; ok: boolean; output: string; duration_ms?: number }>('chat-tool-done', (event) => {
    const { conversation_id, run_id, call_id, tool, ok, output, duration_ms } = event.payload
    if (!acceptsRun(conversation_id, run_id)) return
    const state = get()
    // 工具活动按会话计数（看门狗判据，后台会话同样生效）
    const bucket = state.streamings[conversation_id]
    if (bucket) setBucket(conversation_id, { toolRunning: Math.max(0, bucket.toolRunning - 1) })
    // 视图级过程状态（toolRuns/plan/todos/ask…）仅反映当前会话；后台会话事件不污染当前视图
    if (state.currentConversation?.id !== conversation_id) return
    const runningToolIdx = call_id
      ? state.toolRuns.findIndex((r) => r.id === `tool-call-${call_id}` && r.status === 'running')
      : firstRunningIndex(state.toolRuns, (r) => r.tool === tool)
    const runningTerminalIdx = call_id
      ? state.terminalEntries.findIndex((e) => e.id === `term-call-${call_id}` && e.status === 'running')
      : firstRunningIndex(state.terminalEntries, (e) => e.tool === tool)
    set({
      // 同名工具可并发执行；一次 done 只能收敛一个 start，不能把所有同名卡片一起结束。
      toolRuns: state.toolRuns.map((r, idx) =>
        idx === runningToolIdx
          ? {
              ...r,
              status: ok ? 'done' : 'error',
              output,
              // 后端精确耗时优先（含审批等待/重试）；缺失时回退本地估算
              durationMs: duration_ms ?? Date.now() - (r.startedAt ?? Date.now()),
            }
          : r,
      ),
      // 终端面板同步更新：匹配同工具名的运行中条目（最后一条），写入结果与耗时
      terminalEntries: state.terminalEntries.map((e, idx) => {
        if (idx !== runningTerminalIdx) return e
        return {
          ...e,
          status: ok ? 'done' : 'error',
          output,
          durationMs: Date.now() - e.startedAt,
        }
      }),
      // 任务进度清单联动：工具成功→步骤完成；失败→步骤失败（replan 后继续推进）
      plan: advancePlan(state.plan, state.streaming.content, state.messages, { ok }),
    })

    // 构建/部署/测试等重型任务结束时发送桌面通知（窗口失焦时尤其有用）
    const notifTools: Record<string, { zh: string; en: string }> = {
      build_project: { zh: '构建', en: 'Build' },
      deploy: { zh: '部署', en: 'Deploy' },
      run_tests: { zh: '测试', en: 'Tests' },
      run_preview: { zh: '预览', en: 'Preview' },
    }
    const meta = notifTools[tool]
    if (meta) {
      const titleOk = `${meta.zh}完成`
      const titleErr = `${meta.zh}失败`
      const body = ok ? '任务已成功结束' : (output || '').slice(-160) || '请查看构建日志'
      sendNotification(ok ? titleOk : titleErr, body, ok ? 'success' : 'error').catch(() => {})
    }
  }).catch(() => {})

  // 任务账本推送（Ledger 协议）：每轮工具执行后实时刷新（finished=false）；任务结束时最终态
  // （finished=true：完成→ledger=null 收起；中断→保留展示）。按会话聚合，切回会话恢复展示
  listen<{ conversation_id: string; ledger: TaskLedger | null; finished: boolean }>('chat-ledger', (event) => {
    const { conversation_id, ledger, finished } = event.payload
    set((s) => ({ taskLedgers: { ...s.taskLedgers, [conversation_id]: { ledger, finished } } }))
  }).catch(() => {})

  // 工具权限审核请求（自动审核模式）：入队等待用户确认弹窗
  listen<{ conversation_id: string; request_id: string; tool: string; args: string; level?: string; desc?: string }>(
    'chat-tool-approval',
    (event) => {
      const { conversation_id, request_id, tool, args, level, desc } = event.payload
      const isCurrent = get().currentConversation?.id === conversation_id
      // 后台会话同样记录到待确认表（列表角标 + 切回恢复）；弹窗视图仅当前会话刷新
      upsertPending({
        conversation_id,
        kind: 'approval',
        request_id,
        tool,
        args,
        plan: null,
        question: null,
        options: null,
      })
      if (isCurrent) {
        set((s) => ({
          toolApprovals: [...s.toolApprovals, { requestId: request_id, tool, args, level, desc }],
        }))
      }
    },
  ).catch(() => {})

  // Agent 诊断引导卡片：签名/SDK/依赖等需用户手动操作时，在对话流上方展示可操作卡片
  listen<{
    conversation_id: string
    request_id: string
    category: DiagnoseCard['category']
    title: string
    message: string
    action: DiagnoseCard['action']
    created_at: number
  }>('diagnose-card', (event) => {
    const { conversation_id, request_id, category, title, message, action, created_at } = event.payload
    const card: DiagnoseCard = {
      id: `diag-${created_at}-${Math.random().toString(36).slice(2, 8)}`,
      requestId: request_id,
      conversationId: conversation_id,
      category,
      title,
      message,
      action,
      createdAt: created_at,
    }
    const state = get()
    set({ diagnoseCards: [...state.diagnoseCards, card] })
  }).catch(() => {})

  // 计划/审查模式：Agent 输出计划后等待用户确认
  listen<{ conversation_id: string; request_id: string; plan: string }>('chat-plan', (event) => {
    const { conversation_id, request_id, plan } = event.payload
    const isCurrent = get().currentConversation?.id === conversation_id
    upsertPending({
      conversation_id,
      kind: 'plan',
      request_id,
      tool: null,
      args: null,
      plan,
      question: null,
      options: null,
    })
    if (isCurrent) {
      set({ pendingPlan: { requestId: request_id, conversationId: conversation_id, plan } })
    }
  }).catch(() => {})

  // 计划已被处理（批准/驳回）：清空待确认状态；批准时把计划保留为"已批准计划"执行中展示。
  // 后台会话的计划被处理时（如超时自动驳回）也要从待确认表移除，避免角标残留。
  listen<{ conversation_id: string; approved: boolean }>('chat-plan-resolved', (event) => {
    const { conversation_id, approved } = event.payload
    set((s) => {
      const arr = s.pendingConfirmations[conversation_id]
      let pendingConfirmations = s.pendingConfirmations
      if (arr) {
        const filtered = arr.filter((p) => p.kind !== 'plan')
        if (filtered.length !== arr.length) {
          pendingConfirmations = { ...s.pendingConfirmations }
          if (filtered.length > 0) pendingConfirmations[conversation_id] = filtered
          else delete pendingConfirmations[conversation_id]
        }
      }
      const isCurrentPlan = s.pendingPlan?.conversationId === conversation_id
      return {
        pendingConfirmations,
        pendingPlan: isCurrentPlan ? null : s.pendingPlan,
        approvedPlan:
          isCurrentPlan && approved
            ? { conversationId: conversation_id, plan: s.pendingPlan?.plan ?? '' }
            : s.approvedPlan,
      }
    })
  }).catch(() => {})

  // ---------- 子 Agent 事件 ----------
  listen<{ conversation_id: string; run_id?: string; name: string; model: string }>('chat-agent-start', (event) => {
    const { conversation_id, run_id, name, model } = event.payload
    if (!acceptsRun(conversation_id, run_id)) return
    const state = get()
    // 视图级过程状态（toolRuns/plan/todos/ask…）仅反映当前会话；后台会话事件不污染当前视图
    if (state.currentConversation?.id !== conversation_id) return
    set({
      agentRuns: [
        ...state.agentRuns,
        { id: `agent-${Date.now()}-${agentSeq++}`, name, model, status: 'running', output: '' },
      ],
    })
  }).catch(() => {})

  listen<{ conversation_id: string; run_id?: string; name: string; model: string; ok: boolean; output: string }>(
    'chat-agent-done',
    (event) => {
      const { conversation_id, run_id, name, ok, output } = event.payload
      if (!acceptsRun(conversation_id, run_id)) return
      const state = get()
      // 视图级过程状态（toolRuns/plan/todos/ask…）仅反映当前会话；后台会话事件不污染当前视图
    if (state.currentConversation?.id !== conversation_id) return
      const runningAgentIdx = firstRunningIndex(state.agentRuns, (r) => r.name === name)
      set({
        agentRuns: state.agentRuns.map((r, idx) =>
          idx === runningAgentIdx ? { ...r, status: ok ? 'done' : 'error', output } : r,
        ),
      })
    },
  ).catch(() => {})

  // 后台任务完成（run_in_background 启动的长命令）：终端面板记录 + 桌面通知。
  // 完成时可能没有正在流式的请求，故不按 streaming 会话过滤（模型侧由注入队列负责）
  listen<{
    conversation_id: string
    job_id: string
    command: string
    ok: boolean
    summary: string
  }>('chat-job-done', (event) => {
    const { conversation_id, job_id, command, ok, summary } = event.payload
    const state = get()
    const line = `[后台任务 ${job_id}] ${ok ? '完成' : '失败'}：${command}（${summary}）`
    if (state.currentConversation?.id === conversation_id) {
      const next = [...state.buildLogs, { id: nextBuildLogId(), stream: 'system' as const, line, ts: Date.now() }]
      const trimmed = next.length > 2000 ? next.slice(next.length - 2000) : next
      set({ buildLogs: trimmed })
    }
    sendNotification(ok ? '后台任务完成' : '后台任务失败', `${command}｜${summary}`, ok ? 'success' : 'error').catch(
      () => {},
    )
  }).catch(() => {})

  // 构建/部署流式日志（agent:log）：按行累积到 buildLogs，任务开始时清空，保留最近 2000 行；
  // 同时把行追加到当前运行中工具的 liveOutput（工具卡片实时可见执行过程）。
  // 高频输出下用 rAF 批处理：日志先进缓冲，每帧合并一次 set，避免逐行 set 卡死主线程。
  const flushPendingLogs = () => {
    logFlushScheduled = false
    if (logFlushTimer !== null) {
      clearTimeout(logFlushTimer)
      logFlushTimer = null
    }
    const batch = pendingLogLines
    pendingLogLines = []
    if (batch.length === 0) return
    const state = get()
    const convId = state.currentConversation?.id
    if (!convId) return
    // 批内可能混入已切换会话后的残留事件，按当前会话过滤
    const lines = batch.filter((l) => l.convId === convId)
    if (lines.length === 0) return

    // buildLogs 追加（保留最近 2000 行）
    const merged = [
      ...state.buildLogs,
      ...lines.map(({ stream, line, ts }) => ({ id: nextBuildLogId(), stream, line, ts })),
    ]
    const trimmed = merged.length > 2000 ? merged.slice(merged.length - 2000) : merged

    // 把本批 stdout/stderr 行一次性拼到运行中工具/终端的 liveOutput 尾部，避免每行 O(n) 复制
    const appendLive = (entries: { status: string; liveOutput?: string }[]) => {
      const lastRunning = entries.reduce((acc, x, i) => (x.status === 'running' ? i : acc), -1)
      if (lastRunning < 0) return entries
      let acc = ''
      for (const l of lines) {
        if (l.stream !== 'system') acc += l.line + '\n'
      }
      if (!acc) return entries
      return entries.map((r, idx) => {
        if (idx !== lastRunning) return r
        const prev = r.liveOutput ?? ''
        const capped = prev.length > 80000 ? prev.slice(prev.length - 40000) : prev
        return { ...r, liveOutput: capped + acc }
      })
    }

    set({
      buildLogs: trimmed,
      toolRuns: appendLive(state.toolRuns) as typeof state.toolRuns,
      terminalEntries: appendLive(state.terminalEntries) as typeof state.terminalEntries,
    })
  }

  const scheduleLogFlush = () => {
    if (logFlushScheduled) return
    logFlushScheduled = true
    requestAnimationFrame(flushPendingLogs)
    // 后台/最小化窗口中 rAF 会被节流；100ms 兜底同时适用于 WKWebView 与 WebView2。
    logFlushTimer = setTimeout(flushPendingLogs, 100)
  }

  listen<{ conversation_id: string; run_id?: string; stream: string; line: string }>('agent:log', (event) => {
    const { conversation_id, run_id, stream, line } = event.payload
    if (!acceptsRun(conversation_id, run_id)) return
    const state = get()
    // 视图级过程状态（toolRuns/plan/todos/ask…）仅反映当前会话；后台会话事件不污染当前视图
    if (state.currentConversation?.id !== conversation_id) return
    pendingLogLines.push({ stream, line, ts: Date.now(), convId: conversation_id })
    if (pendingLogLines.length > MAX_PENDING_LOG_LINES) {
      pendingLogLines.splice(0, pendingLogLines.length - MAX_PENDING_LOG_LINES)
    }
    scheduleLogFlush()
  }).catch(() => {})

  // Rust 命令执行器把高频 stdout/stderr 合并成批量 IPC；单行事件仍用于系统提示并保持兼容。
  listen<{ conversation_id: string; run_id?: string; stream: string; lines: string[] }>('agent:log-batch', (event) => {
    const { conversation_id, run_id, stream, lines } = event.payload
    if (!acceptsRun(conversation_id, run_id)) return
    if (get().currentConversation?.id !== conversation_id || lines.length === 0) return
    const ts = Date.now()
    for (const line of lines) pendingLogLines.push({ stream, line, ts, convId: conversation_id })
    if (pendingLogLines.length > MAX_PENDING_LOG_LINES) {
      pendingLogLines.splice(0, pendingLogLines.length - MAX_PENDING_LOG_LINES)
    }
    scheduleLogFlush()
  }).catch(() => {})

  // 任务清单（todo_write）：后端推送合并后的完整清单，前端实时渲染进度
  listen<{ conversation_id: string; todos: TodoItem[] }>('agent:todo', (event) => {
    const { conversation_id, todos } = event.payload
    const state = get()
    // 视图级过程状态（toolRuns/plan/todos/ask…）仅反映当前会话；后台会话事件不污染当前视图
    if (state.currentConversation?.id !== conversation_id) return
    set({ todos })
  }).catch(() => {})

  // Agent 提问（ask_user）：渲染提问卡等待用户回答
  listen<{ conversation_id: string; request_id: string; question: string; options: string[] }>(
    'chat-ask',
    (event) => {
      const { conversation_id, request_id, question, options } = event.payload
      const isCurrent = get().currentConversation?.id === conversation_id
      upsertPending({
        conversation_id,
        kind: 'ask',
        request_id,
        tool: null,
        args: null,
        plan: null,
        question,
        options,
      })
      if (isCurrent) {
        set({ askCard: { requestId: request_id, conversationId: conversation_id, question, options } })
        // 后端 5 分钟超时兜底：超时后前端残留卡片自动关闭
        setTimeout(() => {
          if (get().askCard?.requestId === request_id) {
            set({ askCard: null })
          }
        }, 5 * 60 * 1000)
      }
    },
  ).catch(() => {})

  return {
    conversations: [],
    currentConversation: null,
    messages: [],
    olderHasMore: false,
    loadingOlder: false,
    streaming: emptyStreaming(),
    streamings: {},
    toolRuns: [],
    terminalEntries: [],
    buildLogs: [],
    agentRuns: [],
    plan: null,
    toolApprovals: [],
    pendingConfirmations: {},
    taskLedgers: {},
    snapshots: [],
    loadingSnapshots: false,
    diagnoseCards: [],
    dismissDiagnoseCard: (id) => set((s) => ({ diagnoseCards: s.diagnoseCards.filter((c) => c.id !== id) })),
    pendingPlan: null,
    approvedPlan: null,
    unfinishedConv: null,
    todos: [],
    askCard: null,
    queuedList: [],
    conversationKeyword: '',

    setConversationKeyword: async (kw) => {
      set({ conversationKeyword: kw })
      const project = get().currentProject
      if (!project) return
      // 保留归档视图状态：当前列表若含归档会话则继续按归档过滤
      const includeArchived = get().conversations.some((c) => c.archived)
      const conversations = await listConversations(project.id, includeArchived, kw.trim())
      set({ conversations })
    },

    clearTerminal: () => {
      set({ terminalEntries: [] })
    },

    clearBuildLogs: () => {
      set({ buildLogs: [] })
    },

    loadSnapshots: async (conversationId) => {
      const s = get()
      if (s.loadingSnapshots || s.currentConversation?.id !== conversationId) return
      set({ loadingSnapshots: true })
      try {
        const snapshots = await listSnapshotsApi(conversationId)
        if (get().currentConversation?.id === conversationId) set({ snapshots })
      } catch {
        // 快照是增值能力：加载失败不阻塞时间线弹窗（空列表可重试打开）
      } finally {
        if (get().currentConversation?.id === conversationId) set({ loadingSnapshots: false })
      }
    },

    // 恢复会话到历史快照点（时间旅行）：成功后刷新消息/账本/快照列表，
    // 返回归档与恢复条数（调用方展示提示）
    restoreToSnapshot: async (conversationId, snapshotId) => {
      const result = await restoreSnapshotApi(conversationId, snapshotId)
      const same = () => get().currentConversation?.id === conversationId
      // 刷新可见消息（分页重拉：归档段消失、恢复段重现）
      const page = await listMessagesPage(conversationId)
      if (same()) {
        set({
          messages: page.messages,
          olderHasMore: page.hasMore,
          feedbackMap: {},
          versionMap: {},
          tokenStats: null,
        })
      }
      // 刷新账本（写回快照时刻的执行轨迹，续跑/继续任务继承）
      void getTaskLedgerApi(conversationId)
        .then((ledger) => {
          if (same()) set((s) => ({ taskLedgers: { ...s.taskLedgers, [conversationId]: { ledger, finished: true } } }))
        })
        .catch(() => {})
      // 刷新快照列表（is_current 标记变化）
      await get().loadSnapshots(conversationId)
      return result
    },

    rollbackTask: async (conversationId, dryRun) => {
      const info = await rollbackConversation(conversationId, dryRun)
      if (!dryRun) {
        // 回滚后工作区已变：刷新 Git 面板状态，文件树目录内容也可能变化
        get().refreshGitBranches().catch(() => {})
        get().rebuildIndex().catch(() => {})
      }
      return info
    },

    newConversation: async (worktree) => {
      const project = get().currentProject
      if (!project) return
      ++openConversationSeq
      const prevRoot = conversationRoot(get().currentConversation)
      const t = startPerfTrace('newConversation', { pid: project.id.slice(0, 8) })
      const conv = await createConversation(project.id, undefined, worktree)
      if (get().currentProject?.id !== project.id) {
        t.end()
        return
      }
      t.mark('created')
      const conversations = await listConversations(project.id)
      if (get().currentProject?.id !== project.id) {
        t.end()
        return
      }
      t.mark('convs-listed')
      // 新会话：清空上一会话的过程记录（完成态徽章不复用）
      set({ conversations, currentConversation: conv, messages: [], plan: null, toolRuns: [], agentRuns: [], approvedPlan: null, unfinishedConv: null, todos: [], askCard: null })
      // 持久化会话选择
      setItem(STORAGE_KEYS.LAST_CONV_PREFIX + project.id, conv.id)
      // 工作目录变化（local ↔ worktree / 不同 worktree）：重载文件树跟随新会话
      if (prevRoot !== conversationRoot(conv)) {
        get().rebuildIndex().catch(() => {})
      }
      t.end()
    },

    forkCurrentConversation: async (untilMessageId) => {
      const cur = get().currentConversation
      const project = get().currentProject
      if (!cur || !project) return
      const conv = await forkConversation(cur.id, untilMessageId)
      // 先刷新列表（openConversation 从列表查找会话对象）
      const conversations = await listConversations(project.id)
      set({ conversations })
      await get().openConversation(conv.id)
    },

    openConversation: async (id) => {
      const openSeq = ++openConversationSeq
      const trace = startPerfTrace('openConversation', { convId: id.slice(0, 8) })
      const prevConv = get().currentConversation
      // 搜索命中跳转：目标会话可能不在当前可见列表（如已归档/被归档视图隐藏），按 id 兜底拉取
      let target = get().conversations.find((c) => c.id === id) ?? null
      if (!target) {
        target = await getConversation(id).catch(() => null)
      }
      if (openSeq !== openConversationSeq) {
        trace.end()
        return
      }
      if (!target) {
        trace.end()
        return
      }
      // 从待确认表恢复本会话的审批/计划（后台会话事件已按会话记录；提问由下方 getAsk 异步恢复）
      const pendings = get().pendingConfirmations[id] ?? []
      const restoredApprovals = pendings
        .filter((p) => p.kind === 'approval')
        .map((p) => ({ requestId: p.request_id, tool: p.tool ?? '', args: p.args ?? '' }))
      const restoredPlan = (() => {
        const p = pendings.find((p) => p.kind === 'plan')
        return p ? { requestId: p.request_id, conversationId: p.conversation_id, plan: p.plan ?? '' } : null
      })()
      set({
        currentConversation: target,
        plan: null,
        lastTaskSummary: null,
        approvedPlan: null,
        unfinishedConv: null,
        // 切换会话：恢复目标会话的流式分桶视图（后台流式中切回，内容/状态不丢）
        streaming: get().streamings[id] ?? emptyStreaming(),
        // 不清空 messages：保留旧会话内容直到新会话消息就绪后一次性替换，
        // 避免切换瞬间出现 loading→消息列表 两次渲染（ChatGPT 风格无缝切换）
        olderHasMore: true,
        loadingOlder: false,
        // 会话级运行态立即重置（这些字段不被消息列表项订阅，不触发 MessageItem 重渲染）
        toolRuns: [],
        agentRuns: [],
        todos: [],
        askCard: null,
        // 审批/计划视图同样按会话恢复（旧会话的待确认弹窗不泄漏到新会话）
        toolApprovals: restoredApprovals,
        pendingPlan: restoredPlan,
        // 注意：feedbackMap / versionMap / tokenStats 不在此处清空——它们是 MessageItem 的订阅依赖，
        // 立即清空会导致旧消息列表（等待新消息加载期间仍在显示）全量重渲染且丢失反馈状态，
        // 纯属浪费一次渲染；延迟到新消息 set 时一并清空，把两次重渲染（清字段→换消息）合并为一次
      })
      trace.mark('state-set')

      // 会话工作目录变化（同项目内 local ↔ worktree / 不同 worktree）：重载文件树跟随；
      // 跨项目切换由 openProject 的侧边栏加载处理，这里跳过避免重复加载
      if (target && prevConv?.project_id === target.project_id && conversationRoot(prevConv) !== conversationRoot(target)) {
        get().rebuildIndex().catch(() => {})
      }

      // 持久化会话选择（下次打开该项目时恢复）
      const pid = get().currentProject?.id
      if (pid) {
        setItem(STORAGE_KEYS.LAST_CONV_PREFIX + pid, id)
      }

      // 分页加载：只取最近一页，向上滚动时再按游标加载更早历史，避免长会话全量加载渲染
      let page
      try {
        page = await listMessagesPage(id)
      } catch {
        // 平滑切换会暂留上一会话消息，但加载失败后继续显示会造成严重的会话错觉。
        // 失败时清空旧内容，等待用户重试打开；不能把 A 会话内容挂在 B 会话标题下。
        if (openSeq === openConversationSeq && get().currentConversation?.id === id) {
          set({ messages: [], olderHasMore: false })
        }
        trace.mark('messages-error')
        trace.end()
        return
      }
      trace.mark('messages-loaded')
      // 竞态保护：期间用户可能已切到其他会话，过期结果直接丢弃，
      // 否则旧会话消息/反馈/版本会覆盖新会话（快速连续切换时必现）
      if (openSeq !== openConversationSeq || get().currentConversation?.id !== id) {
        trace.end()
        return
      }
      set({
        messages: page.messages,
        olderHasMore: page.hasMore,
        // 新消息落地时一并清空旧会话的反馈/版本/统计：与 messages 同批更新只触发一次 React 渲染
        feedbackMap: {},
        versionMap: {},
        tokenStats: null,
      })
      trace.mark('messages-state-set')
      // 三个数据查询（feedback / versions / tokenStats）互相独立，立即并行触发，
      // 各自返回后单独 setState，不因某个慢查询阻塞首次渲染。
      void get().loadFeedback(id).catch(() => {})
      void get().loadVersions(id).catch(() => {})
      void get().loadTokenStats(id).catch(() => {})
      // 恢复该会话的任务清单与挂起提问（若有）
      void getTodosApi(id)
        .then((todos) => {
          if (get().currentConversation?.id === id) set({ todos })
        })
        .catch(() => {})
      // 恢复该会话的任务账本（未完成任务落库；完成时已清空返回 null，不展示）
      void getTaskLedgerApi(id)
        .then((ledger) => {
          if (get().currentConversation?.id !== id) return
          set((s) => ({ taskLedgers: { ...s.taskLedgers, [id]: { ledger, finished: true } } }))
        })
        .catch(() => {})
      void getAskApi(id)
        .then((ask) => {
          if (get().currentConversation?.id !== id) return
          set({
            askCard: ask
              ? { requestId: ask.request_id, conversationId: ask.conversation_id, question: ask.question, options: ask.options }
              : null,
          })
        })
        .catch(() => {})
      // 等待 React commit + 浏览器 paint 完成：set() 只是调度了状态更新，
      // 真正的 DOM 渲染和屏幕绘制发生在之后；双 rAF 后用户才真正看到新消息列表，
      // 此时标记才反映真实可感知耗时。
      await waitForNextPaint()
      if (get().currentConversation?.id !== id) {
        trace.end()
        return
      }
      trace.mark('messages-painted')
      trace.end()
    },

    // 加载更早的历史消息：以当前第一条消息为游标拉取上一页，prepend 到头部。
    // 返回新增条数（调用方用于滚动锚定：新内容插入后保持视口位置不跳动）。
    loadOlderMessages: async (conversationId) => {
      const s = get()
      if (s.loadingOlder || !s.olderHasMore || s.currentConversation?.id !== conversationId) return 0
      const first = s.messages[0]
      if (!first) return 0
      set({ loadingOlder: true })
      const t = startPerfTrace('loadOlderMessages', { conv: conversationId.slice(0, 8) })
      try {
        const page = await listMessagesPage(conversationId, first.id)
        t.mark('page-loaded')
        if (get().currentConversation?.id !== conversationId) { t.end(); return 0 }
        if (page.messages.length === 0) {
          // 游标失效（消息已删除）或确无更早：终止分页
          set({ olderHasMore: false })
          t.end()
          return 0
        }
        // await 期间可能已追加新消息（流式/排队），以其为尾部，避免覆盖丢失
        const tail = get().messages
        set({ messages: [...page.messages, ...tail], olderHasMore: page.hasMore })
        t.mark('merged')
        t.end()
        return page.messages.length
      } catch {
        t.mark('error')
        t.end()
        return 0
      } finally {
        // 失败或已切会话：恢复可重试状态（防重入锁释放）
        if (get().currentConversation?.id === conversationId) {
          set({ loadingOlder: false })
        }
      }
    },

    // 新任务开始时清空上次任务摘要（防止跨任务残留展示）
    sendUserMessage: async (content, options, references, images) => {
      const conv = get().currentConversation
      if (!conv) return
      // 同会话流式中防重（error 态允许重发；跨会话放行——后端同项目任务排队机制生效）
      const existing = get().streamings[conv.id]
      if (existing && !existing.error) return
      // 开始发送消息性能追踪
      const sendTrace = startPerfTrace('sendMessage', { conv: conv.id.slice(0, 8) })
      sendTraces.set(conv.id, sendTrace)
      // 乐观展示 user 消息（Rust 端同样入库）。ID 带随机后缀，避免跨会话同秒发送碰撞
      const now = Math.floor(Date.now() / 1000)
      const localId = `local-${now}-${Math.random().toString(36).slice(2, 8)}`
      optimisticRunUserIds.set(conv.id, localId)
      const fresh: StreamingState = { ...emptyStreaming(), conversationId: conv.id, startedAt: Date.now(), lastDeltaAt: Date.now() }
      set({
        messages: [
          ...get().messages,
          {
            id: localId,
            conversation_id: conv.id,
            role: 'user',
            content,
            references_json: references?.length ? JSON.stringify(references) : null,
            model: null,
            tokens_in: null,
            tokens_out: null,
            reasoning: null,
            queued: 0,
            agent_owned: 0,
            modified_files_json: null,
            duration_ms: null,
            created_at: now,
          },
        ],
        streamings: { ...get().streamings, [conv.id]: fresh },
        streaming: fresh,
        toolRuns: [],
        terminalEntries: [],
        buildLogs: [],
        agentRuns: [],
        plan: null,
        todos: [],
        askCard: null,
        lastTaskSummary: null,
        approvedPlan: null,
        unfinishedConv: null,
      })
      sendTrace.mark('optimistic-rendered')
      try {
        await streamChatApi(conv.id, content, options, false, references, images)
      } catch (e) {
        optimisticRunUserIds.delete(conv.id)
        sendTraces.delete(conv.id)
        sendTrace.mark('error')
        sendTrace.end()
        // 看门狗报错后用户可能已立即重试；旧 invoke 的延迟 reject 不得覆盖新任务桶。
        if (get().streamings[conv.id]?.startedAt === fresh.startedAt) {
          setBucket(conv.id, { ...emptyStreaming(), error: String(e) })
          // invoke 可能在 user 入库前失败，也可能在入库后模型请求失败。以数据库为
          // 真源刷新最近一页，既移除幽灵乐观消息，也保留已真实入库的用户请求。
          const page = await listMessagesPage(conv.id).catch(() => null)
          if (page && get().currentConversation?.id === conv.id) {
            const pendingQueued = get().messages.filter(
              (message) => message.queued === 1 && message.id.startsWith('local-'),
            )
            const persistedIds = new Set(page.messages.map((message) => message.id))
            set({
              messages: [
                ...page.messages,
                ...pendingQueued.filter((message) => !persistedIds.has(message.id)),
              ],
              olderHasMore: page.hasMore,
            })
          }
        }
      }
    },

    stopGeneration: async () => {
      const conv = get().currentConversation
      if (!conv) return
      const cid = conv.id
      try {
        await stopChatApi(cid)
      } catch {
        // 忽略：后端在安全点自行退出
      }
      // 停止兜底：后端任务若已死（线程卡死、join 永不返回），invoke 永不 reject，
      // 前端须在宽限期后自行释放流式桶，否则界面永久转圈且无法再发消息。
      // 用代次 token（startedAt）校验：用户停止后若立即重新发送，新桶的 startedAt
      // 不同，旧计时器不会误杀新任务。
      const bucket = get().streamings[cid]
      if (!bucket) return
      const token = bucket.startedAt
      const deadline = Date.now() + 60 * 1000
      const timer = setInterval(() => {
        const cur = get().streamings[cid]
        if (!cur || cur.startedAt !== token) {
          clearInterval(timer)
          return
        }
        if (Date.now() >= deadline) {
          clearInterval(timer)
          setBucket(cid, {
            ...emptyStreaming(),
            error: '停止未生效：后端任务已无响应。请查看应用日志定位卡点，或重启应用后重试',
          })
        }
      }, 10 * 1000)
    },

    /** 停止当前正在执行的工具：强杀子进程，模型拿到中断反馈后继续生成结论（不终止任务） */
    stopCurrentTool: async () => {
      const conv = get().currentConversation
      if (!conv) return
      try {
        await stopToolApi(conv.id)
      } catch {
        // 忽略：后端轮询中断标志自行处理
      }
    },

    /** 流式运行中提交消息进入排队：乐观展示（带排队标记）→ 后端入库 queued=1
     *  → 返回真实消息替换本地占位。普通排队（agentOwned=false）当前任务结束后自动续跑；
     * 发送到 Agent（agentOwned=true）由 Agent 在任务内安全点并入当前任务。 */
    queueUserMessage: async (content, agentOwned, references, images) => {
      const conv = get().currentConversation
      if (!conv) return
      const now = Math.floor(Date.now() / 1000)
      const optimistic: ChatMessage = {
        id: `local-${now}-${Math.random().toString(36).slice(2, 6)}`,
        conversation_id: conv.id,
        role: 'user',
        content,
        references_json: references?.length ? JSON.stringify(references) : null,
        model: null,
        tokens_in: null,
        tokens_out: null,
        reasoning: null,
        queued: 1,
        agent_owned: agentOwned ? 1 : 0,
        modified_files_json: null,
        duration_ms: null,
        created_at: now,
      }
      set({ messages: [...get().messages, optimistic] })
      try {
        const saved = await queueMessageApi(conv.id, content, agentOwned, references, images)
        if (get().currentConversation?.id !== conv.id) return
        // 用后端返回的真实消息替换本地占位（id 与时间戳以数据库为准）
        set({ messages: get().messages.map((m) => (m.id === optimistic.id ? saved : m)) })
      } catch (e) {
        // 入库失败：移除乐观消息，避免界面残留无效的排队条目
        if (get().currentConversation?.id === conv.id) {
          set({ messages: get().messages.filter((m) => m.id !== optimistic.id) })
        }
        throw e
      }
    },

    editMessage: async (messageId, content) => {
      const conversationId = get().currentConversation?.id
      await updateMessageApi(messageId, content)
      // 全量刷新（编辑影响消息位置未知）：已加载全部，终止分页
      if (!conversationId) return
      const messages = await listMessages(conversationId)
      if (get().currentConversation?.id === conversationId) set({ messages, olderHasMore: false })
    },

    removeMessage: async (messageId) => {
      const conversationId = get().currentConversation?.id
      await deleteMessageApi(messageId)
      if (!conversationId) return
      const messages = await listMessages(conversationId)
      if (get().currentConversation?.id === conversationId) set({ messages, olderHasMore: false })
    },

    /** 回复工具权限审核：true=允许执行 / false=拒绝（可附理由）；remember 勾选后本会话同工具免审，scope=project 额外写入项目白名单 */
    resolveToolApproval: async (requestId, approved, remember, feedback, scope) => {
      try {
        await resolveToolApprovalApi(requestId, approved, remember, feedback, scope)
      } catch {
        // 超时/失效请求：后端按拒绝处理，这里同样移除即可
      } finally {
        removePendingByRequestId(requestId)
        set((s) => ({ toolApprovals: s.toolApprovals.filter((a) => a.requestId !== requestId) }))
      }
    },

    /** 拉取项目内所有会话的待确认项（审批/计划/提问），刷新会话列表角标与恢复数据 */
    refreshPendingConfirmations: async () => {
      const project = get().currentProject
      if (!project) return
      try {
        const items = await listPendingConfirmationsApi(project.id)
        const map: Record<string, PendingConfirmation[]> = {}
        for (const it of items) {
          const list = map[it.conversation_id] ?? []
          list.push(it)
          map[it.conversation_id] = list
        }
        set({ pendingConfirmations: map })
      } catch {
        // 后端不可用时保留旧数据
      }
    },

    /** 回复计划审查：批准执行或驳回（可附带修改意见） */
    resolvePlanReview: async (requestId, approved, feedback) => {
      const conv = get().currentConversation
      const pending = get().pendingPlan
      try {
        if (conv) {
          await resolvePlanReviewApi(conv.id, requestId, approved, feedback)
        }
      } catch {
        // 超时/失效：后端按驳回处理
      } finally {
        // 驳回时保留计划内容直到收到 resolved 事件也无妨；这里先清空，事件里再兜底
        removePendingByRequestId(requestId)
        if (pending?.requestId === requestId) {
          set({ pendingPlan: null })
        }
      }
    },

    /** 回复 Agent 提问：answer 为空串表示跳过（已超时/已处理时后端拒绝，卡片仍关闭） */
    resolveAskUser: async (requestId, answer) => {
      try {
        await resolveAskUserApi(requestId, answer)
      } catch {
        // 已超时或已处理：忽略后端错误
      } finally {
        removePendingByRequestId(requestId)
        if (get().askCard?.requestId === requestId) {
          set({ askCard: null })
        }
      }
    },

    /** 重新生成：messageId 指定时从该 user 消息分支重生成（丢弃其后主线并归档旧回复）；
     *  未指定则重新生成最后一条回复（删除旧回复 → 以最后一条 user 消息重新流式） */
    regenerateLast: async (options, messageId) => {
      const conv = get().currentConversation
      if (!conv) return
      // 同会话流式中防重（error 态允许重试；跨会话流式不影响本会话重新生成）
      const existing = get().streamings[conv.id]
      if (existing && !existing.error) return
      // 开始重新生成性能追踪
      const sendTrace = startPerfTrace('regenerateMessage', { conv: conv.id.slice(0, 8) })
      sendTraces.set(conv.id, sendTrace)
      // 分支模式：校验指定消息存在；否则取最后一条 user 消息
      const all = get().messages
      const lastUser = messageId
        ? all.find((m) => m.id === messageId && m.role === 'user')
        : [...all].reverse().find((m) => m.role === 'user')
      if (!lastUser) {
        sendTraces.delete(conv.id)
        sendTrace.mark('no-user-message')
        sendTrace.end()
        return
      }
      const fresh: StreamingState = { ...emptyStreaming(), conversationId: conv.id, startedAt: Date.now(), lastDeltaAt: Date.now() }
      set({
        streamings: { ...get().streamings, [conv.id]: fresh },
        streaming: fresh,
        toolRuns: [],
        terminalEntries: [],
        buildLogs: [],
        agentRuns: [],
        plan: null,
        todos: [],
        askCard: null,
        lastTaskSummary: null,
      })
      sendTrace.mark('stream-started')
      try {
        // 后端 regenerate 模式会删除旧回复，先刷新消息列表保持界面一致（全量：终止分页）
        await streamChatApi(conv.id, lastUser.content, options, true, undefined, undefined, messageId)
        sendTrace.mark('stream-finished')
        const messages = await listMessages(conv.id)
        if (get().currentConversation?.id === conv.id) set({ messages, olderHasMore: false })
        await get().loadVersions(conv.id)
        sendTrace.mark('post-refresh')
      } catch (e) {
        sendTraces.delete(conv.id)
        sendTrace.mark('error')
        sendTrace.end()
        // 失败时也刷新消息（regenerate 模式可能已删除旧回复），保持界面一致
        const messages = await listMessages(conv.id).catch(() => null)
        if (messages && get().currentConversation?.id === conv.id) set({ messages, olderHasMore: false })
        if (get().streamings[conv.id]?.startedAt === fresh.startedAt) {
          setBucket(conv.id, { ...emptyStreaming(), error: String(e) })
        }
      }
    },

    refreshQueued: async (conversationId) => {
      try {
        const list = await listQueuedMessagesApi(conversationId)
        set({ queuedList: list })
      } catch {
        // 失败保留旧数据
      }
    },

    removeQueued: async (messageId) => {
      const conv = get().currentConversation
      if (!conv) return
      try {
        await removeQueuedMessageApi(conv.id, messageId)
        set((s) => ({ queuedList: s.queuedList.filter((q) => q.id !== messageId) }))
      } catch {
        // 失败静默（下次刷新自愈）
      }
    },

    renameConversation: async (id, title) => {
      await renameConversationApi(id, title)
      const projectId = get().currentConversation?.project_id
      if (projectId) {
        const conversations = await listConversations(projectId)
        set({
          conversations,
          currentConversation:
            get().currentConversation?.id === id
              ? { ...get().currentConversation!, title }
              : get().currentConversation,
        })
      }
    },

    deleteConversation: async (id) => {
      await deleteConversationApi(id)
      const projectId = get().currentConversation?.project_id
      if (!projectId) return
      const isCurrent = get().currentConversation?.id === id
      const conversations = await listConversations(projectId)
      // 流式分桶随会话清理（防泄漏；删除正在流式的会话由后端停止任务）
      {
        const streamings = { ...get().streamings }
        delete streamings[id]
        set({ streamings })
      }
      if (isCurrent) {
        // 删除当前会话后自动打开第一个会话
        set({ conversations, currentConversation: null, messages: [], streaming: emptyStreaming() })
        if (conversations.length > 0) {
          await get().openConversation(conversations[0].id)
        }
      } else {
        set({ conversations })
      }
    },

    pinConversation: async (id, pinned) => {
      await pinConversationApi(id, pinned)
      const projectId = get().currentConversation?.project_id
      if (projectId) {
        const conversations = await listConversations(projectId)
        set({ conversations })
      }
    },

    archiveConversation: async (id, archived) => {
      await archiveConversationApi(id, archived)
      const projectId = get().currentConversation?.project_id
      if (!projectId) return
      // 归档视图只展示已归档会话；正常视图展示未归档会话
      const includeArchived = get().conversations.some((c) => c.archived)
      const conversations = await listConversations(projectId, includeArchived)
      set({ conversations })
    },
  }
}
