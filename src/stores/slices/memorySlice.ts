import type { StateCreator } from 'zustand'
import type { MemorySlice, ProjectState } from '../projectStoreTypes'
import {
  listMemories as listMemoriesApi,
  saveMemory as saveMemoryApi,
  deleteMemory as deleteMemoryApi,
  setMemoryEnabled as setMemoryEnabledApi,
  listToolStats as listToolStatsApi,
  listToolTokenStats as listToolTokenStatsApi,
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
  toolTokenStats: [],
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
    // 乐观移除：先让列表立即消失，再异步落库，避免「delete + 全量重载」两次 IPC 的等待
    set((s) => ({ memories: s.memories.filter((m) => m.id !== id) }))
    try {
      await deleteMemoryApi(id)
    } catch {
      // 删除失败回滚重载，恢复真实状态
      await get().loadMemories()
    }
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

  loadToolTokenStats: async () => {
    try {
      const list = await listToolTokenStatsApi(30)
      set({ toolTokenStats: list })
    } catch {
      set({ toolTokenStats: [] })
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
    const title = currentConversation?.title ?? '对话记录'
    const now = new Date()
    const ts = (ms: number) => {
      const d = new Date(ms)
      const pad = (n: number) => String(n).padStart(2, '0')
      return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`
    }
    // 导出可见消息（排除 tool 角色；reasoning 单独处理）
    const visible = messages.filter((m) => m.role !== 'tool')
    const lines: string[] = []
    if (format === 'html') {
      // 独立 HTML 文档：含样式，离线打开即可阅读；body 内 toHtml 走 unified pipeline
      lines.push(
        `<!doctype html><html lang="zh"><head><meta charset="utf-8"><title>${escapeHtml(title)}</title>` +
          `<style>` +
          `body{max-width:860px;margin:24px auto;padding:0 20px;font-family:system-ui,-apple-system,Segoe UI,Roboto,sans-serif;line-height:1.7;color:#1f2328;background:#fff}` +
          `h1.title{margin:0 0 4px;font-size:1.6em;border-bottom:1px solid #e5e7eb;padding-bottom:8px}` +
          `.meta{color:#6b7280;font-size:.88em;margin:0 0 24px}` +
          `.msg{margin:18px 0;padding:14px 16px;border-radius:8px;background:#f9fafb;border:1px solid #e5e7eb}` +
          `.msg.user{background:#eff6ff;border-color:#bfdbfe}` +
          `.msg.assistant{background:#f9fafb}` +
          `.who{display:flex;align-items:baseline;gap:8px;margin:0 0 8px;font-weight:600}` +
          `.who .tag{font-size:.95em}` +
          `.who .when{color:#9ca3af;font-weight:400;font-size:.82em}` +
          `.who .model{color:#9ca3af;font-weight:400;font-size:.82em}` +
          `pre{background:#0d1117;color:#e6edf3;padding:12px 14px;border-radius:6px;overflow:auto;line-height:1.5}` +
          `pre code{background:transparent;padding:0;color:inherit}` +
          `code{background:#f3f4f6;padding:1px 5px;border-radius:4px;font-size:.9em;font-family:ui-monospace,SFMono-Regular,Menlo,monospace}` +
          `blockquote{border-left:3px solid #d1d5db;margin:8px 0;padding:2px 12px;color:#4b5563;background:#f9fafb}` +
          `table{border-collapse:collapse;margin:8px 0}` +
          `td,th{border:1px solid #d1d5db;padding:6px 12px}` +
          `th{background:#f3f4f6}` +
          `ul,ol{padding-left:1.5em}` +
          `li{margin:2px 0}` +
          `.reasoning{margin-top:10px;padding:8px 12px;background:#fef3c7;border-left:3px solid #f59e0b;border-radius:4px;color:#78350f;font-size:.92em;white-space:pre-wrap}` +
          `.reasoning-label{font-weight:600;margin-bottom:4px}` +
          `</style></head><body>`,
      )
      lines.push(`<h1 class="title">${escapeHtml(title)}</h1>`)
      lines.push(
        `<p class="meta">导出时间 ${ts(now.getTime())} · 共 ${visible.length} 条消息</p>`,
      )
      for (const m of visible) {
        const who = m.role === 'user' ? '用户' : 'AI'
        const meta: string[] = [ts(m.created_at)]
        if (m.model) meta.push(m.model)
        if (m.duration_ms != null) meta.push(`耗时 ${(m.duration_ms / 1000).toFixed(1)}s`)
        const tokens =
          m.tokens_in != null || m.tokens_out != null
            ? `tokens ${m.tokens_in ?? 0}/${m.tokens_out ?? 0}`
            : ''
        if (tokens) meta.push(tokens)
        lines.push(
          `<section class="msg ${m.role}">` +
            `<div class="who"><span class="tag">${escapeHtml(who)}</span>` +
            `<span class="when">${escapeHtml(meta.join(' · '))}</span></div>` +
            `<div>${toHtml(m.content)}</div>`,
        )
        if (m.reasoning) {
          lines.push(
            `<div class="reasoning"><div class="reasoning-label">思考过程</div>${escapeHtml(m.reasoning)}</div>`,
          )
        }
        lines.push(`</section>`)
      }
      lines.push('</body></html>')
    } else if (format === 'md') {
      // Markdown：YAML frontmatter（标题/导出时间/消息数），## 二级角色段头
      lines.push('---')
      lines.push(`title: "${title.replace(/"/g, '\\"')}"`)
      lines.push(`date: ${ts(now.getTime())}`)
      lines.push(`messages: ${visible.length}`)
      lines.push('---')
      lines.push('')
      lines.push(`# ${title}`)
      lines.push('')
      for (const m of visible) {
        const who = m.role === 'user' ? '用户' : 'AI'
        const meta: string[] = [ts(m.created_at)]
        if (m.model) meta.push(m.model)
        if (m.duration_ms != null) meta.push(`耗时 ${(m.duration_ms / 1000).toFixed(1)}s`)
        const tokens =
          m.tokens_in != null || m.tokens_out != null
            ? `tokens ${m.tokens_in ?? 0}/${m.tokens_out ?? 0}`
            : ''
        if (tokens) meta.push(tokens)
        lines.push(`## ${who}`)
        lines.push('')
        lines.push(`<sub>${meta.join(' · ')}</sub>`)
        lines.push('')
        lines.push(m.content)
        if (m.reasoning) {
          lines.push('')
          lines.push('> **思考过程**')
          lines.push('>')
          for (const rl of m.reasoning.split('\n')) {
            lines.push(`> ${rl}`)
          }
        }
        lines.push('')
      }
    } else {
      // txt：纯文本，【角色】头 + 元信息行 + 缩进正文
      lines.push(`标题：${title}`)
      lines.push(`导出时间：${ts(now.getTime())}`)
      lines.push(`消息数：${visible.length}`)
      lines.push('=' .repeat(60))
      lines.push('')
      for (const m of visible) {
        const who = m.role === 'user' ? '用户' : 'AI'
        const meta: string[] = [ts(m.created_at)]
        if (m.model) meta.push(m.model)
        if (m.duration_ms != null) meta.push(`耗时 ${(m.duration_ms / 1000).toFixed(1)}s`)
        const tokens =
          m.tokens_in != null || m.tokens_out != null
            ? `tokens ${m.tokens_in ?? 0}/${m.tokens_out ?? 0}`
            : ''
        if (tokens) meta.push(tokens)
        lines.push(`【${who}】${meta.join('  ')}`)
        lines.push('-'.repeat(60))
        // 缩进正文（每行 2 空格），方便阅读
        for (const ln of m.content.split('\n')) {
          lines.push(ln ? `  ${ln}` : '')
        }
        if (m.reasoning) {
          lines.push('')
          lines.push('  [思考过程]')
          for (const rl of m.reasoning.split('\n')) {
            lines.push(rl ? `    ${rl}` : '    ')
          }
        }
        lines.push('')
      }
    }
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
