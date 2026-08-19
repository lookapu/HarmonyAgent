// ============================================================
// 消息 Pin store：把任意消息钉到会话顶部（仅前端，localStorage 持久化）
// ============================================================
//
// 设计目标：
// - 会话长了之后找回"关键决策点/报错信息/成功方法"很痛 → 允许用户手动 pin 任意消息
// - 纯前端方案：不动后端 schema（避开迁移成本 + 跨设备同步由用户自己决定）
// - 结构：Record<conversationId, messageId[]>（按 pin 时间顺序：先 pin 的在前）
// - 限制：每个会话最多 8 条 pin（多了反而成噪音）
// - 不持久化"已删除消息"的 id：渲染时如果消息不存在则静默跳过
import { create } from 'zustand'
import { getJSON, setJSON } from '../utils/storage'
import { STORAGE_KEYS } from '../constants'

const MAX_PER_CONV = 8

const loadFromStorage = (): Record<string, string[]> => {
  const raw = getJSON<unknown>(STORAGE_KEYS.PINNED, null)
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) return {}
  const obj = raw as Record<string, unknown>
  const out: Record<string, string[]> = {}
  for (const [k, v] of Object.entries(obj)) {
    if (Array.isArray(v) && v.every((x) => typeof x === 'string')) {
      out[k] = v.slice(0, MAX_PER_CONV)
    }
  }
  return out
}

const saveToStorage = (map: Record<string, string[]>) => {
  setJSON(STORAGE_KEYS.PINNED, map)
}

interface PinStore {
  /** conversationId -> 已 pin 的 messageId 列表（按 pin 时间顺序） */
  pins: Record<string, string[]>
  /** 是否已 pin */
  isPinned: (convId: string, msgId: string) => boolean
  /** pin 一条；已存在则不重复；超过上限则丢弃最早的 */
  pin: (convId: string, msgId: string) => void
  /** 取消 pin */
  unpin: (convId: string, msgId: string) => void
  /** 切换 pin 状态，返回新状态 */
  toggle: (convId: string, msgId: string) => boolean
  /** 清空某会话所有 pin */
  clear: (convId: string) => void
}

export const usePinStore = create<PinStore>((set, get) => ({
  pins: loadFromStorage(),
  isPinned: (convId, msgId) => (get().pins[convId] ?? []).includes(msgId),
  pin: (convId, msgId) => {
    const cur = get().pins[convId] ?? []
    if (cur.includes(msgId)) return
    // 超过上限 → 丢弃最早一条（FIFO）
    const next = cur.length >= MAX_PER_CONV ? [...cur.slice(1), msgId] : [...cur, msgId]
    const map = { ...get().pins, [convId]: next }
    set({ pins: map })
    saveToStorage(map)
  },
  unpin: (convId, msgId) => {
    const cur = get().pins[convId]
    if (!cur) return
    const next = cur.filter((id) => id !== msgId)
    const map = next.length === 0
      ? Object.fromEntries(Object.entries(get().pins).filter(([k]) => k !== convId))
      : { ...get().pins, [convId]: next }
    set({ pins: map })
    saveToStorage(map)
  },
  toggle: (convId, msgId) => {
    if (get().isPinned(convId, msgId)) {
      get().unpin(convId, msgId)
      return false
    }
    get().pin(convId, msgId)
    return true
  },
  clear: (convId) => {
    const map = Object.fromEntries(Object.entries(get().pins).filter(([k]) => k !== convId))
    set({ pins: map })
    saveToStorage(map)
  },
}))

export const PIN_MAX_PER_CONV = MAX_PER_CONV
