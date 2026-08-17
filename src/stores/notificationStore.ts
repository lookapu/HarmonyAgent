/**
 * 应用内通知中心：在右下角浮出"铃铛"按钮，点击展开通知列表。
 *
 * - info / success / warn / error 四种 tone
 * - 每条通知有 title + body + 时间 + 已读状态
 * - 不持久化（刷新清空）；超过 50 条自动 FIFO 清理
 * - 任意模块可调用 useNotificationStore.getState().push() 投递
 */
import { create } from 'zustand'

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

const MAX = 50
let _id = 0
const newId = () => `n${Date.now().toString(36)}${(++_id).toString(36)}`

export const useNotificationStore = create<NotificationStore>((set, get) => ({
  notifications: [],
  unreadCount: () => get().notifications.filter((n) => !n.read).length,
  push: (n) => {
    const id = newId()
    const item: AppNotification = { id, createdAt: Date.now(), read: false, ...n }
    set((s) => ({ notifications: [item, ...s.notifications].slice(0, MAX) }))
    return id
  },
  markRead: (id) => set((s) => ({
    notifications: s.notifications.map((n) => (n.id === id ? { ...n, read: true } : n)),
  })),
  markAllRead: () => set((s) => ({ notifications: s.notifications.map((n) => ({ ...n, read: true })) })),
  clear: () => set({ notifications: [] }),
}))
