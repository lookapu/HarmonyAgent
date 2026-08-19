// ============================================================
// Agent 消息评分 store：1-5 星 + 文字理由（纯前端，localStorage 持久化）
// ============================================================
//
// 与 feedbackMap（后端 like/dislike）的差异：
// - 反馈是粗粒度二态：点踩/点赞
// - 评分是细粒度 5 档：用户能给具体分数，让"本月 agent 表现"可量化追踪
// - 互不干扰：用户可以既点 like 又给 4 星
//
// 设计原则：
// - 纯前端，不写后端 DB（避免 schema 改动 → 记忆红线）
// - localStorage 持久化，按 messageId 索引
// - 支持统计：所有评分的平均分、按 model 分组（可选增强）
import { create } from 'zustand'
import { getJSON, setJSON } from '../utils/storage'
import { STORAGE_KEYS } from '../constants'

const MAX_COMMENT = 280

const loadFromStorage = (): Record<string, RatingEntry> => {
  const raw = getJSON<unknown>(STORAGE_KEYS.RATINGS, null)
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) return {}
  const obj = raw as Record<string, { score?: unknown; comment?: unknown; ts?: unknown }>
  const out: Record<string, RatingEntry> = {}
  for (const [k, v] of Object.entries(obj)) {
    if (!v || typeof v !== 'object') continue
    const r = v as { score?: unknown; comment?: unknown; ts?: unknown }
    if (typeof r.score === 'number' && r.score >= 1 && r.score <= 5) {
      out[k] = {
        score: r.score,
        comment: typeof r.comment === 'string' ? r.comment.slice(0, MAX_COMMENT) : null,
        ts: typeof r.ts === 'number' ? r.ts : Date.now(),
      }
    }
  }
  return out
}

const saveToStorage = (map: Record<string, RatingEntry>) => {
  setJSON(STORAGE_KEYS.RATINGS, map)
}

export interface RatingEntry {
  /** 1-5 星 */
  score: number
  /** 可选文字理由 */
  comment: string | null
  /** 评分时间（unix ms） */
  ts: number
}

interface RatingStore {
  ratings: Record<string, RatingEntry>
  get: (msgId: string) => RatingEntry | null
  set: (msgId: string, entry: Omit<RatingEntry, 'ts'>) => void
  remove: (msgId: string) => void
  /** 全部评分的平均分（排除无评分消息），null = 无评分 */
  average: (msgIds?: string[]) => { avg: number; count: number } | null
}

export const useRatingStore = create<RatingStore>((set, get) => ({
  ratings: loadFromStorage(),
  get: (msgId) => get().ratings[msgId] ?? null,
  set: (msgId, entry) => {
    const full: RatingEntry = { ...entry, ts: Date.now() }
    const map = { ...get().ratings, [msgId]: full }
    set({ ratings: map })
    saveToStorage(map)
  },
  remove: (msgId) => {
    if (!get().ratings[msgId]) return
    const map = Object.fromEntries(Object.entries(get().ratings).filter(([k]) => k !== msgId))
    set({ ratings: map })
    saveToStorage(map)
  },
  average: (msgIds) => {
    const all = get().ratings
    const ids = msgIds ?? Object.keys(all)
    if (ids.length === 0) return null
    let sum = 0, count = 0
    for (const id of ids) {
      const r = all[id]
      if (r) { sum += r.score; count++ }
    }
    if (count === 0) return null
    return { avg: sum / count, count }
  },
}))

export const RATING_MAX_COMMENT = MAX_COMMENT
