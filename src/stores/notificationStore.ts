/**
 * 应用内通知中心：在右下角浮出"铃铛"按钮，点击展开通知列表。
 *
 * - info / success / warn / error 四种 tone
 * - 每条通知有 title + body + 时间 + 已读状态
 * - 本地持久化（localStorage，经 utils/storage 封装）：初始化时水合，每次变更写穿，
 *   历史跨启动留存（含已读/未读状态）。
 * - FIFO 上限 500：超限丢弃最旧，避免 localStorage 无限膨胀与长列表滚动性能劣化。
 * - 任意模块可调用 useNotificationStore.getState().push() 投递
 */
import { create } from 'zustand'
import { getJSON, setJSON } from '../utils/storage'
import { STORAGE_KEYS } from '../constants'

export type NotifyTone = 'info' | 'success' | 'warn' | 'error'

export interface AppNotification {
  id: string
  tone: NotifyTone
  title: string
  body?: string
  /** 毫秒时间戳 */
  createdAt: number
  /** 已读标记（未读 = 加粗 + 圆点） */
  read: boolean
  /** 关联资源链接（点击跳转），可选 */
  href?: string
}

interface NotificationStore {
  notifications: AppNotification[]
  /** 未读数量（派生） */
  unreadCount: () => number
  /** 投递新通知 */
  push: (n: Omit<AppNotification, 'id' | 'createdAt' | 'read'>) => string
  /** 标记单条已读 */
  markRead: (id: string) => void
  /** 全部标已读 */
  markAllRead: () => void
  /** 清空全部 */
  clear: () => void
}

const MAX = 500
let _id = 0
const newId = () => `n${Date.now().toString(36)}${(++_id).toString(36)}`

export const useNotificationStore = create<NotificationStore>((set, get) => ({
  // 初始化即从 localStorage 水合；缺失 / 解析失败回退空数组
  notifications: getJSON<AppNotification[]>(STORAGE_KEYS.NOTIFICATIONS, []),
  unreadCount: () => get().notifications.filter((n) => !n.read).length,
  push: (n) => {
    const id = newId()
    const item: AppNotification = { id, createdAt: Date.now(), read: false, ...n }
    const next = [item, ...get().notifications].slice(0, MAX)
    set({ notifications: next })
    setJSON(STORAGE_KEYS.NOTIFICATIONS, next)
    return id
  },
  markRead: (id) => {
    const next = get().notifications.map((n) => (n.id === id ? { ...n, read: true } : n))
    set({ notifications: next })
    setJSON(STORAGE_KEYS.NOTIFICATIONS, next)
  },
  markAllRead: () => {
    const next = get().notifications.map((n) => ({ ...n, read: true }))
    set({ notifications: next })
    setJSON(STORAGE_KEYS.NOTIFICATIONS, next)
  },
  clear: () => {
    set({ notifications: [] })
    setJSON(STORAGE_KEYS.NOTIFICATIONS, [])
  },
}))
