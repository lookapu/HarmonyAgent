/**
 * 符号检索面板：按名称检索组件/函数/类等，点击内联查看并定位行号。
 * （含 SYMBOL_KINDS / symbolKindColor——只有本面板用）
 */
import { useEffect, useMemo, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { readProjectFile } from '../../../api/project'
import {
  indexProjectSymbols,
  refreshProjectSymbols,
  searchSymbols as searchSymbolsApi,
  searchSymbolsAll,
  symbolIndexMeta,
  type CodeSymbol,
  type CrossProjectSymbol,
  type SymbolIndexMeta,
} from '../../../api/symbols'
import Icon from '../../../icons/Icon'

const SYMBOL_KINDS: { value: string; labelKey: string }[] = [
  { value: '', labelKey: 'home.symbolKindAll' },
  { value: 'component', labelKey: 'home.symbolKindComponent' },
  { value: 'function', labelKey: 'home.symbolKindFunction' },
  { value: 'method', labelKey: 'home.symbolKindMethod' },
  { value: 'class', labelKey: 'home.symbolKindClass' },
  { value: 'interface', labelKey: 'home.symbolKindInterface' },
  { value: 'route', labelKey: 'home.symbolKindRoute' },
  { value: 'struct', labelKey: 'home.symbolKindStruct' },
  { value: 'enum', labelKey: 'home.symbolKindEnum' },
]

function symbolKindColor(kind: string): string {
  switch (kind) {
    case 'component':
      return 'var(--accent)'
    case 'function':
    case 'method':
      return '#10b981'
    case 'class':
    case 'struct':
      return '#f59e0b'
    case 'interface':
      return '#8b5cf6'
    case 'route':
      return '#ec4899'
    case 'enum':
      return '#06b6d4'
    default:
      return 'var(--text-muted)'
  }
}

/* ============ 符号检索面板：按名称检索组件/函数/类等，点击内联查看并定位行号 ============ */
export function SymbolsPanel({
  projectId,
  projectName,
  onReference,
  root,
}: {
  projectId: string
  projectName?: string
  onReference: (path: string) => void
  /** 会话工作目录（worktree 模式为 worktree 路径，本地模式为 undefined） */
  root?: string
}) {
  const { t } = useTranslation()
  const [query, setQuery] = useState('')
  const [kind, setKind] = useState('')
  const [scope, setScope] = useState<'project' | 'all'>('project')
  const [symbols, setSymbols] = useState<CodeSymbol[]>([])
  const [loading, setLoading] = useState(false)
  const [refreshing, setRefreshing] = useState(false)
  const [searching, setSearching] = useState(false)
  const [meta, setMeta] = useState<SymbolIndexMeta | null>(null)
  const [error, setError] = useState<string | null>(null)
  // 内联代码查看：当前打开的符号/文件
  const [viewer, setViewer] = useState<{ file: string; line: number; content: string } | null>(null)
  const [viewerLoading, setViewerLoading] = useState(false)
  const viewerRef = useRef<HTMLDivElement>(null)
  // Outline 分组视图：按第一层文件夹聚合，两级（目录/文件）均可折叠
  // （默认全部折叠，点击展开；有搜索词时自动展开匹配项）
  const [collapsedDirs, setCollapsedDirs] = useState<Set<string>>(() => new Set())
  const [collapsedFiles, setCollapsedFiles] = useState<Set<string>>(() => new Set())
  const didInitCollapse = useRef(false)

  const dirs = useMemo(() => {
    type FileGroup = { key: string; file: string; syms: CodeSymbol[] }
    type DirGroup = { key: string; dir: string; project?: string; files: FileGroup[] }
    const map = new Map<string, DirGroup>()
    for (const s of symbols) {
      const cs = s as CrossProjectSymbol
      const project = scope === 'all' && cs.project_name ? cs.project_name : undefined
      // 第一层文件夹：相对路径的首段；项目根下的零散文件归入“（项目根）”
      const top = s.file.split('/')[0] || t('home.symbolRootDir')
      const dirKey = project ? `${project}::${top}` : top
      let d = map.get(dirKey)
      if (!d) {
        d = { key: dirKey, dir: top, project, files: [] }
        map.set(dirKey, d)
      }
      const fKey = `${dirKey}::${s.file}`
      const f = d.files.find((x) => x.key === fKey)
      if (f) f.syms.push(s)
      else d.files.push({ key: fKey, file: s.file, syms: [s] })
    }
    const list = Array.from(map.values()).sort((a, b) => a.key.localeCompare(b.key))
    for (const d of list) d.files.sort((a, b) => a.file.localeCompare(b.file))
    return list
  }, [symbols, scope, t])

  // 默认全部折叠：首次加载完成（无搜索词）时把所有目录/文件 key 加入折叠集合；
  // 用户输入搜索词时自动展开全部，便于浏览匹配结果。
  useEffect(() => {
    if (query.trim() || kind) {
      setCollapsedDirs(new Set())
      setCollapsedFiles(new Set())
      return
    }
    if (dirs.length > 0 && !didInitCollapse.current) {
      didInitCollapse.current = true
      setCollapsedDirs(new Set(dirs.map((d) => d.key)))
      setCollapsedFiles(new Set(dirs.flatMap((d) => d.files.map((f) => f.key))))
    }
  }, [dirs, query, kind])

  const toggleDir = (d: string) => {
    setCollapsedDirs((prev) => {
      const next = new Set(prev)
      if (next.has(d)) next.delete(d)
      else next.add(d)
      return next
    })
  }

  const toggleFile = (f: string) => {
    setCollapsedFiles((prev) => {
      const next = new Set(prev)
      if (next.has(f)) next.delete(f)
      else next.add(f)
      return next
    })
  }

  const handleRefresh = async () => {
    if (scope !== 'project' || refreshing) return
    setRefreshing(true)
    setError(null)
    try {
      const list = await refreshProjectSymbols(projectId, root)
      setSymbols(list)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setRefreshing(false)
    }
  }

  // 索引元信息：加载/刷新完成后拉取一次，展示符号/文件数量与缓存来源
  useEffect(() => {
    if (scope !== 'project' || loading || refreshing) return
    let cancelled = false
    symbolIndexMeta(projectId, root)
      .then((m) => {
        if (!cancelled) setMeta(m)
      })
      .catch(() => {})
    return () => {
      cancelled = true
    }
  }, [projectId, root, scope, loading, refreshing])

  // 首次进入：构建当前项目索引（全量扫描；「全部项目」范围不预加载，等用户输入检索）
  useEffect(() => {
    if (scope !== 'project') return
    let cancelled = false
    setLoading(true)
    setError(null)
    indexProjectSymbols(projectId, root)
      .then((list) => {
        if (!cancelled) setSymbols(list)
      })
      .catch((e) => {
        if (!cancelled) setError(e instanceof Error ? e.message : String(e))
      })
      .finally(() => {
        if (!cancelled) setLoading(false)
      })
    return () => {
      cancelled = true
    }
  }, [projectId, root, scope])

  // 输入防抖检索（当前项目或全部项目）
  useEffect(() => {
    const handle = setTimeout(() => {
      let cancelled = false
      if (!query.trim() && !kind) {
        if (scope === 'all') setSymbols([])
        return
      }
      const p =
        scope === 'all'
          ? searchSymbolsAll(query.trim(), kind || undefined)
          : searchSymbolsApi(projectId, query.trim(), kind || undefined, root)
      if (!cancelled) setSearching(true)
      p.then((list) => {
        if (!cancelled) setSymbols(list)
      })
        .catch(() => {})
        .finally(() => {
          if (!cancelled) setSearching(false)
        })
      return () => {
        cancelled = true
      }
    }, 220)
    return () => clearTimeout(handle)
  }, [query, kind, projectId, root, scope])

  const openSymbol = async (sym: CodeSymbol) => {
    setViewer({ file: sym.file, line: sym.line, content: '' })
    setViewerLoading(true)
    try {
      const res = await readProjectFile(projectId, sym.file, root)
      setViewer({ file: sym.file, line: sym.line, content: res.content })
      // 等行渲染后滚动到目标行
      requestAnimationFrame(() => {
        viewerRef.current
          ?.querySelector(`[data-line="${sym.line}"]`)
          ?.scrollIntoView({ block: 'center', behavior: 'smooth' })
      })
    } catch (e) {
      // catch 变量在非 strict 配置下为 {}，instanceof 收窄无效；用断言安全提取 message
      const msg = String((e as Error | null)?.message ?? e)
      setViewer({ file: sym.file, line: sym.line, content: `// ${msg}` })
    } finally {
      setViewerLoading(false)
    }
  }

  const lines = viewer ? viewer.content.split('\n') : []
  const startLine = Math.max(1, viewer ? viewer.line - 8 : 1)

  return (
    <div className="flex flex-col h-full min-h-0">
      {/* 搜索栏 */}
      <div className="p-2.5 border-b border-[var(--border)] shrink-0 space-y-2">
        <div className="relative">
          <span className="absolute left-2.5 top-1/2 -translate-y-1/2 text-[var(--text-muted)]">
            <Icon name="search" size={13} />
          </span>
          <input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder={t('home.symbolSearch')}
            spellCheck={false}
            className="w-full h-8 pl-8 pr-7 rounded-lg border border-[var(--border)] bg-[var(--bg-primary)] text-[length:var(--app-text-sm)] outline-none focus:border-[var(--accent)] transition-colors"
          />
          {query && (
            <button
              onClick={() => setQuery('')}
              aria-label={t('common.clear')}
              title={t('common.clear')}
              className="absolute right-2 top-1/2 -translate-y-1/2 text-[var(--text-muted)] hover:text-[var(--text-secondary)]"
            >
              <Icon name="close" size={12} />
            </button>
          )}
        </div>
        {scope === 'project' && (
          <button
            onClick={handleRefresh}
            disabled={refreshing}
            className="w-full h-7 flex items-center justify-center gap-1.5 rounded-lg border border-[var(--border)] bg-[var(--bg-primary)] text-[length:var(--app-text-xs)] text-[var(--text-secondary)] hover:border-[var(--border-strong)] hover:text-[var(--text-primary)] transition-colors disabled:opacity-50"
          >
            <Icon name="refresh" size={12} className={refreshing ? 'animate-spin' : ''} />
            {t('home.symbolRefresh')}
          </button>
        )}
        {/* 范围切换：当前项目 / 全部项目 */}
        <div className="flex gap-1.5">
          <button
            onClick={() => setScope('project')}
            className={`flex-1 h-8 rounded-lg text-[length:var(--app-text-xs)] border transition-colors ${
              scope === 'project'
                ? 'border-[var(--accent)]/50 bg-[var(--accent-soft)] text-[var(--accent)] font-medium'
                : 'border-[var(--border)] bg-[var(--bg-primary)] text-[var(--text-secondary)] hover:border-[var(--border-strong)]'
            }`}
          >
            {projectName ? t('home.symbolScopeProject', { name: projectName }) : t('home.symbolScopeProjectShort')}
          </button>
          <button
            onClick={() => setScope('all')}
            className={`flex-1 h-8 rounded-lg text-[length:var(--app-text-xs)] border transition-colors ${
              scope === 'all'
                ? 'border-[var(--accent)]/50 bg-[var(--accent-soft)] text-[var(--accent)] font-medium'
                : 'border-[var(--border)] bg-[var(--bg-primary)] text-[var(--text-secondary)] hover:border-[var(--border-strong)]'
            }`}
          >
            {t('home.symbolScopeAll')}
          </button>
        </div>
        <select
          value={kind}
          onChange={(e) => setKind(e.target.value)}
          className="w-full h-8 px-2 rounded-lg border border-[var(--border)] bg-[var(--bg-primary)] text-[length:var(--app-text-sm)] text-[var(--text-secondary)] outline-none focus:border-[var(--accent)] transition-colors"
        >
          {SYMBOL_KINDS.map((k) => (
            <option key={k.value} value={k.value}>
              {t(k.labelKey)}
            </option>
          ))}
        </select>
      </div>

      {/* 内联代码查看器 */}
      {viewer && (
        <div className="border-b border-[var(--border)] shrink-0 max-h-[55%] flex flex-col bg-[var(--bg-primary)]">
          <div className="flex items-center gap-2 px-2.5 py-1.5 border-b border-[var(--border)] bg-[var(--bg-card)]">
            <Icon name="file" size={12} className="text-[var(--accent)]" />
            <span className="text-[length:var(--app-text-xs)] font-mono truncate flex-1 text-[var(--text-secondary)]">
              {viewer.file}:{viewer.line}
            </span>
            <button
              onClick={() => onReference(viewer.file)}
              title={t('home.symbolReference')}
              aria-label={t('home.symbolReference')}
              className="p-1 rounded text-[var(--text-muted)] hover:text-[var(--accent)] hover:bg-[var(--bg-hover)]"
            >
              <Icon name="chat" size={12} />
            </button>
            <button
              onClick={() => setViewer(null)}
              aria-label={t('common.close')}
              title={t('common.close')}
              className="p-1 rounded text-[var(--text-muted)] hover:text-[var(--text-secondary)] hover:bg-[var(--bg-hover)]"
            >
              <Icon name="close" size={12} />
            </button>
          </div>
          <div ref={viewerRef} className="flex-1 min-h-0 overflow-auto font-mono text-[length:var(--app-text-xs)] leading-[1.65] py-1">
            {viewerLoading ? (
              <div className="shimmer h-32 mx-2 my-1 rounded bg-[var(--bg-hover)]" />
            ) : (
              lines
                .map((l, i) => ({ no: i + 1, text: l }))
                .filter((row) => row.no >= startLine && row.no <= startLine + 30)
                .map((row) => (
                  <div
                    key={row.no}
                    data-line={row.no}
                    className={`flex gap-3 px-2.5 ${
                      row.no === viewer.line
                        ? 'bg-[var(--accent-soft)]'
                        : row.no % 2 === 0
                          ? 'bg-transparent'
                          : 'bg-transparent'
                    }`}
                  >
                    <span
                      className={`select-none text-right w-7 shrink-0 ${
                        row.no === viewer.line ? 'text-[var(--accent)] font-semibold' : 'text-[var(--text-muted)]/50'
                      }`}
                    >
                      {row.no}
                    </span>
                    <pre className="whitespace-pre-wrap break-all text-[var(--text-secondary)] m-0">{row.text}</pre>
                  </div>
                ))
            )}
          </div>
        </div>
      )}

      {/* 结果列表（按文件分组 Outline 视图） */}
      <div className="flex-1 min-h-0 overflow-y-auto p-1.5">
        {loading ? (
          <div className="p-3 space-y-1.5">
            <div className="flex items-center gap-1.5 text-[length:var(--app-text-xs)] text-[var(--text-muted)] pb-1">
              <Icon name="refresh" size={11} className="animate-spin" />
              {t('home.symbolScanning')}
            </div>
            {[0, 1, 2, 3].map((i) => (
              <div key={i} className="shimmer h-9 rounded-lg bg-[var(--bg-hover)]" />
            ))}
          </div>
        ) : error ? (
          <div className="p-3 text-[length:var(--app-text-xs)] text-[var(--danger)]">{error}</div>
        ) : symbols.length === 0 ? (
          <div className="h-full flex flex-col items-center justify-center gap-1.5 text-center px-4">
            <Icon name="search" size={22} className="opacity-30" />
            <span className="text-[length:var(--app-text-xs)] text-[var(--text-muted)]">{t('home.symbolEmpty')}</span>
          </div>
        ) : (
          <div className="space-y-1">
            {searching && (
              <div className="flex items-center gap-1.5 text-[length:var(--app-text-2xs)] text-[var(--text-muted)] px-1 pt-0.5 pb-1">
                <Icon name="refresh" size={10} className="animate-spin" />
                {t('home.symbolSearching')}
              </div>
            )}
            {query.trim() || kind ? (
              <div className="text-[length:var(--app-text-2xs)] text-[var(--text-muted)] px-1 pt-0.5 pb-1">
                {t('home.symbolResultCount', { count: symbols.length })}
              </div>
            ) : (
              <div className="text-[length:var(--app-text-2xs)] text-[var(--text-muted)] px-1 pt-0.5 pb-1 flex items-center gap-1.5 flex-wrap">
                {t('home.symbolIndexCount', { count: symbols.length })}
                {meta && meta.files > 0 && (
                  <>
                    <span>·</span>
                    <span>{t('home.symbolFileCount', { count: meta.files })}</span>
                    {meta.source === 'disk' && (
                      <span className="px-1 py-px rounded bg-[var(--bg-hover)] text-[length:var(--app-text-micro)]">{t('home.symbolSourceDisk')}</span>
                    )}
                  </>
                )}
              </div>
            )}
            {dirs.slice(0, 120).map((d) => {
              const dirHidden = collapsedDirs.has(d.key)
              return (
                <div key={d.key} className="rounded-lg border border-[var(--border)] overflow-hidden bg-[var(--bg-card)]">
                  <button
                    onClick={() => toggleDir(d.key)}
                    className="w-full flex items-center gap-1.5 px-2.5 py-1.5 hover:bg-[var(--bg-hover)] transition-colors text-left"
                    title={d.dir}
                  >
                    <Icon name="chevron-right" size={11} className={`transition-transform shrink-0 ${dirHidden ? '' : 'rotate-90'}`} />
                    <Icon name="folder" size={12} className="text-[var(--accent)] shrink-0" />
                    {d.project && (
                      <span className="shrink-0 text-[length:var(--app-text-micro)] font-medium px-1.5 py-0.5 rounded bg-[var(--bg-hover)] text-[var(--text-muted)]">
                        {d.project}
                      </span>
                    )}
                    <span className="text-[length:var(--app-text-xs)] font-medium truncate flex-1 text-[var(--text-primary)]">{d.dir}</span>
                    <span className="text-[length:var(--app-text-2xs)] text-[var(--text-muted)] shrink-0 tabular-nums">{d.files.length}</span>
                  </button>
                  {!dirHidden && (
                    <div className="border-t border-[var(--border)] bg-[var(--bg-primary)]/40">
                      {d.files.slice(0, 120).map((f) => {
                        const fileHidden = collapsedFiles.has(f.key)
                        return (
                          <div key={f.key}>
                            <button
                              onClick={() => toggleFile(f.key)}
                              className="w-full flex items-center gap-1.5 pl-6 pr-2.5 py-1.5 hover:bg-[var(--bg-hover)] transition-colors text-left"
                              title={f.file}
                            >
                              <Icon name="chevron-right" size={10} className={`transition-transform shrink-0 ${fileHidden ? '' : 'rotate-90'}`} />
                              <Icon name="file" size={12} className="text-[var(--accent)] shrink-0" />
                              <span className="text-[length:var(--app-text-xs)] font-mono truncate flex-1 text-[var(--text-secondary)]">{f.file}</span>
                              <span className="text-[length:var(--app-text-2xs)] text-[var(--text-muted)] shrink-0 tabular-nums">{f.syms.length}</span>
                            </button>
                            {!fileHidden && (
                              <div className="border-t border-[var(--border)]/60">
                                {f.syms.slice(0, 60).map((s, i) => (
                                  <button
                                    key={`${s.file}:${s.line}:${i}`}
                                    onClick={() => openSymbol(s)}
                                    className="list-item w-full text-left pl-9 pr-2.5 py-1.5 flex items-start gap-2 group"
                                  >
                                    <span
                                      className="mt-0.5 shrink-0 text-[length:var(--app-text-micro)] font-semibold uppercase tracking-wide px-1.5 py-0.5 rounded"
                                      style={{
                                        color: symbolKindColor(s.kind),
                                        background: `color-mix(in srgb, ${symbolKindColor(s.kind)} 14%, transparent)`,
                                      }}
                                    >
                                      {s.kind}
                                    </span>
                                    <span className="min-w-0 flex-1">
                                      <span className="block text-[length:var(--app-text-sm)] font-medium text-[var(--text-primary)] truncate">
                                        {s.name}
                                        {s.parent && <span className="text-[var(--text-muted)] font-normal"> · {s.parent}</span>}
                                      </span>
                                      <span className="block text-[length:var(--app-text-2xs)] text-[var(--text-muted)] font-mono truncate mt-0.5">
                                        L{s.line}
                                      </span>
                                    </span>
                                  </button>
                                ))}
                                {f.syms.length > 60 && (
                                  <div className="text-center text-[length:var(--app-text-2xs)] text-[var(--text-muted)] py-1.5">
                                    {t('home.symbolMore', { count: f.syms.length - 60 })}
                                  </div>
                                )}
                              </div>
                            )}
                          </div>
                        )
                      })}
                      {d.files.length > 120 && (
                        <div className="text-center text-[length:var(--app-text-2xs)] text-[var(--text-muted)] py-1.5">
                          {t('home.symbolMore', { count: d.files.length - 120 })}
                        </div>
                      )}
                    </div>
                  )}
                </div>
              )
            })}
            {dirs.length > 120 && (
              <div className="text-center text-[length:var(--app-text-xs)] text-[var(--text-muted)] py-2">
                {t('home.symbolTruncated', { count: dirs.length })}
              </div>
            )}
          </div>
        )}
      </div>
    </div>
  )
}
