import type { StateCreator } from 'zustand'
import type { MemorySlice, ProjectState } from '../projectStoreTypes'
import {
  listMemories as listMemoriesApi,
  saveMemory as saveMemoryApi,
  deleteMemory as deleteMemoryApi,
  setMemoryEnabled as setMemoryEnabledApi,
  listToolStats as listToolStatsApi,
  saveMessageFeedback as saveMessageFeedbackApi,
  listMessageFeedback as listMessageFeedbackApi,
  listMessageVersions as listMessageVersionsApi,
  summarizeMemory as summarizeMemoryApi,
  getConversationCostStats,
} from '../../api/project'
import type { MessageFeedback, MessageVersion } from '../../api/project'
import { getTaskRuns } from '../../api/cost'
import { escapeHtml, toHtml } from './chatUtils'

/** 记忆/统计/反馈/版本切片实现 */
export const createMemorySlice: StateCreator<ProjectState, [], [], MemorySlice> = (set, get) => ({
  memories: [],
  toolStats: [],
  feedbackMap: {},
  versionMap: {},
  memoryDraft: null,
  summarizing: false,
  tokenStats: null,
  recentRuns: [],
  lastTaskSummary: null,

  loadMemories: async () => {
    const project = get().currentProject
    if (!project) {
      set({ memories: [] })
      return
    }
    try {
      const list = await listMemoriesApi(project.id)
      set({ memories: list })
    } catch {
      set({ memories: [] })
    }
  },

  saveMemory: async (input) => {
    const project = get().currentProject
    if (!project) return
    await saveMemoryApi({ ...input, project_id: project.id })
    await get().loadMemories()
  },

  deleteMemory: async (id) => {
    await deleteMemoryApi(id)
    await get().loadMemories()
  },

  setMemoryEnabled: async (id, enabled) => {
    await setMemoryEnabledApi(id, enabled)
    await get().loadMemories()
  },

  loadToolStats: async () => {
    const project = get().currentProject
    if (!project) {
      set({ toolStats: [] })
      return
    }
    try {
      const list = await listToolStatsApi(project.id)
      set({ toolStats: list })
    } catch {
      set({ toolStats: [] })
    }
  },

  loadFeedback: async (conversationId) => {
    try {
      const list = await listMessageFeedbackApi(conversationId)
      const map: Record<string, MessageFeedback> = {}
      list.forEach((f) => {
        map[f.message_id] = f
      })
      set({ feedbackMap: map })
    } catch {
      // 加载失败保留旧数据
    }
  },

  rateMessage: async (messageId, feedback, reason) => {
    const conv = get().currentConversation
    if (!conv) return
    try {
      const saved = await saveMessageFeedbackApi({
        messageId,
        conversationId: conv.id,
        feedback,
        reason,
      })
      set((s) => {
        const next = { ...s.feedbackMap }
        if (saved) {
          next[messageId] = saved
        } else {
          delete next[messageId]
        }
        return { feedbackMap: next }
      })
    } catch {
      // 保存失败静默
    }
  },

  loadVersions: async (conversationId) => {
    try {
      const list = await listMessageVersionsApi(conversationId)
      const map: Record<string, MessageVersion[]> = {}
      list.forEach((v) => {
        ;(map[v.user_message_id] ??= []).push(v)
      })
      set({ versionMap: map })
    } catch {
      // 加载失败保留旧数据
    }
  },

  summarizeMemory: async (conversationId) => {
    set({ summarizing: true, memoryDraft: null })
    try {
      const draft = await summarizeMemoryApi(conversationId)
      set({ memoryDraft: draft })
      return draft
    } catch {
      set({ memoryDraft: null })
      return null
    } finally {
      set({ summarizing: false })
    }
  },

  exportConversation: (format) => {
    const { messages, currentConversation } = get()
    const lines: string[] = []
    const title = currentConversation?.title ?? '对话记录'
    if (format === 'html') {
      lines.push(
        '<!doctype html><html lang="zh"><head><meta charset="utf-8"><title>' +
          escapeHtml(title) +
          '</title><style>body{max-width:860px;margin:24px auto;padding:0 20px;font-family:system-ui,-apple-system,sans-serif;line-height:1.7}pre{background:#f6f8fa;padding:12px;border-radius:8px;overflow:auto}blockquote{border-left:3px solid #ddd;margin-left:0;padding-left:12px;color:#666}table{border-collapse:collapse}td,th{border:1px solid #ddd;padding:4px 10px}code{background:#f0f0f0;padding:1px 4px;border-radius:4px;font-size:.9em}</style></head><body>',
      )
    }
    for (const m of messages) {
      if (m.role === 'tool') continue
      const who = m.role === 'user' ? '用户' : 'AI'
      if (format === 'html') {
        lines.push(`<h3>${escapeHtml(who)}</h3>`)
        lines.push(`<div>${toHtml(m.content)}</div>`)
      } else {
        lines.push(`### ${who}`)
        lines.push('')
        lines.push(m.content)
        lines.push('')
      }
    }
    if (format === 'html') lines.push('</body></html>')
    return lines.join('\n')
  },

  loadRecentRuns: async () => {
    const project = get().currentProject
    if (!project) {
      set({ recentRuns: [] })
      return
    }
    try {
      const runs = await getTaskRuns(project.id, '', 10)
      set({ recentRuns: runs })
    } catch {
      // 统计表缺失/数据库忙时静默，概览不阻塞
    }
  },

  loadTokenStats: async (conversationId) => {
    try {
      const stats = await getConversationCostStats(conversationId)
      set({ tokenStats: stats })
    } catch {
      set({ tokenStats: null })
    }
  },
})
