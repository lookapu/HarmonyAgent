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
import { getJSON, setJSON } from '../utils/storage'
import { STORAGE_KEYS } from '../constants'

const MAX_LEN = 4000

const loadFromStorage = (): Record<string, NoteEntry> => {
  const raw = getJSON<unknown>(STORAGE_KEYS.CONV_NOTES, null)
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) return {}
  const obj = raw as Record<string, { text?: unknown; updatedAt?: unknown }>
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
}

const saveToStorage = (map: Record<string, NoteEntry>) => {
  setJSON(STORAGE_KEYS.CONV_NOTES, map)
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
