/**
 * 全局命令面板（Cmd+K / Ctrl+K）：
 * - 动作注册表由 Home 组装（后端静态命令 + 前端动态命令：会话切换/模型切换/斜杠指令）
 * - fuzzy 搜索：原文包含 / 中文全拼 / 拼音首字母（复用 utils/pinyin）
 * - 键盘导航：↑↓ 移动、Enter 执行、Esc 关闭、输入框内 Ctrl+K 关闭
 * - 按 group 分组渲染，空组隐藏；无结果时展示空态提示
 */
import { useEffect, useMemo, useRef, useState } from 'react'
import Icon, { type IconName } from '../icons/Icon'
import { pinyinMatch } from '../utils/pinyin'

/** 命令面板条目（由调用方组装，执行逻辑就地注入） */
export interface PaletteCommand {
  id: string
  title: string
  subtitle?: string
  group: string
  icon?: IconName
  /** 附加搜索关键字（如英文名/路径），与标题一起参与 fuzzy 匹配 */
  keywords?: string
  run: () => void
}

/** 拍平后的渲染项：组头 或 命令项 */
type FlatItem = { kind: 'header'; group: string } | { kind: 'item'; cmd: PaletteCommand }

export default function CommandPalette({
  open,
  onClose,
  commands,
  placeholder,
}: {
  open: boolean
  onClose: () => void
  commands: PaletteCommand[]
  placeholder?: string
}) {
  const [query, setQuery] = useState('')
  // 选中项在"命令项序列"中的下标（不含组头）
  const [sel, setSel] = useState(0)
  const inputRef = useRef<HTMLInputElement>(null)
  const listRef = useRef<HTMLDivElement>(null)

  // 打开时：重置查询/选中、聚焦输入框（等弹层挂载动画完成）
  useEffect(() => {
    if (open) {
      setQuery('')
      setSel(0)
      const h = setTimeout(() => inputRef.current?.focus(), 30)
      return () => clearTimeout(h)
    }
  }, [open])

  // 过滤：保持组序（首次出现顺序），组内保持原顺序
  const filtered = useMemo(() => {
    const q = query.trim()
    const groups = new Map<string, PaletteCommand[]>()
    for (const c of commands) {
      if (q && !pinyinMatch(`${c.title} ${c.subtitle ?? ''} ${c.keywords ?? ''}`, q)) continue
      const arr = groups.get(c.group)
      if (arr) arr.push(c)
      else groups.set(c.group, [c])
    }
    return [...groups.entries()].map(([group, items]) => ({ group, items }))
  }, [commands, query])

  // 拍平为渲染列表（组头 + 命令项），索引即渲染下标
  const flat = useMemo<FlatItem[]>(() => {
    const list: FlatItem[] = []
    for (const g of filtered) {
      list.push({ kind: 'header', group: g.group })
      for (const c of g.items) list.push({ kind: 'item', cmd: c })
    }
    return list
  }, [filtered])

  // 命令项在 flat 中的下标集合（sel 是该集合内的序号）
  const itemIndexes = useMemo(() => {
    const idx: number[] = []
    flat.forEach((e, i) => {
      if (e.kind === 'item') idx.push(i)
    })
    return idx
  }, [flat])
  const selFlat = itemIndexes.length > 0 ? itemIndexes[Math.min(sel, itemIndexes.length - 1)] : -1

  // 选中变化时滚动到视野（只滚列表容器）
  useEffect(() => {
    if (selFlat < 0) return
    const el = listRef.current?.querySelector<HTMLElement>(`[data-palette-idx="${selFlat}"]`)
    el?.scrollIntoView({ block: 'nearest' })
  }, [selFlat])

  const runSelected = () => {
    if (selFlat < 0) return
    const e = flat[selFlat]
    if (e.kind === 'item') {
      onClose()
      e.cmd.run()
    }
  }

  // 键盘：↑↓ 移动 / Enter 执行 / Esc 关闭 / Ctrl+K 关闭
  useEffect(() => {
    if (!open) return
    const onKey = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'k') {
        e.preventDefault()
        onClose()
        return
      }
      if (e.key === 'Escape') {
        e.preventDefault()
        onClose()
        return
      }
      if (e.key === 'ArrowDown') {
        e.preventDefault()
        setSel((s) => (itemIndexes.length ? (s + 1) % itemIndexes.length : 0))
      } else if (e.key === 'ArrowUp') {
        e.preventDefault()
        setSel((s) => (itemIndexes.length ? (s - 1 + itemIndexes.length) % itemIndexes.length : 0))
      } else if (e.key === 'Enter') {
        e.preventDefault()
        runSelected()
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, itemIndexes.length, selFlat, flat])

  if (!open) return null

  return (
    <div
      className="cmdk-backdrop"
      onMouseDown={onClose}
    >
      <div
        className="w-[600px] max-w-[92vw] rounded-2xl glass-card overflow-hidden animate-modal-in"
        onMouseDown={(e) => e.stopPropagation()}
      >
        {/* 输入框 */}
        <div className="flex items-center gap-2.5 px-4 h-12 border-b border-[var(--border)]">
          <Icon name="search" size={16} className="text-[var(--text-muted)] shrink-0" />
          <input
            ref={inputRef}
            value={query}
            onChange={(e) => {
              setQuery(e.target.value)
              setSel(0)
            }}
            placeholder={placeholder ?? '输入命令、会话或模型名称…'}
            className="flex-1 bg-transparent outline-none text-[14px] placeholder:text-[var(--text-muted)]"
            spellCheck={false}
          />
          <kbd className="shrink-0 text-[10px] px-1.5 py-0.5 rounded border border-[var(--border)] bg-[var(--bg-hover)] text-[var(--text-muted)] tnum">
            Esc
          </kbd>
        </div>
        {/* 结果列表 */}
        <div ref={listRef} className="max-h-[46vh] overflow-y-auto py-1.5">
          {flat.length === 0 && (
            <div className="px-4 py-8 text-center text-[13px] text-[var(--text-muted)]">
              没有匹配的命令
            </div>
          )}
          {flat.map((e, i) =>
            e.kind === 'header' ? (
              <div
                key={`h-${e.group}`}
                className="group-label"
                style={{ paddingLeft: 16, paddingRight: 16 }}
              >
                <span>{e.group}</span>
              </div>
            ) : (
              <button
                key={e.cmd.id}
                data-palette-idx={i}
                onMouseEnter={() => setSel(itemIndexes.indexOf(i))}
                onClick={runSelected}
                className={`w-full flex items-center gap-3 px-4 py-2 text-left transition-colors list-row ${
                  i === selFlat ? 'bg-[var(--accent-soft)] is-active' : 'hover:bg-[var(--bg-hover)]'
                }`}
              >
                <Icon
                  name={e.cmd.icon ?? 'check'}
                  size={15}
                  className={`shrink-0 ${i === selFlat ? 'text-[var(--accent)]' : 'text-[var(--text-muted)]'}`}
                />
                <span
                  className={`min-w-0 flex-1 truncate text-[13px] ${
                    i === selFlat ? 'text-[var(--accent)]' : 'text-[var(--text-primary)]'
                  }`}
                >
                  {e.cmd.title}
                </span>
                {e.cmd.subtitle && (
                  <span className="shrink-0 text-[11px] text-[var(--text-muted)] truncate max-w-[240px]">
                    {e.cmd.subtitle}
                  </span>
                )}
              </button>
            ),
          )}
        </div>
        {/* 底部提示 */}
        <div className="flex items-center gap-3 px-4 h-9 border-t border-[var(--border)] text-[11px] text-[var(--text-muted)] tnum">
          <span>
            <kbd className="px-1 py-0.5 rounded border border-[var(--border)] bg-[var(--bg-hover)]">↑</kbd>{' '}
            <kbd className="px-1 py-0.5 rounded border border-[var(--border)] bg-[var(--bg-hover)]">↓</kbd> 选择
          </span>
          <span>
            <kbd className="px-1 py-0.5 rounded border border-[var(--border)] bg-[var(--bg-hover)]">Enter</kbd> 执行
          </span>
          <span>
            <kbd className="px-1 py-0.5 rounded border border-[var(--border)] bg-[var(--bg-hover)]">Esc</kbd> 关闭
          </span>
        </div>
      </div>
    </div>
  )
}
