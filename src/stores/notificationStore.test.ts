import { describe, it, expect, beforeEach, vi } from 'vitest'
import { STORAGE_KEYS } from '../constants'

// 测 hydration 需要 store 在「已写入 localStorage」之后重建，故每个用例重置模块，
// 让 store 以干净状态重读 localStorage。
beforeEach(() => {
  localStorage.clear()
  vi.resetModules()
})

async function freshStore() {
  const m = await import('./notificationStore')
  return m.useNotificationStore
}

describe('notificationStore 本地持久化', () => {
  it('push 写穿到 localStorage，重新加载后水合回来', async () => {
    const store = await freshStore()
    store.getState().push({ tone: 'info', title: 'A', body: 'B' })

    const raw = localStorage.getItem(STORAGE_KEYS.NOTIFICATIONS)
    expect(raw).not.toBeNull()
    const stored = JSON.parse(raw!) as { tone: string; title: string; body?: string; read: boolean }[]
    expect(stored).toHaveLength(1)
    expect(stored[0]).toMatchObject({ tone: 'info', title: 'A', body: 'B', read: false })

    // 模拟重启：resetModules 后 store 从 localStorage 水合，且写穿已包含 id/createdAt
    vi.resetModules()
    const store2 = await freshStore()
    expect(store2.getState().notifications).toHaveLength(1)
    expect(store2.getState().notifications[0].title).toBe('A')
    expect(store2.getState().notifications[0].id).toBeTruthy()
  })

  it('markAllRead / clear 都写穿，已读状态随持久化留存', async () => {
    const store = await freshStore()
    store.getState().push({ tone: 'warn', title: 'X' })
    store.getState().markAllRead()
    let stored = JSON.parse(localStorage.getItem(STORAGE_KEYS.NOTIFICATIONS)!) as { read: boolean }[]
    expect(stored[0].read).toBe(true)

    store.getState().clear()
    stored = JSON.parse(localStorage.getItem(STORAGE_KEYS.NOTIFICATIONS)!)
    expect(stored).toEqual([])
  })

  it('FIFO 上限 500：超限丢弃最旧', async () => {
    const store = await freshStore()
    for (let i = 0; i < 510; i++) store.getState().push({ tone: 'info', title: `t${i}` })

    const list = store.getState().notifications
    expect(list).toHaveLength(500)
    expect(list[0].title).toBe('t509')
    expect(list[499].title).toBe('t10')
  })
})
