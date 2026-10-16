/**
 * 全局命令面板（Cmd+K / Ctrl+K）：
 * - 动作注册表由 Home 组装（后端静态命令 + 前端动态命令：会话切换/模型切换/斜杠指令）
 * - fuzzy 搜索：原文包含 / 中文全拼 / 拼音首字母（复用 utils/pinyin）
 * - 键盘导航：↑↓ 移动、Enter 执行、Esc 关闭、输入框内 Ctrl+K 关闭
 * - 按 group 分组渲染，空组隐藏；无结果时展示空态提示
 *
 * ## 无障碍：为什么用 combobox + listbox + aria-activedescendant
 *
 * 这是 W3C combobox 模式。关键是**焦点始终留在输入框**，选中项靠
 * `aria-activedescendant` 指向当前项的 id 告知读屏软件。
 *
 * 另一条路是让每个 option 都能 Tab 聚焦（roving tabindex），但那样：
 * ① Tab 会跳到每一项，用户按一次 Tab 就掉出输入框，搜索体验直接崩掉；
 * ② 焦点离开输入框后 `onChange` 之外还得自己管焦点回归。
 * 命令面板的核心交互是「一直在打字」，焦点必须留在输入框。
 *
 * 改造前这个面板对读屏软件几乎不可用：列表没有 listbox 角色、选项没有 option、
 * 选中态只体现在背景色上——读屏用户**听不出自己选中了第几项**。
 */
import { useEffect, useReducer, useRef, useState } from 'react'
import Icon, { type IconName } from '../icons/Icon'
import { pinyinMatch, preloadPinyin, onPinyinReady } from '../utils/pinyin'
import { track } from '../utils/telemetry'

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
  // 稳定的 DOM id：aria-activedescendant 要指向具体 option 的 id，
  // 而 aria-controls 要指向 listbox 本身。id 必须是稳定的，不能每次渲染现生成。
  const [uid] = useState(() => `cmdk-${Math.random().toString(36).slice(2, 8)}`)
  const listboxId = `${uid}-listbox`
  const optionId = (i: number) => `${uid}-opt-${i}`

  // 打开时：重置查询/选中、聚焦输入框（等弹层挂载动画完成）
  useEffect(() => {
    if (open) {
      setQuery('')
      setSel(0)
      // 拼音词典懒加载（见 utils/pinyin.ts）：打开面板就开始预热，
      // 用户打完第一个查询词之前通常已就绪
      void preloadPinyin()
      const h = setTimeout(() => inputRef.current?.focus(), 30)
      return () => clearTimeout(h)
    }
  }, [open])

  // 拼音词典就绪 → 触发一次重渲染，好让下面的过滤重跑。
  // 用 useReducer 的 dispatch 而不是 useState 布尔值：这里**不需要**知道是否就绪，
  // 只需要在就绪那一刻重渲染一次。持有状态反而会诱使人去读它、给它加分支，
  // 而它一旦被读就会变成渲染逻辑的一部分——那正是它不该有的身份。
  // 没有这一条，用户输入「sy」这类拼音首字母时会搜不到「首页」且不会自愈。
  const [, onDictReady] = useReducer((n: number) => n + 1, 0)
  useEffect(() => onPinyinReady(() => onDictReady()), [])

  // 过滤 → 拍平 → 选中下标：三段都**不用 useMemo**。
  //
  // commands 来自后端、量级在几十条，一次 map + filter + 拍平的代价远小于
  // memo 的维护成本。更关键的是另一条理由：要让这段在词典就绪后重算，
  // 依赖数组里必须放 pinyinReady——而它是个「只作触发器、不参与计算」的值，
  // eslint 的 exhaustive-deps 会判定它是 unnecessary dependency。
  // 一旦有人照着提示把它删掉，拼音搜索就静默失效且不自愈——
  // 这正是本轮要消灭的那类缺陷。改成直接计算，依赖根本不存在。
  const q = query.trim()
  const filtered: { group: string; items: PaletteCommand[] }[] = (() => {
    // 保持组序（首次出现顺序），组内保持原顺序
    const groups = new Map<string, PaletteCommand[]>()
    for (const c of commands) {
      if (q && !pinyinMatch(`${c.title} ${c.subtitle ?? ''} ${c.keywords ?? ''}`, q)) continue
      const arr = groups.get(c.group)
      if (arr) arr.push(c)
      else groups.set(c.group, [c])
    }
    return [...groups.entries()].map(([group, items]) => ({ group, items }))
  })()

  // 拍平为渲染列表（组头 + 命令项），索引即渲染下标
  const flat: FlatItem[] = (() => {
    const list: FlatItem[] = []
    for (const g of filtered) {
      list.push({ kind: 'header', group: g.group })
      for (const c of g.items) list.push({ kind: 'item', cmd: c })
    }
    return list
  })()

  // 命令项在 flat 中的下标集合（sel 是该集合内的序号）
  const itemIndexes: number[] = []
  flat.forEach((e, i) => {
    if (e.kind === 'item') itemIndexes.push(i)
  })
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
      // 记命令 id（低基数、可聚合），不记标题——标题含用户填的搜索词
      track('command_palette_used', { command: e.cmd.id })
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
        {/* 输入框：combobox。焦点始终留在这里，选中项靠 aria-activedescendant 告知 */}
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
            role="combobox"
            aria-expanded={flat.length > 0}
            aria-controls={listboxId}
            aria-activedescendant={selFlat >= 0 ? optionId(selFlat) : undefined}
            aria-autocomplete="list"
            aria-label={placeholder ?? '输入命令、会话或模型名称…'}
            className="flex-1 bg-transparent outline-none text-[length:var(--app-text-base)] placeholder:text-[var(--text-muted)]"
            spellCheck={false}
          />
          <kbd className="shrink-0 text-[length:var(--app-text-2xs)] px-1.5 py-0.5 rounded border border-[var(--border)] bg-[var(--bg-hover)] text-[var(--text-muted)] tnum">
            Esc
          </kbd>
        </div>
        {/* 结果列表 */}
        <div
          ref={listRef}
          id={listboxId}
          role="listbox"
          aria-label={placeholder ?? '命令结果'}
          className="max-h-[46vh] overflow-y-auto py-1.5"
        >
          {flat.length === 0 && (
            <div className="px-4 py-8 text-center text-[length:var(--app-text-md)] text-[var(--text-muted)]">
              没有匹配的命令
            </div>
          )}
          {flat.map((e, i) =>
            e.kind === 'header' ? (
              <div
                key={`h-${e.group}`}
                className="group-label px-4"
              >
                <span>{e.group}</span>
              </div>
            ) : (
              <button
                key={e.cmd.id}
                id={optionId(i)}
                role="option"
                aria-selected={i === selFlat}
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
                  className={`min-w-0 flex-1 truncate text-[length:var(--app-text-md)] ${
                    i === selFlat ? 'text-[var(--accent)]' : 'text-[var(--text-primary)]'
                  }`}
                >
                  {e.cmd.title}
                </span>
                {e.cmd.subtitle && (
                  <span className="shrink-0 text-[length:var(--app-text-xs)] text-[var(--text-muted)] truncate max-w-[240px]">
                    {e.cmd.subtitle}
                  </span>
                )}
              </button>
            ),
          )}
        </div>
        {/* 底部提示 */}
        <div className="flex items-center gap-3 px-4 h-9 border-t border-[var(--border)] text-[length:var(--app-text-xs)] text-[var(--text-muted)] tnum">
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
