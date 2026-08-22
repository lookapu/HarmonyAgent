import type { StateCreator } from 'zustand'
import type { ProjectSlice, ProjectState } from '../projectStoreTypes'
import {
  listProjects,
  addProject,
  trustProject,
  deleteProject,
  getGitBranches,
  switchGitBranch,
  listProjectDir,
  listConversations,
  buildProjectIndex,
  conversationRoot,
  setProjectPinned,
} from '../../api/project'
import { startPerfTrace, waitForNextPaint } from '../../utils/perfTrace'
import { getItem, setItem } from '../../utils/storage'
import { STORAGE_KEYS } from '../../constants'
import { emptyStreaming } from './chatSlice'

/** localStorage key：持久化最近选中的项目/会话（统一见 src/constants.ts 的 STORAGE_KEYS） */

/** 记住项目/会话选择（下次打开时恢复） */
function persistLastProject(projectId: string, convId?: string) {
  setItem(STORAGE_KEYS.LAST_PROJECT, projectId)
  if (convId) setItem(STORAGE_KEYS.LAST_CONV_PREFIX + projectId, convId)
}

/** 获取上次项目 ID；不存在返回 null */
export function getLastProjectId(): string | null {
  return getItem(STORAGE_KEYS.LAST_PROJECT)
}

/** 获取项目上次打开的会话 ID */
export function getLastConversationId(projectId: string): string | null {
  return getItem(STORAGE_KEYS.LAST_CONV_PREFIX + projectId)
}

/** 项目/文件树/分支切片实现 */
export const createProjectSlice: StateCreator<ProjectState, [], [], ProjectSlice> = (set, get) => ({
  projects: [],
  currentProject: null,
  gitBranches: null,
  fileTree: null,
  indexBuilding: false,
  dirCache: {},
  loading: false,

  refreshProjects: async () => {
    const projects = await listProjects()
    set({ projects })
    // 保持当前项目引用最新（trusted 等字段刷新）
    const cur = get().currentProject
    if (cur) {
      const fresh = projects.find((p) => p.id === cur.id)
      if (fresh) set({ currentProject: fresh })
    }
  },

  toggleProjectPin: async (id: string) => {
    const { projects } = get()
    const p = projects.find((x) => x.id === id)
    if (!p) return
    try {
      const updated = await setProjectPinned(id, !p.pinned)
      // 置顶优先重排（与后端 list_projects 排序一致：pinned → global → 最近打开）
      const reordered = projects
        .map((x) => (x.id === id ? updated : x))
        .sort((a, b) => Number(b.pinned) - Number(a.pinned))
      set({ projects: reordered })
      const cur = get().currentProject
      if (cur && cur.id === id) set({ currentProject: updated })
    } catch (e) {
      console.error('置顶失败', e)
    }
  },

  openProject: async (id) => {
    const trace = startPerfTrace('openProject', { projectId: id.slice(0, 8) })
    const { projects } = get()
    const project = projects.find((p) => p.id === id)
    if (!project) {
      trace.end()
      return
    }

    // 只清空项目相关的侧边栏/运行态数据；
    // messages / conversations / currentConversation 保持旧值，
    // 等新会话消息加载完成后一次性替换（ChatGPT 风格无缝切换，避免清空闪烁）。
    // openConversation 内部也是同样策略：不清空 messages 直到新消息落地。
    set({
      currentProject: project,
      gitBranches: null,
      fileTree: null,
      dirCache: {},
      memories: [],
      toolStats: [],
      toolRuns: [],
      agentRuns: [],
      plan: null,
      pendingPlan: null,
      toolApprovals: [],
      pendingConfirmations: {},
      approvedPlan: null,
      unfinishedConv: null,
      todos: [],
      askCard: null,
      diagnoseCards: [],
      buildLogs: [],
      terminalEntries: [],
      lastTaskSummary: null,
      // 待发送队列与会话搜索词都是会话列表/对话区视角的状态，跨项目切换即丢弃，
      // 避免旧项目的排队消息/搜索过滤串到新项目
      queuedList: [],
      conversationKeyword: '',
      // feedbackMap/versionMap/tokenStats 延迟到新消息 set 时一并清空，合并为一次渲染
    })
    trace.mark('project-set')

    // 持久化当前项目选择
    persistLastProject(id)

    // 1. 加载会话列表
    const conversations = await listConversations(id)
    trace.mark('convs-loaded')
    set({ conversations })
    trace.markAfterPaint('convs-painted')

    // 1.5 加载各会话待确认项（审批/计划/提问）→ 会话列表角标 + 切回会话恢复
    void get().refreshPendingConfirmations().catch(() => {})

    // 2. 打开最近会话（优先恢复上次选中的，否则第一个）
    //    openConversation 内部会等待 React commit + paint 后才 resolve，
    //    所以这里 await 返回时，用户已经能看到新会话的消息列表
    const lastConvId = getLastConversationId(id)
    const targetConv =
      (lastConvId && conversations.find((c) => c.id === lastConvId)) || conversations[0]
    if (targetConv) {
      await get().openConversation(targetConv.id)
      persistLastProject(id, targetConv.id)
      trace.mark('conv-painted')
    } else {
      // 新项目没有任何会话：立即清空对话区（会话与消息都不能沿用上一个项目），
      // 否则切换后残留上一项目的会话标题、消息与流式状态，造成“串项目”错觉
      set({
        currentConversation: null,
        messages: [],
        olderHasMore: false,
        loadingOlder: false,
        streaming: emptyStreaming(),
        feedbackMap: {},
        versionMap: {},
        tokenStats: null,
        toolRuns: [],
        agentRuns: [],
        todos: [],
        askCard: null,
        toolApprovals: [],
        pendingPlan: null,
        plan: null,
        lastTaskSummary: null,
        approvedPlan: null,
        unfinishedConv: null,
      })
      trace.mark('no-conv-cleared')
    }

    // 3. 侧边栏数据（Git/文件树/记忆/工具统计）全部并行加载，不阻塞聊天视图
    //    这些数据只影响左右侧边栏，聊天内容已经可以交互
    const sidebarTasks = Promise.all([
      get().refreshGitBranches().catch(() => {}),
      get().loadFileTree().catch(() => {}),
      get().loadMemories().catch(() => {}),
      get().loadToolStats().catch(() => {}),
    ])
    sidebarTasks.then(() => trace.mark('sidebar-loaded')).catch(() => {})

    // 4. 等待一帧确保所有状态更新都已渲染，然后结束追踪
    await waitForNextPaint()
    trace.end()
  },

  loadFileTree: async () => {
    const project = get().currentProject
    if (!project || !project.path) {
      set({ fileTree: null, indexBuilding: false, dirCache: {} })
      return
    }
    set({ indexBuilding: true })
    const t = startPerfTrace('loadFileTree', { pid: project.id.slice(0, 8) })
    // 会话工作目录：worktree 会话用 worktree_path，否则回退项目主路径
    const root = conversationRoot(get().currentConversation)
    const basePath = root ?? project.path
    try {
      // 懒加载：只读根目录一层，展开时逐级按需请求（无全量索引，不截断）
      const children = await listProjectDir(project.id, '', root)
      t.mark('root-listed')
      const name = basePath.replace(/[\\/]+$/, '').split(/[\\/]/).pop() || project.name
      set({
        fileTree: { name, path: '', type: 'dir', children },
        dirCache: { '': children },
        indexBuilding: false,
        currentProject: { ...project, index_state: 'ready' },
      })
      t.mark('state-set')
      // 后台构建全量索引：真正更新 DB 的 index_state（pending→ready）并写入文件树缓存，
      // 完成后刷新项目状态，让右侧栏"工程概览"的索引状态显示真实结果；失败不影响懒加载文件树
      buildProjectIndex(project.id, root)
        .then(() => get().refreshProjects().catch(() => {}))
        .catch(() => {})
      t.end()
    } catch {
      set({ fileTree: null, indexBuilding: false, dirCache: {} })
      t.mark('error')
      t.end()
    }
  },

  loadDirChildren: async (path: string) => {
    const project = get().currentProject
    if (!project?.path) return []
    const cache = get().dirCache
    if (cache[path]) return cache[path]
    const root = conversationRoot(get().currentConversation)
    const children = await listProjectDir(project.id, path, root)
    set({ dirCache: { ...cache, [path]: children } })
    return children
  },

  rebuildIndex: async () => {
    // 刷新：清空懒加载缓存后重新读根目录（目录内容变化后重新拉取）
    set({ dirCache: {}, fileTree: null })
    await get().loadFileTree()
  },

  refreshGitBranches: async () => {
    const project = get().currentProject
    if (!project || !project.path) {
      set({ gitBranches: null })
      return
    }
    try {
      const info = await getGitBranches(project.id)
      set({ gitBranches: info })
    } catch {
      set({ gitBranches: null })
    }
  },

  switchBranch: async (branch) => {
    const project = get().currentProject
    if (!project) return null
    try {
      const info = await switchGitBranch(project.id, branch)
      set({ gitBranches: info })
      return info.error ?? null
    } catch (e) {
      return String(e)
    }
  },

  addProjectByPath: async (path) => {
    const project = await addProject(path)
    await get().refreshProjects()
    return project
  },

  confirmTrust: async (id) => {
    const project = await trustProject(id)
    set({ currentProject: project })
    await get().refreshProjects()
  },

  removeProject: async (id) => {
    await deleteProject(id)
    const { currentProject } = get()
    if (currentProject?.id === id) {
      set({ currentProject: null, conversations: [], currentConversation: null, messages: [], plan: null })
    }
    await get().refreshProjects()
  },

  reset: () =>
    set({
      projects: [],
      currentProject: null,
      conversations: [],
      currentConversation: null,
      messages: [],
      memories: [],
      toolStats: [],
      plan: null,
      streamings: {},
    }),
})
