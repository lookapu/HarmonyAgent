import { memo, useEffect, useMemo, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import type { ChatMessage } from '../../api/project'
import Icon from '../../icons/Icon'

/** 会话段落导航：浮动按钮 → 弹出用户消息列表，点击滚动定位到该消息。
 *  长对话快速跳转，避免反复滚动查找。 */
export const ConversationNav = memo(function ConversationNav({
  messages,
}: {
  messages: ChatMessage[]
}) {
  const { t } = useTranslation()
  const [open, setOpen] = useState(false)
  const ref = useRef<HTMLDivElement>(null)

  const userMessages = useMemo(
    () => messages.filter((m) => m.role === 'user'),
    [messages],
  )

  useEffect(() => {
    if (!open) return
    const handler = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) {
        setOpen(false)
      }
    }
    document.addEventListener('mousedown', handler)
    return () => document.removeEventListener('mousedown', handler)
  }, [open])

  if (userMessages.length <= 2) return null

  const scrollToMsg = (msgId: string) => {
    const escapedId = typeof CSS !== 'undefined' && CSS.escape ? CSS.escape(msgId) : msgId.replace(/["\\]/g, '\\$&')
    const el = document.querySelector(`[data-msg-id="${escapedId}"]`)
    if (el) {
      el.scrollIntoView({ behavior: 'smooth', block: 'center' })
      setOpen(false)
    }
  }

  const truncate = (s: string, max: number) =>
    s.length > max ? s.slice(0, max) + '…' : s

  return (
    <div className="relative" ref={ref}>
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        aria-label={t('home.conversationNav')}
        aria-expanded={open}
        title={t('home.conversationNav')}
        className="flex items-center justify-center w-7 h-7 rounded-lg text-[var(--text-muted)] hover:text-[var(--text-secondary)] hover:bg-[var(--bg-hover)] transition-colors"
      >
        <Icon name="chat" size={14} />
      </button>
      {open && (
        <div className="absolute right-0 bottom-full mb-1.5 w-72 max-h-80 overflow-y-auto rounded-xl modern-card shadow-2xl shadow-black/40 py-1 z-50 animate-modal-in">
          <div className="px-3 py-1.5 text-[10px] font-medium text-[var(--text-muted)] border-b border-[var(--border)]">
            {t('home.conversationNavTitle', { count: userMessages.length })}
          </div>
          {userMessages.map((m, i) => {
            const preview = truncate(m.content.replace(/\n/g, ' '), 60)
            return (
              <button
                key={m.id}
                type="button"
                onClick={() => scrollToMsg(m.id)}
                className="w-full flex items-start gap-2 px-3 py-1.5 text-left hover:bg-[var(--bg-hover)] transition-colors"
              >
                <span className="text-[10px] text-[var(--text-muted)] tabular-nums shrink-0 mt-0.5 w-4 text-right">
                  {i + 1}
                </span>
                <span className="flex-1 min-w-0 text-[11.5px] text-[var(--text-secondary)] leading-relaxed">
                  {preview || t('home.emptyMessage')}
                </span>
              </button>
            )
          })}
        </div>
      )}
    </div>
  )
})
