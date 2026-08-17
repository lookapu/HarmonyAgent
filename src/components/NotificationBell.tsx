import { useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useNotificationStore } from '../stores/notificationStore'

/* ============ 通知中心：铃铛按钮 + 浮层（应用内通知） ============
 * 抽为独立组件：管理后台侧边栏底部 + 工作区侧边栏底部 双处挂载。
 * 默认浮层 absolute left-0 bottom-full（相对铃铛向上弹出）；
 * fixed 模式用于工作区侧边栏（overflow-hidden 会裁剪超宽浮层），
 * 浮层改用 fixed 定位贴视口左下角。 */
export default function NotificationBell({ fixed = false }: { fixed?: boolean }) {
  const { t } = useTranslation()
  const [open, setOpen] = useState(false)
  const notifications = useNotificationStore((s) => s.notifications)
  const markRead = useNotificationStore((s) => s.markRead)
  const markAllRead = useNotificationStore((s) => s.markAllRead)
  const clear = useNotificationStore((s) => s.clear)
  const unread = notifications.filter((n) => !n.read).length
  const ref = useRef<HTMLDivElement>(null)

  // 点击外部关闭
  useEffect(() => {
    if (!open) return
    const close = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false)
    }
    window.addEventListener('mousedown', close)
    return () => window.removeEventListener('mousedown', close)
  }, [open])

  return (
    <div ref={ref} className="relative">
      <button
        onClick={() => setOpen((v) => !v)}
        className="p-1.5 rounded hover:bg-[var(--bg-card)] transition-colors relative"
        title={t('notification.title')}
      >
        <svg width="16" height="16" viewBox="0 -960 960 960" fill="currentColor">
          <path d="M160-200v-80h80v-280q0-83 50-147.5T420-792v-28q0-25 17.5-42.5T480-880q25 0 42.5 17.5T540-820v28q80 11 130 75.5T720-560v280h80v80H160Zm320-300Zm0 460q-33 0-56.5-23.5T400-120h160q0 33-23.5 56.5T480-40Z" />
        </svg>
        {unread > 0 && (
          <span
            className="absolute -top-0.5 -right-0.5 min-w-[16px] h-[16px] px-1 flex items-center justify-center rounded-full bg-[var(--danger)] text-white text-[9.5px] font-semibold leading-none shadow-md tnum"
            title={t('notification.unreadCount', { count: unread })}
          >
            {unread > 99 ? '99+' : unread}
          </span>
        )}
      </button>
      {open && (
        <div
          className={
            fixed
              ? 'fixed bottom-16 left-3 w-[340px] max-w-[calc(100vw-1rem)] max-h-[60vh] flex flex-col rounded-xl glass-card shadow-2xl animate-modal-in z-50'
              : 'absolute left-0 bottom-full mb-1.5 w-[340px] max-h-[480px] flex flex-col rounded-xl glass-card shadow-2xl animate-modal-in z-50'
          }
        >
          <div className="flex items-center justify-between px-3 py-2 border-b border-[var(--border)]">
            <span className="text-[12.5px] font-semibold">{t('notification.title')}</span>
            <div className="flex items-center gap-2">
              {unread > 0 && (
                <button
                  onClick={markAllRead}
                  className="text-[10.5px] text-[var(--accent)] hover:underline"
                >
                  {t('notification.markAllRead')}
                </button>
              )}
              {notifications.length > 0 && (
                <button
                  onClick={clear}
                  className="text-[10.5px] text-[var(--text-muted)] hover:text-[var(--danger)]"
                >
                  {t('notification.clear')}
                </button>
              )}
            </div>
          </div>
          <div className="flex-1 overflow-y-auto">
            {notifications.length === 0 ? (
              <div className="px-3 py-8 text-center text-[12px] text-[var(--text-muted)]">
                {t('notification.empty')}
              </div>
            ) : (
              notifications.map((n) => {
                const toneCls =
                  n.tone === 'error'
                    ? 'badge-tone-bad'
                    : n.tone === 'warn'
                      ? 'badge-tone-warn'
                      : n.tone === 'success'
                        ? 'badge-tone-ok'
                        : 'badge-tone-info'
                return (
                  <button
                    key={n.id}
                    onClick={() => {
                      markRead(n.id)
                      if (n.href) {
                        window.location.assign(n.href)
                        setOpen(false)
                      }
                    }}
                    className={`w-full text-left px-3 py-2 border-b border-[var(--border)] last:border-b-0 hover:bg-[var(--bg-hover)] transition-colors ${
                      n.read ? 'opacity-70' : ''
                    }`}
                  >
                    <div className="flex items-start gap-2">
                      {!n.read && <span className="w-1.5 h-1.5 rounded-full bg-[var(--accent)] mt-1.5 shrink-0" />}
                      <div className="flex-1 min-w-0">
                        <div className="flex items-center gap-1.5">
                          <span className={`badge-tone ${toneCls}`}>{n.tone}</span>
                          <span className="text-[12px] font-medium truncate">{n.title}</span>
                        </div>
                        {n.body && (
                          <p className="text-[11px] text-[var(--text-secondary)] mt-0.5 line-clamp-2 leading-relaxed">{n.body}</p>
                        )}
                        <p className="text-[10px] text-[var(--text-muted)] mt-1 tnum">
                          {new Date(n.createdAt).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}
                        </p>
                      </div>
                    </div>
                  </button>
                )
              })
            )}
          </div>
        </div>
      )}
    </div>
  )
}
