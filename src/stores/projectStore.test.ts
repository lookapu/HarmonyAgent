import { beforeEach, describe, expect, it, vi } from 'vitest'

const { listeners } = vi.hoisted(() => ({
  listeners: new Map<string, (event: { payload: unknown }) => void>(),
}))

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn((name: string, cb: (event: { payload: unknown }) => void) => {
    listeners.set(name, cb)
    return Promise.resolve(() => {})
  }),
}))

vi.mock('../api/project', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../api/project')>()
  return {
    ...actual,
    listProjects: vi.fn(),
    listConversations: vi.fn(),
    deleteConversation: vi.fn(),
    deleteMessage: vi.fn(),
    listMessages: vi.fn(),
    listMessagesPage: vi.fn(),
    getConversationCostStats: vi.fn(),
    listSnapshots: vi.fn(),
    listQueuedMessages: vi.fn(),
    restoreSnapshot: vi.fn(),
    getTaskLedger: vi.fn(),
    listMessageFeedback: vi.fn(),
    listMessageVersions: vi.fn(),
    listPendingConfirmations: vi.fn(),
    getTodos: vi.fn().mockResolvedValue([]),
    streamChat: vi.fn().mockResolvedValue(undefined),
    stopChat: vi.fn().mockResolvedValue(undefined),
  }
})

import {
  deleteConversation,
  deleteMessage,
  getConversationCostStats,
  getTaskLedger,
  getTodos,
  listConversations,
  listMessageFeedback,
  listMessageVersions,
  listMessages,
  listMessagesPage,
  listPendingConfirmations,
  listProjects,
  listQueuedMessages,
  listSnapshots,
  restoreSnapshot,
  streamChat,
} from '../api/project'
import { emptyStreaming } from './slices/chatSlice'
import { useProjectStore } from './projectStore'

const project = (id: string) =>
  ({ id, name: id, path: `/${id}`, kind: 'harmony', trusted: true, index_state: 'ready', last_opened_at: 1, created_at: 1, conversation_count: 0, pinned: false }) as never
const conversation = (id: string) =>
  ({ id, project_id: 'p1', title: id, created_at: 1, updated_at: 1 }) as never

/** 填满所有“当前会话视图”字段：删除当前会话后这些值都不允许残留。 */
const populatedConversationView = () => ({
  conversations: [conversation('c1')],
  currentConversation: conversation('c1'),
  messages: [{ id: 'm1' } as never],
  olderHasMore: true,
  loadingOlder: true,
  streamings: { c1: emptyStreaming() },
  pendingConfirmations: { c1: [{ request_id: 'r1' } as never] },
  genStatus: { c1: { kind: 'image', stage: 'running', waitedSecs: 1 } as never },
  taskLedgers: { c1: { ledger: null, finished: true } as never },
  toolRuns: [{ id: 't1' } as never],
  terminalEntries: [{ id: 1 } as never],
  buildLogs: [{ id: 1, stream: 'stdout', line: 'x', ts: 1 } as never],
  agentRuns: [{ id: 'a1' } as never],
  plan: { phase: 'running', steps: [] } as never,
  toolApprovals: [{ requestId: 'r1' } as never],
  pendingPlan: { requestId: 'p1', conversationId: 'c1', plan: 'x' } as never,
  approvedPlan: { conversationId: 'c1', plan: 'x' } as never,
  todos: [{ id: 'todo' } as never],
  askCard: { requestId: 'q1', conversationId: 'c1', question: 'q', options: [] } as never,
  diagnoseCards: [{ id: 'd1' } as never],
  queuedList: [{ id: 'q1', content: 'x' } as never],
  unfinishedConv: { conversationId: 'c1' } as never,
  lastTaskSummary: { status: 'completed', durationMs: 1, toolCount: 1, fileCount: 0, tokensIn: 1, tokensOut: 1 } as never,
  snapshots: [{ id: 's1' } as never],
  loadingSnapshots: true,
  feedbackMap: { m1: { message_id: 'm1' } as never },
  versionMap: { m1: [{ id: 'v1' } as never] },
  tokenStats: { total_tokens: 10 } as never,
})

const expectViewCleared = () => {
  const state = useProjectStore.getState()
  expect(state.conversations).toEqual([])
  expect(state.currentConversation).toBeNull()
  expect(state.messages).toEqual([])
  expect(state.olderHasMore).toBe(false)
  expect(state.loadingOlder).toBe(false)
  expect(state.streamings).toEqual({})
  expect(state.pendingConfirmations).toEqual({})
  expect(state.genStatus).toEqual({})
  expect(state.taskLedgers).toEqual({})
  expect(state.toolRuns).toEqual([])
  expect(state.terminalEntries).toEqual([])
  expect(state.buildLogs).toEqual([])
  expect(state.agentRuns).toEqual([])
  expect(state.plan).toBeNull()
  expect(state.toolApprovals).toEqual([])
  expect(state.pendingPlan).toBeNull()
  expect(state.approvedPlan).toBeNull()
  expect(state.todos).toEqual([])
  expect(state.askCard).toBeNull()
  expect(state.diagnoseCards).toEqual([])
  expect(state.queuedList).toEqual([])
  expect(state.unfinishedConv).toBeNull()
  expect(state.lastTaskSummary).toBeNull()
  expect(state.snapshots).toEqual([])
  expect(state.loadingSnapshots).toBe(false)
  expect(state.feedbackMap).toEqual({})
  expect(state.versionMap).toEqual({})
  expect(state.tokenStats).toBeNull()
}

describe('project deletion sync', () => {
  beforeEach(() => {
    useProjectStore.setState({
      projects: [project('p1')],
      currentProject: project('p1'),
      gitBranches: {} as never,
      fileTree: { name: '', path: '', type: 'dir', children: [] } as never,
      dirCache: { '': [] },
    })
  })

  it('refreshProjects 清空已消失的当前项目', async () => {
    vi.mocked(listProjects).mockResolvedValue([project('p2')])
    await useProjectStore.getState().refreshProjects()
    const state = useProjectStore.getState()
    expect(state.currentProject).toBeNull()
    expect(state.gitBranches).toBeNull()
    expect(state.fileTree).toBeNull()
    expect(state.dirCache).toEqual({})
    expect(state.projects.map((p) => p.id)).toEqual(['p2'])
  })

  it('project-deleted 一次性清理该项目全部会话的运行态', () => {
    useProjectStore.setState({
      conversations: [conversation('c1'), conversation('c2')],
      currentConversation: conversation('c2'),
      streamings: { c1: emptyStreaming(), c2: emptyStreaming() },
      pendingConfirmations: { c1: {} as never, c2: {} as never },
      genStatus: { c1: 'running', c2: 'running' } as never,
      taskLedgers: { c1: {} as never, c2: {} as never },
      messages: [{ id: 'm1' } as never],
      toolApprovals: [{ requestId: 'r1' } as never],
      todos: [{ id: 'todo' } as never],
    })

    listeners.get('project-deleted')!({
      payload: { project_id: 'p1', conversation_ids: ['c1', 'c2'] },
    })

    const state = useProjectStore.getState()
    expect(state.conversations).toEqual([])
    expect(state.currentConversation).toBeNull()
    expect(state.streamings).toEqual({})
    expect(state.pendingConfirmations).toEqual({})
    expect(state.genStatus).toEqual({})
    expect(state.taskLedgers).toEqual({})
    expect(state.messages).toEqual([])
    expect(state.toolApprovals).toEqual([])
    expect(state.todos).toEqual([])
  })

  it('conversation-deleted 删除当前（最后一个）会话时复位全部视图状态', () => {
    useProjectStore.setState(populatedConversationView())

    listeners.get('conversation-deleted')!({ payload: { conversation_id: 'c1' } })

    expectViewCleared()
  })

  it('deleteConversation 删除最后一个会话后复位视图并清理运行态', async () => {
    vi.mocked(deleteConversation).mockResolvedValue(undefined)
    vi.mocked(listConversations).mockResolvedValue([])
    useProjectStore.setState(populatedConversationView())

    await useProjectStore.getState().deleteConversation('c1')

    expectViewCleared()
  })

  it('deleteConversation 列表刷新失败时仍复位并从本地移除', async () => {
    vi.mocked(deleteConversation).mockResolvedValue(undefined)
    vi.mocked(listConversations).mockRejectedValue(new Error('offline'))
    useProjectStore.setState(populatedConversationView())

    await useProjectStore.getState().deleteConversation('c1')

    expectViewCleared()
  })

  it('removeMessage 清理被删消息的反馈/版本与过时统计', async () => {
    const stats = { total_tokens: 7 } as never
    vi.mocked(deleteMessage).mockResolvedValue(1)
    vi.mocked(listMessages).mockResolvedValue([{ id: 'm1', conversation_id: 'c1' } as never])
    vi.mocked(getConversationCostStats).mockResolvedValue(stats)
    vi.mocked(listSnapshots).mockResolvedValue([{ id: 's2' } as never])
    vi.mocked(listQueuedMessages).mockResolvedValue([])
    useProjectStore.setState({
      conversations: [conversation('c1')],
      currentConversation: conversation('c1'),
      messages: [{ id: 'm1' } as never, { id: 'm2' } as never],
      feedbackMap: { m1: { message_id: 'm1' } as never, m2: { message_id: 'm2' } as never },
      versionMap: { m1: [{ id: 'v1' } as never], m2: [{ id: 'v2' } as never] },
      lastTaskSummary: { status: 'completed' } as never,
      plan: { phase: 'done', steps: [] } as never,
      approvedPlan: { conversationId: 'c1', plan: 'x' } as never,
      unfinishedConv: { conversationId: 'c1' } as never,
      tokenStats: { total_tokens: 99 } as never,
      snapshots: [{ id: 's1' } as never],
      loadingSnapshots: true,
    })

    await useProjectStore.getState().removeMessage('m2')

    const state = useProjectStore.getState()
    expect(deleteMessage).toHaveBeenCalledWith('m2')
    expect(state.messages.map((m) => m.id)).toEqual(['m1'])
    expect(Object.keys(state.feedbackMap)).toEqual(['m1'])
    expect(Object.keys(state.versionMap)).toEqual(['m1'])
    expect(state.lastTaskSummary).toBeNull()
    expect(state.plan).toBeNull()
    expect(state.approvedPlan).toBeNull()
    expect(state.unfinishedConv).toBeNull()
    await vi.waitFor(() => {
      expect(useProjectStore.getState().tokenStats).toEqual(stats)
      expect(useProjectStore.getState().snapshots).toEqual([{ id: 's2' }])
    })
    expect(getConversationCostStats).toHaveBeenCalledWith('c1')
    expect(listSnapshots).toHaveBeenCalledWith('c1')
    expect(listQueuedMessages).toHaveBeenCalledWith('c1')
  })

  it('restoreToSnapshot 清空未来分支的计划/摘要/续跑断点并重拉反馈、排队与快照', async () => {
    const ledger = { title: '任务' } as never
    vi.mocked(restoreSnapshot).mockResolvedValue({
      label: 'A 阶段',
      archived: 2,
      restored: 0,
      went_back: true,
    })
    vi.mocked(listMessagesPage).mockResolvedValue({ messages: [{ id: 'm1' } as never], hasMore: true })
    vi.mocked(getTaskLedger).mockResolvedValue(ledger)
    vi.mocked(listMessageFeedback).mockResolvedValue([{ message_id: 'm1' } as never])
    vi.mocked(listMessageVersions).mockResolvedValue([{ id: 'v1', user_message_id: 'm1' } as never])
    vi.mocked(listSnapshots).mockResolvedValue([{ id: 's0' } as never])
    vi.mocked(listQueuedMessages).mockResolvedValue([])
    vi.mocked(getTodos).mockResolvedValue([
      { id: 'todo-a', content: '步骤 A', status: 'in_progress' } as never,
    ])
    // loadingSnapshots=true 时 loadSnapshots 会直接返回，这里模拟时间线已加载完
    useProjectStore.setState({ ...populatedConversationView(), loadingSnapshots: false })

    const result = await useProjectStore.getState().restoreToSnapshot('c1', 's1')

    expect(result).toEqual({ label: 'A 阶段', archived: 2, restored: 0, went_back: true })
    const state = useProjectStore.getState()
    expect(state.messages.map((m) => m.id)).toEqual(['m1'])
    expect(state.olderHasMore).toBe(true)
    expect(state.lastTaskSummary).toBeNull()
    expect(state.plan).toBeNull()
    expect(state.approvedPlan).toBeNull()
    expect(state.unfinishedConv).toBeNull()
    expect(state.tokenStats).toBeNull()
    // 被归档未来分支的执行轨迹不再展示
    expect(state.toolRuns).toEqual([])
    expect(state.agentRuns).toEqual([])
    await vi.waitFor(() => {
      const s = useProjectStore.getState()
      expect(s.feedbackMap).toEqual({ m1: { message_id: 'm1' } })
      expect(s.versionMap).toEqual({ m1: [{ id: 'v1', user_message_id: 'm1' }] })
      expect(s.queuedList).toEqual([])
      expect(s.snapshots).toEqual([{ id: 's0' }])
      expect(s.taskLedgers.c1).toEqual({ ledger, finished: true })
      // 清单以后端回退结果为准，而不是停留在未来分支
      expect(s.todos).toEqual([{ id: 'todo-a', content: '步骤 A', status: 'in_progress' }])
    })
    expect(restoreSnapshot).toHaveBeenCalledWith('c1', 's1')
    expect(listMessagesPage).toHaveBeenCalledWith('c1')
    expect(getTaskLedger).toHaveBeenCalledWith('c1')
    expect(getTodos).toHaveBeenCalledWith('c1')
    expect(listMessageFeedback).toHaveBeenCalledWith('c1')
    expect(listMessageVersions).toHaveBeenCalledWith('c1')
    expect(listQueuedMessages).toHaveBeenCalledWith('c1')
    expect(listSnapshots).toHaveBeenCalledWith('c1')
  })
})

describe('pending confirmations sync', () => {
  beforeEach(() => {
    useProjectStore.setState({
      currentProject: project('p1'),
      currentConversation: conversation('c1'),
      pendingConfirmations: {},
      toolApprovals: [],
      pendingPlan: null,
      askCard: null,
    })
  })

  it('服务端快照既关闭失效项、也回填前端缺失的审批', async () => {
    useProjectStore.setState({
      pendingConfirmations: {
        c1: [{ conversation_id: 'c1', kind: 'approval', request_id: 'stale' } as never],
      },
      toolApprovals: [{ requestId: 'stale', tool: 'run_command' } as never],
      pendingPlan: { requestId: 'stale-plan', conversationId: 'c1', plan: 'x' } as never,
    })
    const live = {
      conversation_id: 'c1',
      kind: 'approval',
      request_id: 'live',
      tool: 'run_command',
      args: '{"cmd":"ls"}',
      level: 'L2',
      desc: '执行命令',
    } as never
    vi.mocked(listPendingConfirmations).mockResolvedValue([live])

    await useProjectStore.getState().refreshPendingConfirmations()

    const state = useProjectStore.getState()
    // 失效项消失；服务端仍挂着的审批被回填。
    // 旧实现只删不补 ⇒ 侧栏角标亮着、卡片永远不出现，后端却在等超时。
    expect(state.toolApprovals.map((a) => a.requestId)).toEqual(['live'])
    expect(state.toolApprovals[0]).toMatchObject({ tool: 'run_command', level: 'L2' })
    expect(state.pendingPlan).toBeNull()
    expect(state.pendingConfirmations).toEqual({ c1: [live] })
  })

  it('WebView 重载后按服务端快照回填提问卡', async () => {
    const ask = {
      conversation_id: 'c1',
      kind: 'ask',
      request_id: 'ask-1',
      question: '要部署到哪台设备？',
      options: ['真机', '模拟器'],
    } as never
    vi.mocked(listPendingConfirmations).mockResolvedValue([ask])

    await useProjectStore.getState().refreshPendingConfirmations()

    expect(useProjectStore.getState().askCard).toMatchObject({
      requestId: 'ask-1',
      conversationId: 'c1',
      question: '要部署到哪台设备？',
      options: ['真机', '模拟器'],
    })
  })

  it('同步途中新到达的确认项不被旧快照关闭', async () => {
    let release!: () => void
    vi.mocked(listPendingConfirmations).mockImplementation(
      () =>
        new Promise((resolve) => {
          release = () => resolve([])
        }),
    )
    const syncing = useProjectStore.getState().refreshPendingConfirmations()
    // 查询未返回时，新审批通过事件进入视图与待确认表
    useProjectStore.setState({
      pendingConfirmations: {
        c1: [{ conversation_id: 'c1', kind: 'approval', request_id: 'new' } as never],
      },
      toolApprovals: [{ requestId: 'new', tool: 'run_command' } as never],
    })
    release()
    await syncing

    const state = useProjectStore.getState()
    expect(state.toolApprovals.map((a) => a.requestId)).toEqual(['new'])
    expect(state.pendingConfirmations.c1.map((p) => p.request_id)).toEqual(['new'])
  })
})

describe('heartbeat decoupled from output (P0-3 / P0-5)', () => {
  beforeEach(() => {
    useProjectStore.setState({
      currentProject: project('p1'),
      currentConversation: conversation('c1'),
      conversations: [conversation('c1')],
      messages: [],
      streamings: {},
      pendingConfirmations: {},
      toolApprovals: [],
    })
    vi.clearAllMocks()
  })

  it('心跳不刷新 lastDeltaAt，只刷新 lastHeartbeatAt', () => {
    const before = Date.now() - 5000
    useProjectStore.setState({
      streamings: {
        c1: {
          ...emptyStreaming(),
          conversationId: 'c1',
          runId: 'run-1',
          startedAt: before,
          lastDeltaAt: before,
          content: '已生成内容',
        },
      },
    })

    listeners.get('chat-heartbeat')!({
      payload: { conversation_id: 'c1', run_id: 'run-1', phase: 'streaming', started_at: before },
    })

    const bucket = useProjectStore.getState().streamings.c1
    // 心跳不应推进 lastDeltaAt（看门狗按真实产出判活）
    expect(bucket.lastDeltaAt).toBe(before)
    // 心跳必须刷新 lastHeartbeatAt（监管线程活性）
    expect(bucket.lastHeartbeatAt).toBeGreaterThanOrEqual(before)
    // 已生成内容保留
    expect(bucket.content).toBe('已生成内容')
  })

  it('僵尸桶（心跳恢复但 startedAt 为 null）心跳后补写 startedAt', () => {
    useProjectStore.setState({
      streamings: {
        c1: {
          ...emptyStreaming(),
          conversationId: null, // 错误态：conversationId 被清空
          runId: 'run-1',
          startedAt: null, // 僵尸：startedAt 为 null
          error: '后端长时间无响应',
        },
      },
    })

    listeners.get('chat-heartbeat')!({
      payload: { conversation_id: 'c1', run_id: 'run-1', phase: 'streaming', started_at: 12345 },
    })

    const bucket = useProjectStore.getState().streamings.c1
    // 心跳必须补写 startedAt，避免发送前置判定误拒
    expect(bucket.startedAt).toBe(12345)
    // 错误清除
    expect(bucket.error).toBeNull()
    expect(bucket.conversationId).toBe('c1')
  })

  it('错误态 setBucket 保留已生成正文，不瞬间清空', () => {
    useProjectStore.setState({
      streamings: {
        c1: {
          ...emptyStreaming(),
          conversationId: 'c1',
          runId: 'run-1',
          startedAt: Date.now(),
          content: '已生成正文',
          reasoning: '已生成思考',
        },
      },
    })

    // 模拟 watchdog 超时置错（不再 spread emptyStreaming）
    useProjectStore.setState((s) => {
      const prev = s.streamings.c1
      if (!prev) return {}
      return {
        streamings: {
          ...s.streamings,
          c1: {
            ...prev,
            error: '后端长时间无响应',
            conversationId: null,
            startedAt: null,
          },
        },
      }
    })

    const bucket = useProjectStore.getState().streamings.c1
    expect(bucket.error).toBe('后端长时间无响应')
    // 正文与思考必须保留
    expect(bucket.content).toBe('已生成正文')
    expect(bucket.reasoning).toBe('已生成思考')
  })

  it('sendUserMessage 在僵尸桶（startedAt 为 null）时允许重发', async () => {
    vi.mocked(streamChat).mockResolvedValue(undefined)
    useProjectStore.setState({
      conversations: [conversation('c1')],
      currentConversation: conversation('c1'),
      messages: [],
      streamings: {
        c1: {
          ...emptyStreaming(),
          conversationId: null, // 错误态
          runId: null,
          startedAt: null, // 僵尸：startedAt 为 null
          error: '后端长时间无响应',
        },
      },
    })

    const result = await useProjectStore.getState().sendUserMessage('测试消息')

    // 僵尸桶允许重发（返回 true）
    expect(result).toBe(true)
    expect(streamChat).toHaveBeenCalled()
  })

  it('sendUserMessage 在活跃桶（startedAt 非空且无错误）时拒绝重发', async () => {
    vi.mocked(streamChat).mockResolvedValue(undefined)
    useProjectStore.setState({
      conversations: [conversation('c1')],
      currentConversation: conversation('c1'),
      messages: [],
      streamings: {
        c1: {
          ...emptyStreaming(),
          conversationId: 'c1',
          runId: 'run-1',
          startedAt: Date.now(),
          lastDeltaAt: Date.now(),
          lastHeartbeatAt: Date.now(),
        },
      },
    })

    const result = await useProjectStore.getState().sendUserMessage('测试消息')

    // 活跃桶拒绝重发（返回 false）
    expect(result).toBe(false)
    expect(streamChat).not.toHaveBeenCalled()
  })
})
