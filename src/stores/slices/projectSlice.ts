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
} from '../../api/project'

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

  openProject: async (id) => {
    const { projects } = get()
    const project = projects.find((p) => p.id === id)
    if (!project) return
    set({ currentProject: project, currentConversation: null, messages: [] })
    const conversations = await listConversations(id)
    set({ conversations })
    if (conversations.length > 0) {
      await get().openConversation(conversations[0].id)
    }
    await get().refreshGitBranches()
    await get().loadFileTree()
    await get().loadMemories()
    await get().loadToolStats()
  },

  loadFileTree: async () => {
    const project = get().currentProject
    if (!project || !project.path) {
      set({ fileTree: null, indexBuilding: false, dirCache: {} })
      return
    }
    set({ indexBuilding: true })
    try {
      // 懒加载：只读根目录一层，展开时逐级按需请求（无全量索引，不截断）
      const children = await listProjectDir(project.id, '')
      const name = project.path.replace(/[\\/]+$/, '').split(/[\\/]/).pop() || project.name
      set({
        fileTree: { name, path: '', type: 'dir', children },
        dirCache: { '': children },
        indexBuilding: false,
        currentProject: { ...project, index_state: 'ready' },
      })
      get().refreshProjects().catch(() => {})
    } catch {
      set({ fileTree: null, indexBuilding: false, dirCache: {} })
    }
  },

  loadDirChildren: async (path: string) => {
    const project = get().currentProject
    if (!project?.path) return []
    const cache = get().dirCache
    if (cache[path]) return cache[path]
    const children = await listProjectDir(project.id, path)
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
    }),
})
