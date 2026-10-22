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
  return { ...actual, listProjects: vi.fn() }
})

import { listProjects } from '../api/project'
import { emptyStreaming } from './slices/chatSlice'
import { useProjectStore } from './projectStore'

const project = (id: string) =>
  ({ id, name: id, path: `/${id}`, kind: 'harmony', trusted: true, index_state: 'ready', last_opened_at: 1, created_at: 1, conversation_count: 0, pinned: false }) as never
const conversation = (id: string) =>
  ({ id, project_id: 'p1', title: id, created_at: 1, updated_at: 1 }) as never

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
})
