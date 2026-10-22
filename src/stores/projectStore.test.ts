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
  }
})

import { deleteConversation, listConversations, listProjects } from '../api/project'
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
})
