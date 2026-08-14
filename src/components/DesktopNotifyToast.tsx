import { useEffect, useState } from 'react'
import { listen } from '@tauri-apps/api/event'

interface ToastItem {
  id: number
  title: string
  body: string
  kind: string
}

const accentFor = (kind: string) => {
  if (kind === 'success') return 'var(--success, #16a34a)'
  if (kind === 'error') return 'var(--danger, #dc2626)'
  return 'var(--accent, #3b82f6)'
}

const iconFor = (kind: string) => {
  if (kind === 'success') {
    return (
      <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.4" strokeLinecap="round" strokeLinejoin="round">
        <polyline points="20 6 9 17 4 12" />
      </svg>
    )
  }
  if (kind === 'error') {
    return (
      <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.4" strokeLinecap="round" strokeLinejoin="round">
        <circle cx="12" cy="12" r="10" />
        <line x1="15" y1="9" x2="9" y2="15" />
        <line x1="9" y1="9" x2="15" y2="15" />
      </svg>
    )
  }
  return (
    <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.4" strokeLinecap="round" strokeLinejoin="round">
      <circle cx="12" cy="12" r="10" />
      <line x1="12" y1="16" x2="12" y2="12" />
      <line x1="12" y1="8" x2="12.01" y2="8" />
    </svg>
  )
}

export default function DesktopNotifyToast() {
  const [toasts, setToasts] = useState<ToastItem[]>([])

  useEffect(() => {
    let seq = 0
    const unlisten = listen<{ title: string; body: string; kind: string }>('desktop-notify', (e) => {
      const id = ++seq
      setToasts((prev) => [...prev, { id, ...e.payload }])
      window.setTimeout(() => {
        setToasts((prev) => prev.filter((t) => t.id !== id))
      }, 5200)
    })
    return () => {
      unlisten.then((u) => u()).catch(() => {})
    }
  }, [])

  if (toasts.length === 0) return null

  return (
    <div className="fixed top-4 right-4 z-[100] flex flex-col gap-2 w-80 max-w-[calc(100vw-2rem)]">
      {toasts.map((t) => (
        <div
          key={t.id}
          className="animate-fade-in-up rounded-xl border border-[var(--border)] bg-[var(--bg-elevated)]/95 backdrop-blur shadow-lg shadow-black/10 p-3 flex gap-3"
          style={{ borderLeft: `3px solid ${accentFor(t.kind)}` }}
        >
          <div
            className="shrink-0 w-8 h-8 rounded-lg flex items-center justify-center"
            style={{ background: `color-mix(in srgb, ${accentFor(t.kind)} 16%, transparent)`, color: accentFor(t.kind) }}
          >
            {iconFor(t.kind)}
          </div>
          <div className="min-w-0 flex-1">
            <div className="text-[13px] font-semibold text-[var(--text-primary)] truncate">{t.title}</div>
            {t.body && (
              <div className="text-xs text-[var(--text-secondary)] mt-0.5 line-clamp-2 break-words whitespace-pre-wrap">
                {t.body}
              </div>
            )}
          </div>
        </div>
      ))}
    </div>
  )
}
