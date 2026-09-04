import { describe, it, expect, vi, beforeEach } from 'vitest'
import { sendNotification } from './desktop'
import { useNotificationStore } from '../stores/notificationStore'

// 总线的行为不依赖 Tauri 是否真的把原生通知弹出来；mock 掉 invoke，
// 只验证「sendNotification ⇒ 铃铛必达」这一半。
vi.mock('./invoke', () => ({
  invokeWithError: vi.fn().mockResolvedValue(undefined),
}))

describe('sendNotification（统一通知总线入口）', () => {
  beforeEach(() => {
    useNotificationStore.getState().clear()
  })

  it('每次桌面通知都落一条进铃铛，tone/title/body 原样、初始未读', async () => {
    await sendNotification('后台任务完成', '三条消息已生成', 'success')
    const list = useNotificationStore.getState().notifications
    expect(list).toHaveLength(1)
    expect(list[0]).toMatchObject({
      tone: 'success',
      title: '后台任务完成',
      body: '三条消息已生成',
      read: false,
    })
  })

  it('默认 tone 为 info', async () => {
    await sendNotification('标题', '正文')
    expect(useNotificationStore.getState().notifications[0].tone).toBe('info')
  })

  it('原生通知失败（无 IPC）也不影响进铃铛：历史优先于弹窗成败', async () => {
    const { invokeWithError } = await import('./invoke')
    ;(invokeWithError as ReturnType<typeof vi.fn>).mockRejectedValueOnce(new Error('no IPC'))
    await expect(sendNotification('拖拽失败', '文件过大', 'error')).rejects.toThrow('no IPC')
    // push 在 invoke 之前同步执行，即使 invoke 抛错铃铛也已记录
    expect(useNotificationStore.getState().notifications).toHaveLength(1)
    expect(useNotificationStore.getState().notifications[0].tone).toBe('error')
  })
})
