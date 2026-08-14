import { listen } from '@tauri-apps/api/event'
import { sendNotification } from '../../api/desktop'
import {
  listConversations,
  createConversation,
  listMessages,
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
  getAsk as getAskApi,
  resolveAskUser as resolveAskUserApi,
  renameConversation as renameConversationApi,
  deleteConversation as deleteConversationApi,
  pinConversation as pinConversationApi,
  archiveConversation as archiveConversationApi,
  rollbackConversation,
} from '../../api/project'
import type { ChatMessage, TodoItem } from '../../api/project'
import type { StateCreator } from 'zustand'
import type { ChatSlice, DiagnoseCard, ProjectState } from '../projectStoreTypes'
import { advancePlan } from './chatUtils'

/** 后端异常兜底看门狗句柄（模块级，armStreamWatchdog 使用） */
let streamWatchdog: ReturnType<typeof setTimeout> | null = null

// Agent 工具 / 子 Agent 事件自增序号（模块级）
let toolSeq = 0
let agentSeq = 0

/** 会话/消息/流式/审批/计划切片实现（含全局事件监听注册，store 创建时执行一次） */
export const createChatSlice: StateCreator<ProjectState, [], [], ChatSlice> = (set, get) => {
  /**
   * 后端异常兜底看门狗：发送后若长时间（4 分钟）无任何内容且无工具/审批事件，
   * 自动结束流式状态并提示——防止后端异常挂起时前端永久停留在"正在输入…"。
   * 正常流式（有内容/工具事件）不会触发；chat-done/error/stopped 后状态已重置也不会触发。
   */
  const armStreamWatchdog = (convId: string) => {
    if (streamWatchdog) clearTimeout(streamWatchdog)
    streamWatchdog = setTimeout(() => {
      const s = get()
      if (
        s.streaming.conversationId === convId &&
        !s.streaming.content &&
        s.toolRuns.length === 0 &&
        s.toolApprovals.length === 0
      ) {
        set({
          streaming: {
            conversationId: null,
            content: '',
            reasoning: '',
            error: '后端长时间无响应，已自动停止等待。请检查模型配置与网络后重试',
            errorDetail: null,
            startedAt: null,
            lastDeltaAt: null,
          },
          toolRuns: [],
          agentRuns: [],
          // 超时停止：进度卡定档保留（展示已完成部分）
          plan: s.plan && s.plan.phase === 'running' ? { ...s.plan, phase: 'error' } : s.plan,
        })
      }
    }, 4 * 60 * 1000)
  }

  // ---------- 流式事件监听（全局一次性注册） ----------
  listen<{ conversation_id: string; delta: string }>('chat-stream', (event) => {
    const { conversation_id, delta } = event.payload
    const state = get()
    if (state.streaming.conversationId !== conversation_id) return
    set({
      streaming: { ...state.streaming, content: state.streaming.content + delta, lastDeltaAt: Date.now() },
    })
  }).catch(() => {})

  // 思考过程流式增量（推理模型 reasoning_content 透传）
  listen<{ conversation_id: string; delta: string }>('chat-reasoning', (event) => {
    const { conversation_id, delta } = event.payload
    const state = get()
    if (state.streaming.conversationId !== conversation_id) return
    set({
      streaming: { ...state.streaming, reasoning: state.streaming.reasoning + delta, lastDeltaAt: Date.now() },
    })
  }).catch(() => {})

  listen<{ conversation_id: string; message: ChatMessage; unfinished: boolean }>('chat-done', (event) => {
    const { conversation_id, message, unfinished } = event.payload
    const state = get()
    if (state.streaming.conversationId !== conversation_id) return
    // 任务结束摘要（ChatGPT 式收尾统计）：耗时 + 工具调用数 + 修改文件数
    const toolCount = state.toolRuns.length
    const durationMs = state.streaming.startedAt ? Date.now() - state.streaming.startedAt : 0
    const fileCount = (() => {
      try {
        const v = message.modified_files_json ? JSON.parse(message.modified_files_json) : []
        return Array.isArray(v) ? v.length : 0
      } catch {
        return 0
      }
    })()
    set({
      messages: [...state.messages, message],
      streaming: { conversationId: null, content: '', reasoning: '', error: null, errorDetail: null, startedAt: null, lastDeltaAt: null },
      // 完成后保留过程记录：顶部"已处理 N 个操作"徽章可展开回看（新任务/切会话时清空）
      lastTaskSummary: {
        durationMs,
        toolCount,
        fileCount,
        // 后端把任务累计 token 持久化到结束消息（每轮求和），此处直接取用
        tokensIn: message.tokens_in ?? 0,
        tokensOut: message.tokens_out ?? 0,
      },
      // 任务正常结束：进度卡定档保留
      plan: state.plan && state.plan.phase === 'running' ? { ...state.plan, phase: 'done' } : state.plan,
      // 任务未完成（上限中止/用户停止/中途失败，有工具成果无最终总结）：
      // 保留"继续任务"按钮断点续跑；正常完成则清空
      unfinishedConv: unfinished ? { conversationId: conversation_id } : null,
    })
    // 刷新会话列表（标题/时间变化；保持搜索关键字过滤）
    const projectId = state.currentConversation?.project_id
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

  listen<{ conversation_id: string; error: string; kind: string; title: string; reason: string; suggestion: string; retryable: boolean; status_code?: number | null }>(
    'chat-error',
    (event) => {
      const { conversation_id, error, kind, title, reason, suggestion, retryable, status_code } = event.payload
      const state = get()
      if (state.streaming.conversationId !== conversation_id) return
      set({
        streaming: {
          ...state.streaming,
          error,
          errorDetail: { kind, title, reason, suggestion, retryable, statusCode: status_code ?? null },
        },
        // 任务出错：进度卡立即定档（保留已完成步骤）
        plan: state.plan && state.plan.phase === 'running' ? { ...state.plan, phase: 'error' } : state.plan,
        // 任务结束：挂起的提问卡无意义，一并关闭
        askCard: null,
      })
    },
  ).catch(() => {})

  // 用户停止且无内容可入库：清空流式状态（有内容时后端走 chat-done）
  listen<{ conversation_id: string; unfinished: boolean }>('chat-stopped', (event) => {
    const { conversation_id, unfinished } = event.payload
    const state = get()
    if (state.streaming.conversationId !== conversation_id) return
    const partial = state.streaming.content.trim()
    // 已流式输出的内容不因无正文入库而消失：作为临时消息追加展示（不落库，刷新会话后回到真实历史）
    const messages =
      partial.length > 0
        ? [
            ...state.messages,
            {
              id: `local-stop-${Date.now()}`,
              conversation_id,
              role: 'assistant' as const,
              content: partial,
              references_json: null,
              model: null,
              tokens_in: null,
              tokens_out: null,
              reasoning: state.streaming.reasoning || null,
              queued: 0,
              agent_owned: 0,
              modified_files_json: null,
              duration_ms: null,
              created_at: Math.floor(Date.now() / 1000),
            } satisfies ChatMessage,
          ]
        : state.messages
    set({
      messages,
      streaming: { conversationId: null, content: '', reasoning: '', error: null, errorDetail: null, startedAt: null, lastDeltaAt: null },
      // 停止后同样保留过程记录：已执行部分可在徽章中展开回看
      // 用户停止：进度卡定档保留（展示已完成部分）
      plan: state.plan && state.plan.phase === 'running' ? { ...state.plan, phase: 'done' } : state.plan,
      // 停止且未完成：展示"继续任务"按钮断点续跑（有已执行工具成果可接续）
      unfinishedConv: unfinished ? { conversationId: conversation_id } : state.unfinishedConv,
      // 停止任务：挂起的提问卡同步关闭（后端已关闭通道）
      askCard: null,
    })
  }).catch(() => {})

  // ---------- Agent 工具事件 ----------
  listen<{ conversation_id: string; tool: string; args: string; round?: number; total?: number; level?: string; desc?: string }>(
    'chat-tool-start',
    (event) => {
      const { conversation_id, tool, args, round, total, level, desc } = event.payload
      const state = get()
      if (state.streaming.conversationId !== conversation_id) return
      set({
        toolRuns: [
          ...state.toolRuns,
          {
            id: `tool-${Date.now()}-${toolSeq++}`,
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
            id: `term-${Date.now()}-${toolSeq}`,
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

  listen<{ conversation_id: string; tool: string; ok: boolean; output: string; duration_ms?: number }>('chat-tool-done', (event) => {
    const { conversation_id, tool, ok, output, duration_ms } = event.payload
    const state = get()
    if (state.streaming.conversationId !== conversation_id) return
    set({
      toolRuns: state.toolRuns.map((r) =>
        r.tool === tool && r.status === 'running'
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
        const runningIdx = state.terminalEntries.map((x) => x.tool).lastIndexOf(tool)
        if (idx !== runningIdx || e.status !== 'running') return e
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

  // 工具权限审核请求（自动审核模式）：入队等待用户确认弹窗
  listen<{ conversation_id: string; request_id: string; tool: string; args: string }>(
    'chat-tool-approval',
    (event) => {
      const { conversation_id, request_id, tool, args } = event.payload
      const state = get()
      if (state.streaming.conversationId !== conversation_id) return
      set({
        toolApprovals: [...state.toolApprovals, { requestId: request_id, tool, args }],
      })
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
    const state = get()
    if (state.streaming.conversationId !== conversation_id) return
    set({
      pendingPlan: { requestId: request_id, conversationId: conversation_id, plan },
    })
  }).catch(() => {})

  // 计划已被处理（批准/驳回）：清空待确认状态；批准时把计划保留为"已批准计划"执行中展示
  listen<{ conversation_id: string; approved: boolean }>('chat-plan-resolved', (event) => {
    const state = get()
    if (state.pendingPlan?.conversationId !== event.payload.conversation_id) return
    const plan = state.pendingPlan.plan
    set({
      pendingPlan: null,
      approvedPlan: event.payload.approved
        ? { conversationId: event.payload.conversation_id, plan }
        : state.approvedPlan,
    })
  }).catch(() => {})

  // ---------- 子 Agent 事件 ----------
  listen<{ conversation_id: string; name: string; model: string }>('chat-agent-start', (event) => {
    const { conversation_id, name, model } = event.payload
    const state = get()
    if (state.streaming.conversationId !== conversation_id) return
    set({
      agentRuns: [
        ...state.agentRuns,
        { id: `agent-${Date.now()}-${agentSeq++}`, name, model, status: 'running', output: '' },
      ],
    })
  }).catch(() => {})

  listen<{ conversation_id: string; name: string; model: string; ok: boolean; output: string }>(
    'chat-agent-done',
    (event) => {
      const { conversation_id, name, ok, output } = event.payload
      const state = get()
      if (state.streaming.conversationId !== conversation_id) return
      set({
        agentRuns: state.agentRuns.map((r) =>
          r.name === name && r.status === 'running' ? { ...r, status: ok ? 'done' : 'error', output } : r,
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
    const { job_id, command, ok, summary } = event.payload
    const state = get()
    const line = `[后台任务 ${job_id}] ${ok ? '完成' : '失败'}：${command}（${summary}）`
    const next = [...state.buildLogs, { stream: 'system' as const, line, ts: Date.now() }]
    const trimmed = next.length > 2000 ? next.slice(next.length - 2000) : next
    set({ buildLogs: trimmed })
    sendNotification(ok ? '后台任务完成' : '后台任务失败', `${command}｜${summary}`, ok ? 'success' : 'error').catch(
      () => {},
    )
  }).catch(() => {})

  // 构建/部署流式日志（agent:log）：按行累积到 buildLogs，任务开始时清空，保留最近 2000 行；
  // 同时把行追加到当前运行中工具的 liveOutput（工具卡片实时可见执行过程）
  listen<{ conversation_id: string; stream: string; line: string }>('agent:log', (event) => {
    const { conversation_id, stream, line } = event.payload
    const state = get()
    if (state.streaming.conversationId !== conversation_id) return
    const next = [...state.buildLogs, { stream, line, ts: Date.now() }]
    // 超长时丢弃旧的头部，避免长时间构建导致内存膨胀
    const trimmed = next.length > 2000 ? next.slice(next.length - 2000) : next
    set({
      buildLogs: trimmed,
      // 追加到最近一个运行中工具的实时输出（仅 stdout/stderr；system 提示不进卡片）
      toolRuns: stream === 'system'
        ? state.toolRuns
        : (() => {
            const lastRunning = state.toolRuns.reduce((acc, x, i) => (x.status === 'running' ? i : acc), -1)
            if (lastRunning < 0) return state.toolRuns
            return state.toolRuns.map((r, idx) => {
              if (idx !== lastRunning) return r
              const prev = r.liveOutput ?? ''
              // 保留尾部（约 8 万字符），避免超长构建撑爆渲染
              const capped = prev.length > 80000 ? prev.slice(prev.length - 40000) : prev
              return { ...r, liveOutput: capped + line + '\n' }
            })
          })(),
      // 终端面板同步：同一运行中条目的实时输出
      terminalEntries: stream === 'system'
        ? state.terminalEntries
        : (() => {
            const lastRunning = state.terminalEntries.reduce((acc, x, i) => (x.status === 'running' ? i : acc), -1)
            if (lastRunning < 0) return state.terminalEntries
            return state.terminalEntries.map((e, idx) => {
              if (idx !== lastRunning) return e
              const prev = e.liveOutput ?? ''
              const capped = prev.length > 80000 ? prev.slice(prev.length - 40000) : prev
              return { ...e, liveOutput: capped + line + '\n' }
            })
          })(),
    })
  }).catch(() => {})

  // 任务清单（todo_write）：后端推送合并后的完整清单，前端实时渲染进度
  listen<{ conversation_id: string; todos: TodoItem[] }>('agent:todo', (event) => {
    const { conversation_id, todos } = event.payload
    const state = get()
    if (state.streaming.conversationId !== conversation_id) return
    set({ todos })
  }).catch(() => {})

  // Agent 提问（ask_user）：渲染提问卡等待用户回答
  listen<{ conversation_id: string; request_id: string; question: string; options: string[] }>(
    'chat-ask',
    (event) => {
      const { conversation_id, request_id, question, options } = event.payload
      const state = get()
      if (state.streaming.conversationId !== conversation_id) return
      set({
        askCard: { requestId: request_id, conversationId: conversation_id, question, options },
      })
      // 后端 5 分钟超时兜底：超时后前端残留卡片自动关闭
      setTimeout(() => {
        const cur = get()
        if (cur.askCard?.requestId === request_id) {
          set({ askCard: null })
        }
      }, 5 * 60 * 1000)
    },
  ).catch(() => {})

  return {
    conversations: [],
    currentConversation: null,
    messages: [],
    streaming: { conversationId: null, content: '', reasoning: '', error: null, errorDetail: null, startedAt: null, lastDeltaAt: null },
    toolRuns: [],
    terminalEntries: [],
    buildLogs: [],
    agentRuns: [],
    plan: null,
    toolApprovals: [],
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

    rollbackTask: async (conversationId, dryRun) => {
      const info = await rollbackConversation(conversationId, dryRun)
      if (!dryRun) {
        // 回滚后工作区已变：刷新 Git 面板状态，文件树目录内容也可能变化
        get().refreshGitBranches().catch(() => {})
        get().rebuildIndex().catch(() => {})
      }
      return info
    },

    newConversation: async () => {
      const project = get().currentProject
      if (!project) return
      const conv = await createConversation(project.id)
      const conversations = await listConversations(project.id)
      // 新会话：清空上一会话的过程记录（完成态徽章不复用）
      set({ conversations, currentConversation: conv, messages: [], plan: null, toolRuns: [], agentRuns: [], approvedPlan: null, unfinishedConv: null, todos: [], askCard: null })
    },

    openConversation: async (id) => {
      set({
        currentConversation: get().conversations.find((c) => c.id === id) ?? null,
        plan: null,
        lastTaskSummary: null,
        approvedPlan: null,
        unfinishedConv: null,
        // 切换会话：清空上一会话的过程记录，避免完成态徽章残留
        toolRuns: [],
        agentRuns: [],
        todos: [],
        askCard: null,
      })
      const messages = await listMessages(id)
      set({ messages })
      await get().loadFeedback(id)
      await get().loadVersions(id)
      await get().loadTokenStats(id)
      // 恢复该会话的任务清单与挂起提问（若有）
      void getTodosApi(id).then((todos) => set({ todos })).catch(() => {})
      void getAskApi(id)
        .then((ask) =>
          set({
            askCard: ask
              ? { requestId: ask.request_id, conversationId: ask.conversation_id, question: ask.question, options: ask.options }
              : null,
          }),
        )
        .catch(() => {})
    },

    // 新任务开始时清空上次任务摘要（防止跨任务残留展示）
    sendUserMessage: async (content, options, references, images) => {
      const conv = get().currentConversation
      if (!conv) return
      if (get().streaming.conversationId) return // 流式中防重
      // 乐观展示 user 消息（Rust 端同样入库）
      const now = Math.floor(Date.now() / 1000)
      set({
        messages: [
          ...get().messages,
          {
            id: `local-${now}`,
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
        streaming: { conversationId: conv.id, content: '', reasoning: '', error: null, errorDetail: null, startedAt: Date.now(), lastDeltaAt: Date.now() },
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
      armStreamWatchdog(conv.id)
      try {
        await streamChatApi(conv.id, content, options, false, references, images)
      } catch (e) {
        set({ streaming: { conversationId: null, content: '', reasoning: '', error: String(e), errorDetail: null, startedAt: null, lastDeltaAt: null } })
      }
    },

    stopGeneration: async () => {
      const conv = get().currentConversation
      if (!conv) return
      try {
        await stopChatApi(conv.id)
      } catch {
        // 忽略：后端在安全点自行退出
      }
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
        // 用后端返回的真实消息替换本地占位（id 与时间戳以数据库为准）
        set({ messages: get().messages.map((m) => (m.id === optimistic.id ? saved : m)) })
      } catch (e) {
        // 入库失败：移除乐观消息，避免界面残留无效的排队条目
        set({ messages: get().messages.filter((m) => m.id !== optimistic.id) })
        throw e
      }
    },

    editMessage: async (messageId, content) => {
      await updateMessageApi(messageId, content)
      const conv = get().currentConversation
      if (conv) set({ messages: await listMessages(conv.id) })
    },

    removeMessage: async (messageId) => {
      await deleteMessageApi(messageId)
      const conv = get().currentConversation
      if (conv) set({ messages: await listMessages(conv.id) })
    },

    /** 回复工具权限审核：true=允许执行 / false=拒绝（可附理由）；remember 勾选后本会话同工具免审，scope=project 额外写入项目白名单 */
    resolveToolApproval: async (requestId, approved, remember, feedback, scope) => {
      try {
        await resolveToolApprovalApi(requestId, approved, remember, feedback, scope)
      } catch {
        // 超时/失效请求：后端按拒绝处理，这里同样移除即可
      } finally {
        set({ toolApprovals: get().toolApprovals.filter((a) => a.requestId !== requestId) })
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
      if (get().streaming.conversationId) return
      // 分支模式：校验指定消息存在；否则取最后一条 user 消息
      const all = get().messages
      const lastUser = messageId
        ? all.find((m) => m.id === messageId && m.role === 'user')
        : [...all].reverse().find((m) => m.role === 'user')
      if (!lastUser) return
      set({
        streaming: { conversationId: conv.id, content: '', reasoning: '', error: null, errorDetail: null, startedAt: Date.now(), lastDeltaAt: Date.now() },
        toolRuns: [],
        terminalEntries: [],
        buildLogs: [],
        agentRuns: [],
        plan: null,
        todos: [],
        askCard: null,
        lastTaskSummary: null,
      })
      armStreamWatchdog(conv.id)
      try {
        // 后端 regenerate 模式会删除旧回复，先刷新消息列表保持界面一致
        await streamChatApi(conv.id, lastUser.content, options, true, undefined, undefined, messageId)
        const messages = await listMessages(conv.id)
        set({ messages })
        await get().loadVersions(conv.id)
      } catch (e) {
        // 失败时也刷新消息（regenerate 模式可能已删除旧回复），保持界面一致
        set({ messages: await listMessages(conv.id).catch(() => get().messages) })
        set({ streaming: { conversationId: null, content: '', reasoning: '', error: String(e), errorDetail: null, startedAt: null, lastDeltaAt: null } })
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
      if (isCurrent) {
        // 删除当前会话后自动打开第一个会话
        set({ conversations, currentConversation: null, messages: [] })
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
