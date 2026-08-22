// @ui-states: loading, empty, retry
import { useCallback, useEffect, useMemo, useState, type ReactNode } from 'react'
import { useTranslation } from 'react-i18next'
import { open as shellOpen } from '@tauri-apps/plugin-shell'
import {
  getOhpmLandscapeStatus,
  refreshOhpmLandscape,
  searchOhpmLandscape,
  hotOhpmLandscape,
  byCategoryOhpmLandscape,
  countOhpmLandscape,
  getOhpmLandscapeCategories,
  getOhpmLandscapeRepoUrl,
  type OhpmLandscapeStatus,
  type OhpmPkg,
  type OhpmCategoryStat,
} from '../api/harmonyEnv'

/** 秒级时间戳 → 本地日期时间（如 2026-08-17 14:30） */
function fmtTs(ts: number): string {
  const d = new Date(ts * 1000)
  const pad = (n: number) => String(n).padStart(2, '0')
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`
}

/** 下载量友好显示：≥1万 → x.x万，≥1千 → x.x千 */
function fmtDownloads(n: number): string {
  if (n >= 10000) return `${(n / 10000).toFixed(1)}万`
  if (n >= 1000) return `${(n / 1000).toFixed(1)}千`
  return String(n)
}

/** ohpm 官网包详情页 URL（`/` 编码为 `%2F`，与官网 landscape 一致） */
function ohpmDetailUrl(packageName: string): string {
  return `https://ohpm.openharmony.cn/#/cn/detail/${packageName.replace(/\//g, '%2F')}`
}

/** 高亮文本中的搜索词（大小写不敏感，安全转义：仅按字符切分不注入 HTML） */
function highlight(text: string, q: string): ReactNode {
  if (!q) return text
  const lower = text.toLowerCase()
  const ql = q.toLowerCase()
  const parts: ReactNode[] = []
  let i = 0
  let idx = lower.indexOf(ql)
  let k = 0
  while (idx >= 0 && k < 20) {
    if (idx > i) parts.push(text.slice(i, idx))
    parts.push(
      <mark key={k} className="rounded bg-[var(--accent-soft)] px-0.5 text-[var(--accent)]">
        {text.slice(idx, idx + q.length)}
      </mark>,
    )
    i = idx + q.length
    idx = lower.indexOf(ql, i)
    k++
  }
  if (i < text.length) parts.push(text.slice(i))
  return parts
}

/** 每页条数（后端单次上限 100） */
const PAGE_SIZE = 30

export default function OhpmPage() {
  const { t } = useTranslation()
  // 缓存状态与刷新
  const [status, setStatus] = useState<OhpmLandscapeStatus | null>(null)
  const [busy, setBusy] = useState(false)
  const [msg, setMsg] = useState<string | null>(null)
  // 分类树与筛选（一级 cat / 二级 sub）
  const [cats, setCats] = useState<OhpmCategoryStat[]>([])
  const [cat, setCat] = useState<string | null>(null)
  const [sub, setSub] = useState<string | null>(null)
  // 搜索（搜索词非空时优先于分类展示结果）
  const [query, setQuery] = useState('')
  const [searching, setSearching] = useState(false)
  // 排序：down=下载量（默认）/ likes=最受欢迎 / popularity=最流行 / latest=最新发布
  const [sort, setSort] = useState<'down' | 'likes' | 'popularity' | 'latest'>('down')
  // 列表
  const [pkgs, setPkgs] = useState<OhpmPkg[] | null>(null)
  const [loading, setLoading] = useState(false)
  const [listMsg, setListMsg] = useState<string | null>(null)
  const [hasMore, setHasMore] = useState(false)
  // 正在获取仓库地址并打开的包名（避免重复点击）
  const [opening, setOpening] = useState<string | null>(null)
  // 展示模式：more = 加载更多（累积追加），pages = 页码（按页替换）
  const [mode, setMode] = useState<'more' | 'pages'>('more')
  const [page, setPage] = useState(1)
  const [total, setTotal] = useState<number | null>(null)

  /** 分类树（随缓存刷新重建） */
  const loadCats = useCallback(async () => {
    try {
      const tree = await getOhpmLandscapeCategories()
      setCats(tree.categories)
    } catch {
      setCats([])
    }
  }, [])

  /** 拉取当前范围列表：搜索词 > 选中分类（可带二级） > 热门；sort 控制排序；offset 用于分页，append 为 true 时追加到现有列表 */
  const loadList = useCallback(
    async (
      catName: string | null,
      subName: string | null,
      q: string,
      order: string,
      limit: number,
      offset: number,
      append: boolean,
    ) => {
      setLoading(true)
      setListMsg(null)
      let count = 0
      try {
        const qTrim = q.trim()
        let hits: OhpmPkg[]
        if (qTrim) {
          hits = await searchOhpmLandscape(qTrim, order, limit, offset)
        } else if (catName) {
          hits = await byCategoryOhpmLandscape(catName, subName, order, limit, offset)
        } else {
          hits = await hotOhpmLandscape(order, limit, offset)
        }
        count = hits.length
        setPkgs((prev) => {
          if (!append || !prev) return hits
          const seen = new Set(prev.map((p) => p.package_name))
          return [...prev, ...hits.filter((p) => !seen.has(p.package_name))]
        })
        // 非追加加载时刷新当前范围总数（页码模式与进度显示需要，条件与列表一致）
        if (!append) {
          void countOhpmLandscape(qTrim || undefined, catName, subName)
            .then(setTotal)
            .catch(() => setTotal(null))
        }
        // 返回满页说明还有更多，可继续加载下一页
        setHasMore(hits.length >= limit)
      } catch (e) {
        setListMsg(t('ohpm.loadFailed', { err: String(e) }))
        if (!append) setPkgs([])
        setHasMore(false)
      } finally {
        setLoading(false)
      }
      return count
    },
    [t],
  )

  // 首轮：缓存状态 + 分类树；有缓存直接展示热门
  useEffect(() => {
    void (async () => {
      try {
        const st = await getOhpmLandscapeStatus()
        setStatus(st)
        if (st.total > 0) {
          void loadCats()
          void loadList(null, null, '', sort, PAGE_SIZE, 0, false)
        }
      } catch {
        setStatus(null)
      }
    })()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [loadCats, loadList])

  /** 全量刷新：拉最新数据后重建状态 / 分类树 / 列表 */
  const handleRefresh = async () => {
    setBusy(true)
    setMsg(null)
    try {
      const rep = await refreshOhpmLandscape()
      const st = await getOhpmLandscapeStatus().catch(() => null)
      setStatus(st ?? { total: rep.total, updated_at: rep.updated_at, categories: 0 })
      setCat(null)
      setSub(null)
      setQuery('')
      setPage(1)
      void loadCats()
      void loadList(null, null, '', sort, PAGE_SIZE, 0, false)
      setMsg(t('ohpm.synced', { count: rep.total }))
    } catch (e) {
      setMsg(t('ohpm.syncFailed', { err: String(e) }))
    } finally {
      setBusy(false)
    }
  }

  /** 选中一级分类（再次点击同一分类视为取消，回到热门） */
  const pickCat = (name: string | null) => {
    const next = cat === name ? null : name
    setQuery('')
    setSub(null)
    setCat(next)
    setPage(1)
    void loadList(next, null, '', sort, PAGE_SIZE, 0, false)
  }

  /** 选中二级分类（再次点击取消，回到该一级分类全部） */
  const pickSub = (name: string | null) => {
    const next = sub === name ? null : name
    setSub(next)
    setPage(1)
    void loadList(cat, next, '', sort, PAGE_SIZE, 0, false)
  }

  const handleSearch = async () => {
    const q = query.trim()
    if (!q) return
    setSearching(true)
    setListMsg(null)
    setPage(1)
    await loadList(cat, sub, q, sort, PAGE_SIZE, 0, false)
    setSearching(false)
  }

  const handleReset = () => {
    setQuery('')
    setPage(1)
    void loadList(cat, sub, '', sort, PAGE_SIZE, 0, false)
  }

  /** 加载下一页：offset 为当前已显示条数 */
  const handleMore = () => {
    void loadList(cat, sub, query, sort, PAGE_SIZE, pkgs?.length ?? 0, true)
  }

  /** 打开条目：优先查 registry 元数据跳转仓库主页，无仓库则回退官网详情页 */
  const openPkg = async (p: OhpmPkg) => {
    if (opening) return
    setOpening(p.package_name)
    try {
      const repo = await getOhpmLandscapeRepoUrl(p.package_name)
      if (repo) {
        await shellOpen(repo)
      } else {
        await shellOpen(ohpmDetailUrl(p.package_name))
      }
    } catch {
      await shellOpen(ohpmDetailUrl(p.package_name))
    } finally {
      setOpening(null)
    }
  }

  /** 切换列表展示模式：页码模式下重拉第一页；切回「加载更多」保留已加载数据可继续追加 */
  const switchMode = (m: 'more' | 'pages') => {
    if (m === mode) return
    setMode(m)
    if (m === 'pages') {
      setPage(1)
      void loadList(cat, sub, query, sort, PAGE_SIZE, 0, false)
    }
  }

  /** 切换排序：重拉第一页（保持当前分类/搜索词/模式） */
  const pickSort = (s: 'down' | 'likes' | 'popularity' | 'latest') => {
    if (s === sort) return
    setSort(s)
    setPage(1)
    void loadList(cat, sub, query, s, PAGE_SIZE, 0, false)
  }

  /** 页码模式翻页：total 未知时仅做边界钳制 */
  const goPage = (p: number) => {
    const max = total != null ? Math.max(1, Math.ceil(total / PAGE_SIZE)) : Number.MAX_SAFE_INTEGER
    if (p < 1 || p > max) return
    setPage(p)
    void loadList(cat, sub, query, sort, PAGE_SIZE, (p - 1) * PAGE_SIZE, false)
  }

  /** 页码模式总页数 */
  const totalPages = useMemo(
    () => (total != null ? Math.max(1, Math.ceil(total / PAGE_SIZE)) : null),
    [total],
  )

  /** 选中一级分类的二级子分类列表 */
  const subCats = useMemo(() => {
    if (!cat) return []
    return cats.find((c) => c.name_cn === cat || c.name_en === cat)?.children ?? []
  }, [cats, cat])

  /** 当前列表范围的标题 */
  const scopeTitle = useMemo(() => {
    const q = query.trim()
    if (q) return t('ohpm.searchScope', { q })
    if (cat) return sub ? t('ohpm.catSubScope', { cat, sub }) : t('ohpm.catScope', { cat })
    return t('ohpm.hotTitle')
  }, [query, cat, sub, t])

  const notReady = status !== null && status.total === 0

  return (
    <div>
      {/* 头部：标题 + 刷新 */}
      <div className="flex items-start justify-between gap-3 mb-4 flex-wrap">
        <div className="min-w-0">
          <h2 className="text-lg font-semibold text-[var(--text-primary)]">{t('ohpm.title')}</h2>
          <p className="text-xs text-[var(--text-muted)] mt-1 leading-relaxed">{t('ohpm.subtitle')}</p>
        </div>
        <button
          onClick={() => void handleRefresh()}
          disabled={busy}
          className="px-3 h-8 rounded-lg btn-primary text-xs font-medium shrink-0 disabled:opacity-50 transition-colors"
        >
          {busy ? t('ohpm.syncing') : t('ohpm.refresh')}
        </button>
      </div>

      {/* 缓存状态条 */}
      <div className="modern-card rounded-lg p-4 mb-3">
        <div className="flex items-center gap-3 flex-wrap">
          <div
            className={`w-3 h-3 rounded-full ${status && status.total > 0 ? 'bg-[var(--success)]' : 'bg-[var(--muted)]'} shrink-0`}
          />
          <span className="text-xs text-[var(--text-secondary)]">
            {status && status.total > 0
              ? t('ohpm.ready', {
                  count: status.total,
                  categories: status.categories,
                  time: status.updated_at ? fmtTs(status.updated_at) : '—',
                })
              : t('ohpm.notReady')}
          </span>
        </div>
        {msg && <p className="text-xs text-[var(--text-secondary)] mt-2 break-all">{msg}</p>}
      </div>

      {/* 未缓存：引导刷新（列表区不渲染，避免空分类） */}
      {notReady && (
        <div className="modern-card rounded-lg p-8 mb-3 text-center">
          <p className="text-sm text-[var(--text-secondary)]">{t('ohpm.notReady')}</p>
          <button
            onClick={() => void handleRefresh()}
            disabled={busy}
            className="mt-3 px-4 h-8 rounded-lg btn-primary text-xs font-medium disabled:opacity-50 transition-colors"
          >
            {busy ? t('ohpm.syncing') : t('ohpm.refresh')}
          </button>
        </div>
      )}

      {status && status.total > 0 && (
        <>
          {/* 分类筛选：一级 chips + 选中后的二级 chips */}
          <div className="modern-card rounded-lg p-4 mb-3">
            <div className="flex items-center gap-2 mb-2">
              <span className="text-xs font-medium text-[var(--text-secondary)]">{t('ohpm.catTitle')}</span>
              {cat && subCats.length > 0 && (
                <span className="text-[10px] text-[var(--text-muted)]">{t('ohpm.catHint')}</span>
              )}
            </div>
            <div className="flex flex-wrap gap-1.5">
              <button
                onClick={() => pickCat(null)}
                className={`h-7 px-2.5 rounded-full text-xs transition-colors border ${
                  cat === null
                    ? 'border-[var(--accent)] bg-[var(--accent-soft)] text-[var(--accent)]'
                    : 'border-[var(--border)] text-[var(--text-secondary)] hover:border-[var(--accent)]/50'
                }`}
              >
                {t('ohpm.catAll')}
              </button>
              {cats.map((c) => (
                <button
                  key={c.name_cn}
                  onClick={() => pickCat(c.name_cn)}
                  title={c.name_en || c.name_cn}
                  className={`h-7 px-2.5 rounded-full text-xs transition-colors border ${
                    cat === c.name_cn
                      ? 'border-[var(--accent)] bg-[var(--accent-soft)] text-[var(--accent)]'
                      : 'border-[var(--border)] text-[var(--text-secondary)] hover:border-[var(--accent)]/50'
                  }`}
                >
                  {c.name_cn || c.name_en} <span className="opacity-60">({c.count})</span>
                </button>
              ))}
            </div>
            {cat && subCats.length > 0 && (
              <div className="flex flex-wrap gap-1.5 mt-2.5 pt-2.5 border-t border-[var(--border)]">
                <button
                  onClick={() => pickSub(null)}
                  className={`h-6 px-2 rounded-full text-[11px] transition-colors border ${
                    sub === null
                      ? 'border-[var(--accent)] bg-[var(--accent-soft)] text-[var(--accent)]'
                      : 'border-[var(--border)] text-[var(--text-muted)] hover:border-[var(--accent)]/50'
                  }`}
                >
                  {t('ohpm.subAll')}
                </button>
                {subCats.map((c) => (
                  <button
                    key={c.name_cn}
                    onClick={() => pickSub(c.name_cn)}
                    title={c.name_en || c.name_cn}
                    className={`h-6 px-2 rounded-full text-[11px] transition-colors border ${
                      sub === c.name_cn
                        ? 'border-[var(--accent)] bg-[var(--accent-soft)] text-[var(--accent)]'
                        : 'border-[var(--border)] text-[var(--text-muted)] hover:border-[var(--accent)]/50'
                    }`}
                  >
                    {c.name_cn || c.name_en} <span className="opacity-60">({c.count})</span>
                  </button>
                ))}
              </div>
            )}
          </div>

          {/* 搜索栏 */}
          <div className="modern-card rounded-lg p-4 mb-3">
            <div className="flex gap-2">
              <input
                value={query}
                onChange={(e) => setQuery(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === 'Enter') void handleSearch()
                  if (e.key === 'Escape') handleReset()
                }}
                placeholder={t('ohpm.searchPlaceholder')}
                className="flex-1 h-8 px-3 rounded-lg modern-card border-[var(--border)] text-xs outline-none focus:border-[var(--accent)]"
              />
              <button
                onClick={() => void handleSearch()}
                disabled={searching || !query.trim()}
                className="px-3 h-8 rounded-lg modern-card text-[var(--text-primary)] text-xs hover:border-[var(--accent)]/50 disabled:opacity-50 transition-colors"
              >
                {searching ? t('ohpm.searching') : t('ohpm.search')}
              </button>
              {query.trim() && (
                <button
                  onClick={handleReset}
                  className="px-3 h-8 rounded-lg modern-card text-[var(--text-muted)] text-xs hover:text-[var(--text-primary)] transition-colors"
                >
                  {t('ohpm.reset')}
                </button>
              )}
            </div>
            <p className="text-[10px] text-[var(--text-muted)] mt-2">{t('ohpm.openHint')}</p>
          </div>

          {/* 结果列表 */}
          <div className="modern-card rounded-lg p-4">
            <div className="flex items-center gap-2 mb-2 flex-wrap">
              <span className="text-xs font-medium text-[var(--text-secondary)]">{scopeTitle}</span>
              {pkgs && pkgs.length > 0 && (
                <span className="text-[10px] text-[var(--text-muted)]">
                  {mode === 'pages' && total != null
                    ? `(${total})`
                    : `(${pkgs.length}${total != null ? ` / ${total}` : ''})`}
                </span>
              )}
              <div className="ml-auto flex items-center gap-2 flex-wrap">
                {/* 排序切换：下载量 / 最受欢迎 / 最流行 / 最新发布 */}
                <div className="flex rounded-lg border border-[var(--border)] overflow-hidden shrink-0">
                  <button
                    onClick={() => pickSort('down')}
                    title={t('ohpm.sortHint')}
                    className={`h-6 px-2.5 text-[11px] transition-colors ${sort === 'down' ? 'bg-[var(--accent-soft)] text-[var(--accent)]' : 'text-[var(--text-muted)] hover:text-[var(--text-primary)]'}`}
                  >
                    {t('ohpm.sortDown')}
                  </button>
                  <button
                    onClick={() => pickSort('likes')}
                    className={`h-6 px-2.5 text-[11px] transition-colors border-l border-[var(--border)] ${sort === 'likes' ? 'bg-[var(--accent-soft)] text-[var(--accent)]' : 'text-[var(--text-muted)] hover:text-[var(--text-primary)]'}`}
                  >
                    {t('ohpm.sortLikes')}
                  </button>
                  <button
                    onClick={() => pickSort('popularity')}
                    className={`h-6 px-2.5 text-[11px] transition-colors border-l border-[var(--border)] ${sort === 'popularity' ? 'bg-[var(--accent-soft)] text-[var(--accent)]' : 'text-[var(--text-muted)] hover:text-[var(--text-primary)]'}`}
                  >
                    {t('ohpm.sortPopularity')}
                  </button>
                  <button
                    onClick={() => pickSort('latest')}
                    className={`h-6 px-2.5 text-[11px] transition-colors border-l border-[var(--border)] ${sort === 'latest' ? 'bg-[var(--accent-soft)] text-[var(--accent)]' : 'text-[var(--text-muted)] hover:text-[var(--text-primary)]'}`}
                  >
                    {t('ohpm.sortLatest')}
                  </button>
                </div>
                <div className="flex rounded-lg border border-[var(--border)] overflow-hidden shrink-0">
                <button
                  onClick={() => switchMode('more')}
                  className={`h-6 px-2.5 text-[11px] transition-colors ${
                    mode === 'more'
                      ? 'bg-[var(--accent-soft)] text-[var(--accent)]'
                      : 'text-[var(--text-muted)] hover:text-[var(--text-primary)]'
                  }`}
                >
                  {t('ohpm.modeMore')}
                </button>
                <button
                  onClick={() => switchMode('pages')}
                  className={`h-6 px-2.5 text-[11px] transition-colors border-l border-[var(--border)] ${
                    mode === 'pages'
                      ? 'bg-[var(--accent-soft)] text-[var(--accent)]'
                      : 'text-[var(--text-muted)] hover:text-[var(--text-primary)]'
                  }`}
                >
                  {t('ohpm.modePages')}
                </button>
                </div>
              </div>
            </div>
            {listMsg && <p className="text-xs text-[var(--text-secondary)] mb-2 break-all">{listMsg}</p>}
            {loading && pkgs === null ? (
              <p className="text-xs text-[var(--text-muted)] py-6 text-center">{t('ohpm.loading')}</p>
            ) : pkgs && pkgs.length === 0 ? (
              <p className="text-xs text-[var(--text-muted)] py-6 text-center">
                {query.trim() ? t('ohpm.noHits') : t('ohpm.emptyCat')}
              </p>
            ) : (
              pkgs && (
                <>
                  <div className="rounded-lg border border-[var(--border)] divide-y divide-[var(--border)] overflow-hidden">
                    {pkgs.map((p) => {
                      const qTrim = query.trim()
                      const catPath = (p.level1_cn || p.level1_en) + (p.level2_cn ? ` / ${p.level2_cn}` : '')
                      return (
                        <button
                          key={p.package_name}
                          onClick={() => void openPkg(p)}
                          title={t('ohpm.openRepo')}
                          className="w-full text-left px-3 py-2 hover:bg-[var(--bg-hover)] transition-colors"
                        >
                          <div className="flex items-center gap-2">
                            <span className="text-xs font-medium text-[var(--text-primary)] break-all min-w-0">
                              {highlight(p.package_name, qTrim)}
                            </span>
                            <span className="shrink-0 text-[10px] text-[var(--text-muted)] font-mono">v{p.version}</span>
                            {p.down_count_60d > 0 && (
                              <span className="shrink-0 text-[10px] text-[var(--accent)]">⬇ {fmtDownloads(p.down_count_60d)}/60天</span>
                            )}
                            {p.likes > 0 && (
                              <span className="shrink-0 text-[10px] text-[var(--text-muted)]">❤ {p.likes}</span>
                            )}
                            {p.author_name && (
                              <span className="shrink-0 text-[10px] text-[var(--text-muted)] truncate">· {p.author_name}</span>
                            )}
                            {p.license && (
                              <span
                                className="shrink-0 rounded px-1 py-0.5 text-[9px] font-mono bg-[var(--bg-hover)] text-[var(--text-secondary)]"
                                title={`License: ${p.license}`}
                              >
                                {p.license}
                              </span>
                            )}
                            {opening === p.package_name && (
                              <span className="shrink-0 text-[10px] text-[var(--accent)]">{t('ohpm.opening')}</span>
                            )}
                          </div>
                          <div className="text-[10px] text-[var(--text-muted)] break-words whitespace-pre-line mt-0.5">
                            {catPath}
                            {p.description ? <> · {highlight(p.description, qTrim)}</> : ' · —'}
                          </div>
                        </button>
                      )
                    })}
                  </div>
                  {mode === 'pages' ? (
                    <div className="mt-3 flex items-center justify-center gap-2">
                      <button
                        onClick={() => goPage(page - 1)}
                        disabled={loading || page <= 1}
                        className="h-7 px-3 rounded-lg modern-card text-xs text-[var(--text-secondary)] hover:border-[var(--accent)]/50 disabled:opacity-40 transition-colors"
                      >
                        {t('ohpm.prevPage')}
                      </button>
                      <span className="text-xs text-[var(--text-muted)]">
                        {totalPages != null
                          ? t('ohpm.pageInfo', { page, totalPages })
                          : t('ohpm.pageInfoOnly', { page })}
                      </span>
                      <button
                        onClick={() => goPage(page + 1)}
                        disabled={loading || (totalPages != null && page >= totalPages)}
                        className="h-7 px-3 rounded-lg modern-card text-xs text-[var(--text-secondary)] hover:border-[var(--accent)]/50 disabled:opacity-40 transition-colors"
                      >
                        {t('ohpm.nextPage')}
                      </button>
                    </div>
                  ) : hasMore ? (
                    <button
                      onClick={handleMore}
                      disabled={loading}
                      className="mt-3 w-full h-8 rounded-lg modern-card text-xs text-[var(--text-secondary)] hover:border-[var(--accent)]/50 disabled:opacity-50 transition-colors"
                    >
                      {loading
                        ? t('ohpm.loadMoreLoading')
                        : total != null
                          ? t('ohpm.loadMoreWithCount', { loaded: pkgs.length, total })
                          : t('ohpm.loadMore')}
                    </button>
                  ) : total != null ? (
                    <p className="mt-3 text-center text-[10px] text-[var(--text-muted)]">
                      {t('ohpm.allLoaded', { total })}
                    </p>
                  ) : null}
                </>
              )
            )}
          </div>
        </>
      )}
    </div>
  )
}
