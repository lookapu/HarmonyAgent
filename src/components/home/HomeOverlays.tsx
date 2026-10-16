/**
 * 会话级弹层：快捷键速查面板、项目切换器
 *
 * 从 Home.tsx 搬出（原 L8410-8699）。**纯代码移动，逻辑未改。**
 * 两者都只用 store + i18n，不依赖 Home 的任何局部状态，
 * 所以搬出后 Home 只需从 store 重新取一次即可，无需透传 props。
 */

import { useEffect, useMemo, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import type { Project } from '../../api/project'
import { useProjectStore } from '../../stores/projectStore'
import { useEscapeKey } from '../../hooks/useEscapeKey'
import Icon from '../../icons/Icon'
import { IconButton } from '../ui/IconButton'
import { EmptyState as UiEmptyState } from '../ui/EmptyState'

/* ============ 快捷键速查面板（? 触发）：分组 + 搜索 + 一键复制键位 ============
 * - 数据结构硬编码在组件内：每组若干 { keys, desc, groupKey }
 * - 顶部搜索框：模糊匹配 desc / keys
 * - 点击行复制键位组合到剪贴板（如 "Ctrl+Shift+P"），toast 反馈
 * - Esc 关闭
 */
export function ShortcutsPanel({ onClose }: { onClose: () => void }) {
  const { t } = useTranslation()
  const [query, setQuery] = useState('')
  const [copiedKey, setCopiedKey] = useState<string | null>(null)
  const inputRef = useRef<HTMLInputElement>(null)

  // 全部快捷键（硬编码）：分组 → [{ keys, desc, descKey }]
  const groups: { titleKey: string; items: { keys: string[]; descKey: string; copyKey: string }[] }[] = [
    {
      titleKey: 'home.shortcutGroupNav',
      items: [
        { keys: ['Ctrl', 'K'], descKey: 'home.shortcutCommandPalette', copyKey: 'Ctrl+K' },
        { keys: ['Ctrl', 'Shift', 'K'], descKey: 'home.shortcutFocusSearch', copyKey: 'Ctrl+Shift+K' },
        { keys: ['Ctrl', 'Shift', 'P'], descKey: 'home.shortcutProjectSwitcher', copyKey: 'Ctrl+Shift+P' },
        { keys: ['Ctrl', 'Shift', 'N'], descKey: 'home.shortcutNewConv', copyKey: 'Ctrl+Shift+N' },
        { keys: ['?'], descKey: 'home.shortcutThisPanel', copyKey: '?' },
        { keys: ['Esc'], descKey: 'home.shortcutClose', copyKey: 'Esc' },
      ],
    },
    {
      titleKey: 'home.shortcutGroupQuick',
      items: [
        { keys: ['Ctrl', 'Shift', 'B'], descKey: 'home.shortcutBuild', copyKey: 'Ctrl+Shift+B' },
        { keys: ['Ctrl', 'Shift', 'D'], descKey: 'home.shortcutDeploy', copyKey: 'Ctrl+Shift+D' },
        { keys: ['Ctrl', 'Shift', 'S'], descKey: 'home.shortcutScreenshot', copyKey: 'Ctrl+Shift+S' },
        { keys: ['Ctrl', 'Shift', 'R'], descKey: 'home.shortcutRunCommand', copyKey: 'Ctrl+Shift+R' },
      ],
    },
    {
      titleKey: 'home.shortcutGroupInput',
      items: [
        { keys: ['Enter'], descKey: 'home.shortcutSend', copyKey: 'Enter' },
        { keys: ['Shift', 'Enter'], descKey: 'home.shortcutNewline', copyKey: 'Shift+Enter' },
        { keys: ['Ctrl', 'Enter'], descKey: 'home.shortcutSendCmd', copyKey: 'Ctrl+Enter' },
      ],
    },
    {
      titleKey: 'home.shortcutGroupMessage',
      items: [
        { keys: ['右键'], descKey: 'home.shortcutContextMenu', copyKey: 'RightClick' },
      ],
    },
  ]

  // 过滤：query 命中 keys 或 desc
  const q = query.trim().toLowerCase()
  const filtered = q
    ? groups
        .map((g) => ({
          ...g,
          items: g.items.filter((it) => {
            const desc = t(it.descKey).toLowerCase()
            const keyStr = it.copyKey.toLowerCase()
            return desc.includes(q) || keyStr.includes(q)
          }),
        }))
        .filter((g) => g.items.length > 0)
    : groups

  // 复制键位
  const copyKey = async (text: string) => {
    try {
      await navigator.clipboard.writeText(text)
      setCopiedKey(text)
      setTimeout(() => setCopiedKey((cur) => (cur === text ? null : cur)), 1200)
    } catch {
      // 剪贴板不可用 → 静默
    }
  }

  // 自动聚焦搜索框
  useEffect(() => {
    inputRef.current?.focus()
  }, [])
  useEscapeKey(onClose)

  return (
    <div className="cmdk-backdrop" onClick={onClose}>
      <div
        className="w-[560px] max-w-[92vw] max-h-[82vh] flex flex-col glass-card p-5 animate-modal-in"
        onClick={(e) => e.stopPropagation()}
      >
        {/* 标题 + 关闭 */}
        <div className="flex items-center justify-between mb-3">
          <div className="flex items-center gap-2">
            <Icon name="bolt" size={15} />
            <h2 className="text-[length:var(--app-text-base)] font-semibold">{t('home.shortcuts')}</h2>
          </div>
          <IconButton icon="close" label={t('common.close')} onClick={onClose} />
        </div>

        {/* 搜索框 */}
        <div className="mb-3 flex items-center gap-2 px-2.5 h-8 rounded-lg bg-[var(--bg-primary)] border border-[var(--border)] focus-within:border-[var(--accent)] transition-colors">
          <Icon name="search" size={12} className="text-[var(--text-muted)]" />
          <input
            ref={inputRef}
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder={t('home.shortcutSearchPlaceholder')}
            spellCheck={false}
            className="flex-1 bg-transparent outline-none text-[length:var(--app-text-sm)] placeholder:text-[var(--text-muted)]"
          />
          {query && (
            <button
              aria-label={t('home.clearSearch')}
              title={t('home.clearSearch')}
              onClick={() => setQuery('')}
              className="text-[length:var(--app-text-2xs)] text-[var(--text-muted)] hover:text-[var(--text-primary)]"
            >
              <Icon name="close" size={11} />
            </button>
          )}
        </div>

        {/* 分组列表 */}
        <div className="space-y-3 text-[length:var(--app-text-sm)] overflow-y-auto min-h-0 flex-1">
          {filtered.length === 0 ? (
            <UiEmptyState compact title={t('home.shortcutNoResults')} />
          ) : (
            filtered.map((g) => (
              <div key={g.titleKey}>
                <p className="text-[length:var(--app-text-xs)] font-medium text-[var(--text-muted)] uppercase tracking-wider mb-1.5">
                  {t(g.titleKey)}
                </p>
                {g.items.map((it) => {
                  const isCopied = copiedKey === it.copyKey
                  return (
                    <div
                      key={it.copyKey}
                      onClick={() => void copyKey(it.copyKey)}
                      title={t('home.shortcutCopyHint')}
                      className="group flex items-center justify-between gap-2 py-1 px-1.5 -mx-1.5 rounded-md hover:bg-[var(--bg-hover)] cursor-pointer transition-colors"
                    >
                      <span className="text-[var(--text-secondary)] text-[length:var(--app-text-sm)]">{t(it.descKey)}</span>
                      <span className="flex items-center gap-0.5 shrink-0">
                        {it.keys.map((k, i) => (
                          <span key={i} className="flex items-center gap-0.5">
                            {i > 0 && <span className="text-[length:var(--app-text-2xs)] text-[var(--text-muted)]">+</span>}
                            <kbd className="text-[length:var(--app-text-2xs)] px-1.5 py-0.5 rounded border border-[var(--border)] bg-[var(--bg-hover)] text-[var(--text-muted)] tnum">
                              {k}
                            </kbd>
                          </span>
                        ))}
                        <Icon
                          name={isCopied ? 'check' : 'copy'}
                          size={10}
                          className={`ml-1.5 transition-[color,background-color,border-color,opacity] ${isCopied ? 'text-[var(--success)] opacity-100' : 'text-[var(--text-muted)] opacity-0 group-hover:opacity-100'}`}
                        />
                      </span>
                    </div>
                  )
                })}
              </div>
            ))
          )}
        </div>

        {/* 底部提示 */}
        <p className="mt-3 pt-2.5 border-t border-[var(--border)] text-[length:var(--app-text-xs)] text-[var(--text-muted)]">
          {t('home.shortcutFooter')}
        </p>
      </div>
    </div>
  )
}


/* ============ 项目快速切换器：Ctrl+Shift+P 触发，fuzzy 搜项目 + 最近 5 个置顶 ============ */
export function ProjectSwitcher({ onClose, onSelect }: { onClose: () => void; onSelect: (id: string) => void }) {
  const { t } = useTranslation()
  const projects = useProjectStore((s) => s.projects) as Project[]
  const currentProject = useProjectStore((s) => s.currentProject)
  const [query, setQuery] = useState('')
  const [sel, setSel] = useState(0)
  const listRef = useRef<HTMLDivElement>(null)
  const inputRef = useRef<HTMLInputElement>(null)

  // fuzzy 过滤：项目名 / 路径包含 query
  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase()
    const list = !q
      ? projects
      : projects.filter((p) => p.name.toLowerCase().includes(q) || p.path.toLowerCase().includes(q))
    // 当前项目置顶 + 最近 5 个靠前
    return [...list].sort((a, b) => {
      if (a.id === currentProject?.id) return -1
      if (b.id === currentProject?.id) return 1
      return (b.last_opened_at ?? 0) - (a.last_opened_at ?? 0)
    })
  }, [projects, query, currentProject?.id])

  // 打开时聚焦输入框
  useEffect(() => {
    const h = setTimeout(() => inputRef.current?.focus(), 30)
    return () => clearTimeout(h)
  }, [])

  // 键盘：↑↓ 移动 / Enter 选择（Esc 走全局栈，见下）
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'ArrowDown') {
        e.preventDefault()
        setSel((s) => Math.min(filtered.length - 1, s + 1))
      } else if (e.key === 'ArrowUp') {
        e.preventDefault()
        setSel((s) => Math.max(0, s - 1))
      } else if (e.key === 'Enter') {
        e.preventDefault()
        const p = filtered[sel]
        if (p) onSelect(p.id)
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [filtered, sel, onSelect])
  useEscapeKey(onClose)

  return (
    <div
      className="w-[520px] max-w-[92vw] rounded-2xl glass-card overflow-hidden animate-modal-in"
      onClick={(e) => e.stopPropagation()}
    >
      <div className="flex items-center gap-2.5 px-4 h-12 border-b border-[var(--border)]">
        <Icon name="folder" size={16} className="text-[var(--text-muted)] shrink-0" />
        <input
          ref={inputRef}
          value={query}
          onChange={(e) => {
            setQuery(e.target.value)
            setSel(0)
          }}
          placeholder={t('home.projectSwitcherPlaceholder')}
          className="flex-1 bg-transparent outline-none text-[length:var(--app-text-base)] placeholder:text-[var(--text-muted)]"
          spellCheck={false}
        />
        <kbd className="shrink-0 text-[length:var(--app-text-2xs)] px-1.5 py-0.5 rounded border border-[var(--border)] bg-[var(--bg-hover)] text-[var(--text-muted)] tnum">Esc</kbd>
      </div>
      <div ref={listRef} className="max-h-[50vh] overflow-y-auto py-1.5">
        {filtered.length === 0 ? (
          <div className="px-4 py-8 text-center text-[length:var(--app-text-md)] text-[var(--text-muted)]">{t('home.projectSwitcherEmpty')}</div>
        ) : (
          filtered.map((p, i) => {
            const active = i === sel
            const isCurrent = p.id === currentProject?.id
            return (
              <button
                key={p.id}
                onClick={() => onSelect(p.id)}
                onMouseEnter={() => setSel(i)}
                className={`w-full flex items-center gap-2.5 px-4 py-2 text-left transition-colors ${
                  active ? 'bg-[var(--accent-soft)]' : 'hover:bg-[var(--bg-hover)]'
                }`}
              >
                <div className="w-7 h-7 rounded-lg bg-gradient-to-br from-[var(--accent)] to-[#8b5cf6] flex items-center justify-center shrink-0">
                  <Icon name="folder" size={13} white />
                </div>
                <div className="min-w-0 flex-1">
                  <div className="text-[length:var(--app-text-md)] truncate flex items-center gap-1.5">
                    {p.name}
                    {isCurrent && (
                      <span className="text-[length:var(--app-text-2xs)] px-1 rounded bg-[var(--accent)]/15 text-[var(--accent)]">{t('home.projectSwitcherCurrent')}</span>
                    )}
                  </div>
                  <div className="text-[length:var(--app-text-xs)] text-[var(--text-muted)] truncate font-mono">{p.path}</div>
                </div>
                {p.index_state && (
                  <span className="text-[length:var(--app-text-2xs)] text-[var(--text-muted)] tnum shrink-0">
                    {p.index_state === 'ready' ? '✓' : '…'}
                  </span>
                )}
              </button>
            )
          })
        )}
      </div>
      <div className="flex items-center gap-3 px-4 h-9 border-t border-[var(--border)] text-[length:var(--app-text-xs)] text-[var(--text-muted)] tnum">
        <span><kbd className="px-1 py-0.5 rounded border border-[var(--border)] bg-[var(--bg-hover)]">↑</kbd> <kbd className="px-1 py-0.5 rounded border border-[var(--border)] bg-[var(--bg-hover)]">↓</kbd> {t('home.projectSwitcherNavigate')}</span>
        <span><kbd className="px-1 py-0.5 rounded border border-[var(--border)] bg-[var(--bg-hover)]">Enter</kbd> {t('home.projectSwitcherOpen')}</span>
      </div>
    </div>
  )
}
