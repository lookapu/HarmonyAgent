// ============================================================
// 会话笔记 store：每个会话可写一段个人笔记（纯前端 localStorage）
// ============================================================
//
// 设计目标：
// - 让用户能把"这个会话在做什么"写在会话顶部，避免长会话后忘记上下文
// - 纯前端方案：不动后端 schema（避开迁移成本）
// - 结构：Record<conversationId, { text, updatedAt }>
// - 上限 4000 字（防止 localStorage 撑爆）
import { create } from 'zustand'

const STORAGE_KEY = 'deveco-switch-conv-notes'
const MAX_LEN = 4000

const loadFromStorage = (): Record<string, NoteEntry> => {
  try {
    const raw = localStorage.getItem(STORAGE_KEY)
    if (!raw) return {}
    const obj = JSON.parse(raw)
    if (!obj || typeof obj !== 'object' || Array.isArray(obj)) return {}
    const out: Record<string, NoteEntry> = {}
    for (const [k, v] of Object.entries(obj)) {
      if (!v || typeof v !== 'object') continue
      const r = v as { text?: unknown; updatedAt?: unknown }
      if (typeof r.text === 'string') {
        out[k] = {
          text: r.text.slice(0, MAX_LEN),
          updatedAt: typeof r.updatedAt === 'number' ? r.updatedAt : Date.now(),
        }
      }
    }
    return out
  } catch {
    return {}
  }
}

const saveToStorage = (map: Record<string, NoteEntry>) => {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(map))
  } catch {
    // 满/禁用 → 静默
  }
}

export interface NoteEntry {
  text: string
  updatedAt: number
}

interface NoteStore {
  notes: Record<string, NoteEntry>
  get: (convId: string) => NoteEntry | null
  set: (convId: string, text: string) => void
  clear: (convId: string) => void
}

export const useNoteStore = create<NoteStore>((set, get) => ({
  notes: loadFromStorage(),
  get: (convId) => get().notes[convId] ?? null,
  set: (convId, text) => {
    const trimmed = text.slice(0, MAX_LEN)
    if (trimmed === '') {
      get().clear(convId)
      return
    }
    const map = { ...get().notes, [convId]: { text: trimmed, updatedAt: Date.now() } }
    set({ notes: map })
    saveToStorage(map)
  },
  clear: (convId) => {
    if (!get().notes[convId]) return
    const map = Object.fromEntries(Object.entries(get().notes).filter(([k]) => k !== convId))
    set({ notes: map })
    saveToStorage(map)
  },
}))

export const NOTE_MAX_LEN = MAX_LEN
