import { useState, useEffect, useCallback, useRef } from 'react'
import { useTranslation } from 'react-i18next'
import {
  apiKbStats,
  apiKbFilters,
  apiDocsList,
  apiDetailsList,
  apiDetailGet,
  apiDocAdd,
  apiDocDelete,
  apiDetailUpsert,
  apiDetailDelete,
  apiKbClear,
  apiKbRefreshDocs,
  apiKbRefreshDetails,
  apiKbEmbedStatus,
  apiKbEmbedIndex,
  onDocsProgress,
  onDetailsProgress,
  onEmbedProgress,
  onEmbedDone,
  type ApiKbStats,
  type KbFilters,
  type ApiEntry,
  type DocsPage,
  type DetailListItem,
  type DetailsPage,
  type ApiDetailFull,
  type DocInput,
  type DetailInput,
  type RefreshProgress,
  type EmbedStatus,
  type EmbedDonePayload,
} from '../api/apiKnowledge'

type Tab = 'docs' | 'details'

const PAGE_SIZE_OPTIONS = [20, 50, 100, 200]

function formatTime(ts: number | null): string {
  if (!ts) return '-'
  return new Date(ts * 1000).toLocaleString()
}

function changeTypeColor(ct: string): string {
  switch (ct) {
    case 'added': return 'bg-[var(--success)]/15 text-[var(--success)] border-[var(--success)]/40'
    case 'removed': return 'bg-[var(--danger)]/15 text-[var(--danger)] border-[var(--danger)]/40'
    case 'modified': return 'bg-[#f59e0b]/15 text-[#f59e0b] border-[#f59e0b]/40'
    case 'deprecated': return 'bg-[var(--warning)]/15 text-[var(--warning)] border-[var(--warning)]/40'
    default: return 'bg-[var(--bg-card)] text-[var(--text-secondary)] border-[var(--border)]'
  }
}

export default function ApiKnowledgePage() {
  const { t } = useTranslation()
  const [tab, setTab] = useState<Tab>('docs')

  const [stats, setStats] = useState<ApiKbStats | null>(null)
  const [filters, setFilters] = useState<KbFilters | null>(null)
  const [embedStatus, setEmbedStatus] = useState<EmbedStatus | null>(null)
  const [embedProgress, setEmbedProgress] = useState<RefreshProgress | null>(null)

  const [loading, setLoading] = useState(false)
  const [docsPage, setDocsPage] = useState<DocsPage | null>(null)
  const [detailsPage, setDetailsPage] = useState<DetailsPage | null>(null)

  const [page, setPage] = useState(1)
  const [pageSize, setPageSize] = useState(50)

  const [keyword, setKeyword] = useState('')
  const [searchInput, setSearchInput] = useState('')
  const [kit, setKit] = useState('')
  const [version, setVersion] = useState('')
  const [changeType, setChangeType] = useState('')
  const [apiLevel, setApiLevel] = useState('')
  const [moduleFilter, setModuleFilter] = useState('')
  const [includeDeprecated, setIncludeDeprecated] = useState(true)

  const [detail, setDetail] = useState<ApiDetailFull | null>(null)
  const [detailLoading, setDetailLoading] = useState(false)

  const [showDocForm, setShowDocForm] = useState(false)
  const [showDetailForm, setShowDetailForm] = useState(false)
  const [editingDetail, setEditingDetail] = useState<DetailListItem | null>(null)

  const [refreshing, setRefreshing] = useState<'docs' | 'details' | null>(null)
  const [progress, setProgress] = useState<RefreshProgress | null>(null)
  const progressUnlisten = useRef<(() => void) | null>(null)

  const [error, setError] = useState<string | null>(null)

  const reload = useCallback(async () => {
    setLoading(true)
    setError(null)
    try {
      const [s, f] = await Promise.all([apiKbStats(), apiKbFilters()])
      setStats(s)
      setFilters(f)
    } catch (e) {
      setError(String(e))
    }
    // 语义索引状态独立获取，失败不阻塞列表加载
    try {
      setEmbedStatus(await apiKbEmbedStatus())
    } catch {
      /* 忽略：未启用语义检索时静默 */
    }
    try {
      if (tab === 'docs') {
        const q: Parameters<typeof apiDocsList>[0] = {
          page, pageSize,
          keyword: keyword || undefined,
          kit: kit || undefined,
          module: moduleFilter || undefined,
          versionLabel: version || undefined,
          changeType: changeType || undefined,
          apiLevel: apiLevel ? Number(apiLevel) : undefined,
        }
        setDocsPage(await apiDocsList(q))
      } else {
        const q: Parameters<typeof apiDetailsList>[0] = {
          page, pageSize,
          keyword: keyword || undefined,
          kit: kit || undefined,
          module: moduleFilter || undefined,
          includeDeprecated,
        }
        setDetailsPage(await apiDetailsList(q))
      }
    } catch (e) {
      setError(String(e))
    } finally {
      setLoading(false)
    }
  }, [tab, page, pageSize, keyword, kit, version, changeType, apiLevel, moduleFilter, includeDeprecated])

  useEffect(() => { reload() }, [reload])

  // 建索引完成：刷新状态展示（成功/失败均以事件 payload 为准）
  const handleEmbedDone = useCallback((p: EmbedDonePayload) => {
    setEmbedProgress(null)
    if (!p.ok) {
      setError(p.error ?? t('apiKb.embedFailed'))
    }
    apiKbEmbedStatus().then(setEmbedStatus).catch(() => {})
  }, [t])

  // 语义索引进度/完成事件（页面生命周期内常驻监听，后台任务跨刷新周期持续）
  useEffect(() => {
    let unlistenP: (() => void) | null = null
    let unlistenD: (() => void) | null = null
    let cancelled = false
    Promise.all([onEmbedProgress(setEmbedProgress), onEmbedDone(handleEmbedDone)]).then(
      ([a, b]) => {
        if (cancelled) {
          a()
          b()
          return
        }
        unlistenP = a
        unlistenD = b
      },
    )
    return () => {
      cancelled = true
      unlistenP?.()
      unlistenD?.()
    }
  }, [handleEmbedDone])

  const startEmbed = async () => {
    setError(null)
    try {
      await apiKbEmbedIndex()
      setEmbedProgress({ phase: 'checking', current: 0, total: 0, message: t('apiKb.embedStarting') })
      setEmbedStatus((s) => (s ? { ...s, running: true } : s))
    } catch (e) {
      setError(String(e))
    }
  }

  useEffect(() => {
    setPage(1)
  }, [tab, keyword, kit, version, changeType, apiLevel, moduleFilter, includeDeprecated, pageSize])

  const handleSearch = () => {
    setKeyword(searchInput.trim())
    setPage(1)
  }

  const handleReset = () => {
    setSearchInput('')
    setKeyword('')
    setKit('')
    setVersion('')
    setChangeType('')
    setApiLevel('')
    setModuleFilter('')
    setIncludeDeprecated(true)
    setPage(1)
  }

  const openDetail = async (item: DetailListItem) => {
    setDetailLoading(true)
    setDetail(null)
    try {
      setDetail(await apiDetailGet(item.slug))
    } catch (e) {
      setError(String(e))
    } finally {
      setDetailLoading(false)
    }
  }

  const handleDeleteDoc = async (entry: ApiEntry) => {
    if (!confirm(t('apiKb.confirmDeleteDoc', { name: entry.declaration.slice(0, 60) }))) return
    try {
      if (entry.id == null) {
        alert(t('apiKb.cannotDeleteNoId'))
        return
      }
      await apiDocDelete(entry.id)
      reload()
    } catch (e) {
      alert(String(e))
    }
  }

  const handleDeleteDetail = async (item: DetailListItem) => {
    if (!confirm(t('apiKb.confirmDeleteDetail', { name: item.module }))) return
    try {
      await apiDetailDelete(item.slug)
      reload()
    } catch (e) {
      alert(String(e))
    }
  }

  const handleClearAll = async () => {
    if (!confirm(t('apiKb.confirmClearAll'))) return
    if (!confirm(t('apiKb.confirmClearAll2'))) return
    try {
      await apiKbClear()
      reload()
    } catch (e) {
      alert(String(e))
    }
  }

  const startRefresh = async (kind: 'docs' | 'details') => {
    setRefreshing(kind)
    setProgress({ phase: 'starting', current: 0, total: 0, message: t('apiKb.refreshStarting') })
    setError(null)

    if (progressUnlisten.current) progressUnlisten.current()
    const unlisten = kind === 'docs'
      ? await onDocsProgress((p) => setProgress(p))
      : await onDetailsProgress((p) => setProgress(p))
    progressUnlisten.current = unlisten

    try {
      if (kind === 'docs') {
        const r = await apiKbRefreshDocs()
        if (r.errors.length > 0) {
          setError(t('apiKb.refreshErrors', { n: r.errors.length }))
        }
      } else {
        const r = await apiKbRefreshDetails()
        if (r.errors.length > 0) {
          setError(t('apiKb.refreshErrors', { n: r.errors.length }))
        }
      }
      await reload()
    } catch (e) {
      setError(String(e))
    } finally {
      setRefreshing(null)
      if (progressUnlisten.current) {
        progressUnlisten.current()
        progressUnlisten.current = null
      }
    }
  }

  useEffect(() => {
    return () => {
      if (progressUnlisten.current) progressUnlisten.current()
    }
  }, [])

  const totalPages = tab === 'docs'
    ? Math.max(1, Math.ceil((docsPage?.total ?? 0) / pageSize))
    : Math.max(1, Math.ceil((detailsPage?.total ?? 0) / pageSize))
  // 已有数据时翻页/刷新用遮罩提示（保留旧数据，避免页面高度塌陷导致滚动跳顶）
  const hasData = tab === 'docs' ? docsPage != null : detailsPage != null

  const kits = tab === 'docs' ? (filters?.kits ?? []) : (filters?.detail_kits ?? [])

  return (
    <div className="space-y-5">
      {/* Header */}
      <div className="flex items-start justify-between gap-4 flex-wrap">
        <div>
          <h2 className="text-xl font-semibold">{t('apiKb.title')}</h2>
          <p className="text-xs text-[var(--text-secondary)] mt-1">{t('apiKb.desc')}</p>
        </div>
        <div className="flex items-center gap-2 flex-wrap">
          <button
            onClick={() => startRefresh('docs')}
            disabled={refreshing !== null}
            className="h-9 px-3 rounded-[10px] border border-[var(--accent)] text-[var(--accent)] text-[12px] font-medium hover:bg-[var(--accent-soft)] disabled:opacity-40 disabled:cursor-not-allowed transition-colors"
          >
            {refreshing === 'docs' ? t('apiKb.refreshingDocs') : t('apiKb.refreshDocs')}
          </button>
          <button
            onClick={() => startRefresh('details')}
            disabled={refreshing !== null}
            className="h-9 px-3 rounded-[10px] border border-[var(--accent)] text-[var(--accent)] text-[12px] font-medium hover:bg-[var(--accent-soft)] disabled:opacity-40 disabled:cursor-not-allowed transition-colors"
          >
            {refreshing === 'details' ? t('apiKb.refreshingDetails') : t('apiKb.refreshDetails')}
          </button>
          <button
            onClick={handleClearAll}
            disabled={refreshing !== null}
            className="h-9 px-3 rounded-[10px] border border-[var(--danger)]/40 text-[var(--danger)] text-[12px] font-medium hover:bg-[var(--danger)]/10 disabled:opacity-40 disabled:cursor-not-allowed transition-colors"
          >
            {t('apiKb.clearAll')}
          </button>
        </div>
      </div>

      {/* Stats bar */}
      {stats && (
        <div className="grid grid-cols-2 sm:grid-cols-4 lg:grid-cols-6 gap-3">
          <StatCard label={t('apiKb.statDocs')} value={stats.docs_total.toLocaleString()} />
          <StatCard label={t('apiKb.statDetails')} value={stats.details_total.toLocaleString()} />
          <StatCard label={t('apiKb.statMembers')} value={stats.members_total.toLocaleString()} />
          <StatCard label={t('apiKb.statVersions')} value={stats.versions.length.toString()} />
          <StatCard label={t('apiKb.statKits')} value={stats.kits.length.toString()} />
          <StatCard label={t('apiKb.statLastRefresh')} value={formatTime(stats.last_refreshed_at)} small />
        </div>
      )}

      {/* Semantic index */}
      {embedStatus && (
        <div className="modern-card rounded-xl p-3">
          <div className="flex items-center justify-between gap-3 flex-wrap">
            <div className="flex items-center gap-2.5 flex-wrap min-w-0">
              <span className="text-[11px] text-[var(--text-muted)]">🧠 {t('apiKb.embedTitle')}</span>
              <span className={`px-1.5 py-0.5 rounded border text-[10px] ${
                embedStatus.available
                  ? 'bg-[var(--success)]/15 text-[var(--success)] border-[var(--success)]/40'
                  : 'bg-[var(--warning)]/15 text-[var(--warning)] border-[var(--warning)]/40'
              }`}>
                {embedStatus.available ? t('apiKb.embedReady') : t('apiKb.embedUnavailable')}
              </span>
              {embedStatus.model && (
                <span className="text-[10px] text-[var(--text-muted)] font-mono">{embedStatus.model}</span>
              )}
              <span className="text-[11px] text-[var(--text-secondary)] font-mono">
                {embedStatus.indexed}/{embedStatus.total}
              </span>
              {embedStatus.running && (
                <span className="text-[11px] text-[var(--warning)] animate-pulse">{t('apiKb.embedRunning')}</span>
              )}
              {!embedStatus.running && embedStatus.available && embedStatus.indexed < embedStatus.total && (
                <span className="text-[11px] text-[var(--warning)]">{t('apiKb.embedStale')}</span>
              )}
            </div>
            <button
              onClick={startEmbed}
              disabled={!embedStatus.available || embedStatus.running || embedStatus.total === 0}
              className="h-8 px-3 rounded-lg border border-[var(--accent)] text-[var(--accent)] text-[12px] font-medium hover:bg-[var(--accent-soft)] disabled:opacity-40 disabled:cursor-not-allowed transition-colors"
            >
              {embedStatus.running ? t('apiKb.embedBuilding') : t('apiKb.embedBuild')}
            </button>
          </div>
          {embedStatus.running && embedProgress && (
            <div className="mt-2.5 space-y-1">
              <div className="flex items-center justify-between text-[10px] text-[var(--text-muted)]">
                <span className="truncate">{embedProgress.message}</span>
                <span className="font-mono shrink-0 ml-2">
                  {embedProgress.total > 0 ? `${embedProgress.current}/${embedProgress.total}` : ''}
                </span>
              </div>
              <div className="h-1.5 bg-[var(--bg-card)] rounded-full overflow-hidden">
                <div
                  className="h-full bg-[var(--accent)] transition-all duration-300"
                  style={{
                    width: embedProgress.total > 0
                      ? `${Math.min(100, (embedProgress.current / embedProgress.total) * 100)}%`
                      : '4%',
                  }}
                />
              </div>
            </div>
          )}
        </div>
      )}

      {/* Version distribution */}
      {stats && stats.versions.length > 0 && (
        <div className="modern-card rounded-xl p-3">
          <div className="text-[11px] text-[var(--text-muted)] mb-2">{t('apiKb.versionDist')}</div>
          <div className="flex flex-wrap gap-2">
            {stats.versions.map((v) => (
              <button
                key={v.version_label}
                onClick={() => { setTab('docs'); setVersion(version === v.version_label ? '' : v.version_label); setPage(1) }}
                className={`px-2.5 py-1 rounded-lg border text-[11px] transition-colors ${
                  version === v.version_label
                    ? 'tab-active border-[var(--accent)]'
                    : 'bg-[var(--bg-card)] text-[var(--text-secondary)] border-[var(--border)] hover:border-[var(--accent)]/40'
                }`}
                title={`API ${v.api_level ?? '-'} · +${v.added} -${v.removed} ~${v.modified}`}
              >
                {v.version_label}
                <span className="ml-1.5 opacity-70">{v.total}</span>
              </button>
            ))}
          </div>
        </div>
      )}

      {/* Progress */}
      {refreshing && progress && (
        <div className="bg-[var(--bg-secondary)] border border-[var(--accent)]/30 rounded-xl p-4 space-y-2">
          <div className="flex items-center justify-between text-[12px]">
            <span className="text-[var(--text-primary)] font-medium">{progress.phase}</span>
            <span className="text-[var(--text-muted)] font-mono">{progress.current}/{progress.total}</span>
          </div>
          <div className="h-2 bg-[var(--bg-card)] rounded-full overflow-hidden">
            <div
              className="h-full bg-[var(--accent)] transition-all duration-300"
              style={{ width: progress.total > 0 ? `${(progress.current / progress.total) * 100}%` : '0%' }}
            />
          </div>
          {progress.message && (
            <p className="text-[11px] text-[var(--text-muted)] truncate">{progress.message}</p>
          )}
        </div>
      )}

      {error && (
        <div className="px-3 py-2 rounded-lg border border-[var(--danger)]/40 bg-[var(--danger)]/10 text-[11px] text-[var(--danger)] break-words">
          {error}
        </div>
      )}

      {/* Tabs */}
      <div className="inline-flex modern-card rounded-lg p-0.5 text-[12px]">
        <button
          onClick={() => setTab('docs')}
          className={`px-4 h-8 rounded-md transition-colors ${
            tab === 'docs' ? 'tab-active' : 'tab-inactive'
          }`}
        >
          {t('apiKb.tabDocs')} ({stats?.docs_total ?? 0})
        </button>
        <button
          onClick={() => setTab('details')}
          className={`px-4 h-8 rounded-md transition-colors ${
            tab === 'details' ? 'tab-active' : 'tab-inactive'
          }`}
        >
          {t('apiKb.tabDetails')} ({stats?.details_total ?? 0})
        </button>
      </div>

      {/* Filters */}
      <div className="modern-card rounded-xl p-3 space-y-3">
        <div className="flex flex-wrap items-center gap-2">
          <div className="flex-1 min-w-[200px] flex items-center gap-1">
            <input
              value={searchInput}
              onChange={(e) => setSearchInput(e.target.value)}
              onKeyDown={(e) => e.key === 'Enter' && handleSearch()}
              placeholder={tab === 'docs' ? t('apiKb.searchDocsPlaceholder') : t('apiKb.searchDetailsPlaceholder')}
              className="flex-1 h-8 px-3 modern-card rounded-lg text-[12px] text-[var(--text-primary)] placeholder:text-[var(--text-muted)] focus:outline-none focus:border-[var(--accent)]"
            />
            <button
              onClick={handleSearch}
              className="h-8 px-3 rounded-lg btn-primary text-[12px]  transition-colors"
            >
              {t('apiKb.search')}
            </button>
          </div>

          <select
            value={kit}
            onChange={(e) => setKit(e.target.value)}
            className="h-8 px-2 modern-card rounded-lg text-[12px] text-[var(--text-primary)] focus:outline-none focus:border-[var(--accent)]"
          >
            <option value="">{t('apiKb.allKits')}</option>
            {kits.map((k) => <option key={k} value={k}>{k}</option>)}
          </select>

          {tab === 'docs' && (
            <>
              <select
                value={version}
                onChange={(e) => setVersion(e.target.value)}
                className="h-8 px-2 modern-card rounded-lg text-[12px] text-[var(--text-primary)] focus:outline-none focus:border-[var(--accent)]"
              >
                <option value="">{t('apiKb.allVersions')}</option>
                {filters?.versions.map((v) => <option key={v} value={v}>{v}</option>)}
              </select>
              <select
                value={changeType}
                onChange={(e) => setChangeType(e.target.value)}
                className="h-8 px-2 modern-card rounded-lg text-[12px] text-[var(--text-primary)] focus:outline-none focus:border-[var(--accent)]"
              >
                <option value="">{t('apiKb.allChangeTypes')}</option>
                <option value="added">{t('apiKb.ctAdded')}</option>
                <option value="modified">{t('apiKb.ctModified')}</option>
                <option value="removed">{t('apiKb.ctRemoved')}</option>
                <option value="deprecated">{t('apiKb.ctDeprecated')}</option>
              </select>
              <input
                value={apiLevel}
                onChange={(e) => setApiLevel(e.target.value.replace(/\D/g, ''))}
                placeholder={t('apiKb.apiLevel')}
                className="w-20 h-8 px-2 modern-card rounded-lg text-[12px] text-[var(--text-primary)] placeholder:text-[var(--text-muted)] focus:outline-none focus:border-[var(--accent)]"
              />
            </>
          )}

          {tab === 'details' && (
            <label className="flex items-center gap-1.5 text-[12px] text-[var(--text-secondary)] cursor-pointer select-none">
              <input
                type="checkbox"
                checked={includeDeprecated}
                onChange={(e) => setIncludeDeprecated(e.target.checked)}
                className="w-3.5 h-3.5 accent-[var(--accent)]"
              />
              {t('apiKb.includeDeprecated')}
            </label>
          )}

          <input
            value={moduleFilter}
            onChange={(e) => setModuleFilter(e.target.value)}
            placeholder={t('apiKb.moduleFilter')}
            className="w-36 h-8 px-2 modern-card rounded-lg text-[12px] text-[var(--text-primary)] placeholder:text-[var(--text-muted)] focus:outline-none focus:border-[var(--accent)]"
          />

          <button
            onClick={handleReset}
            className="h-8 px-3 rounded-lg border border-[var(--border)] text-[12px] text-[var(--text-secondary)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] transition-colors"
          >
            {t('apiKb.reset')}
          </button>
        </div>
      </div>

      {/* Action buttons */}
      <div className="flex items-center justify-between">
        <div className="text-[12px] text-[var(--text-secondary)]">
          {t('apiKb.showingCount', {
            from: ((page - 1) * pageSize) + 1,
            to: Math.min(page * pageSize, tab === 'docs' ? (docsPage?.total ?? 0) : (detailsPage?.total ?? 0)),
            total: tab === 'docs' ? (docsPage?.total ?? 0) : (detailsPage?.total ?? 0),
          })}
        </div>
        <div className="flex items-center gap-2">
          {tab === 'docs' && (
            <button
              onClick={() => setShowDocForm(true)}
              className="h-8 px-3 rounded-lg btn-primary text-[12px]  transition-colors"
            >
              + {t('apiKb.addDoc')}
            </button>
          )}
          {tab === 'details' && (
            <button
              onClick={() => { setEditingDetail(null); setShowDetailForm(true) }}
              className="h-8 px-3 rounded-lg btn-primary text-[12px]  transition-colors"
            >
              + {t('apiKb.addDetail')}
            </button>
          )}
          <select
            value={pageSize}
            onChange={(e) => { setPageSize(Number(e.target.value)); setPage(1) }}
            className="h-8 px-2 modern-card rounded-lg text-[12px] text-[var(--text-primary)] focus:outline-none focus:border-[var(--accent)]"
          >
            {PAGE_SIZE_OPTIONS.map((n) => <option key={n} value={n}>{n}/page</option>)}
          </select>
        </div>
      </div>

      {/* Table */}
      <div className="modern-card rounded-lg overflow-hidden">
        {loading && !hasData ? (
          <div className="p-8 text-center text-sm text-[var(--text-secondary)]">{t('apiKb.loading')}</div>
        ) : (
          <div className="relative">
            {tab === 'docs' ? (
              <DocsTable items={docsPage?.items ?? []} onDelete={handleDeleteDoc} />
            ) : (
              <DetailsTable
                items={detailsPage?.items ?? []}
                onOpen={openDetail}
                onEdit={(item) => { setEditingDetail(item); setShowDetailForm(true) }}
                onDelete={handleDeleteDetail}
              />
            )}
            {loading && (
              <div className="absolute inset-0 z-10 flex items-center justify-center bg-[var(--bg-secondary)]/70">
                <span className="text-sm text-[var(--text-secondary)]">{t('apiKb.loading')}</span>
              </div>
            )}
          </div>
        )}
      </div>

      {/* Pagination */}
      <div className="flex items-center justify-between">
        <div className="text-[12px] text-[var(--text-muted)]">
          {t('apiKb.pageInfo', { page, total: totalPages })}
        </div>
        <div className="flex items-center gap-2">
          <button
            onClick={() => setPage(1)}
            disabled={page <= 1}
            className="h-8 px-3 rounded-lg border border-[var(--border)] text-[12px] text-[var(--text-secondary)] hover:bg-[var(--bg-hover)] disabled:opacity-40 disabled:cursor-not-allowed transition-colors"
          >
            « {t('apiKb.first')}
          </button>
          <button
            onClick={() => setPage((p) => Math.max(1, p - 1))}
            disabled={page <= 1}
            className="h-8 px-3 rounded-lg border border-[var(--border)] text-[12px] text-[var(--text-secondary)] hover:bg-[var(--bg-hover)] disabled:opacity-40 disabled:cursor-not-allowed transition-colors"
          >
            ← {t('apiKb.prev')}
          </button>
          <span className="text-[12px] text-[var(--text-secondary)] font-mono">{page} / {totalPages}</span>
          <button
            onClick={() => setPage((p) => Math.min(totalPages, p + 1))}
            disabled={page >= totalPages}
            className="h-8 px-3 rounded-lg border border-[var(--border)] text-[12px] text-[var(--text-secondary)] hover:bg-[var(--bg-hover)] disabled:opacity-40 disabled:cursor-not-allowed transition-colors"
          >
            {t('apiKb.next')} →
          </button>
          <button
            onClick={() => setPage(totalPages)}
            disabled={page >= totalPages}
            className="h-8 px-3 rounded-lg border border-[var(--border)] text-[12px] text-[var(--text-secondary)] hover:bg-[var(--bg-hover)] disabled:opacity-40 disabled:cursor-not-allowed transition-colors"
          >
            {t('apiKb.last')} »
          </button>
        </div>
      </div>

      {/* Detail drawer */}
      {detail && (
        <DetailDrawer
          detail={detail}
          loading={detailLoading}
          onClose={() => setDetail(null)}
          onEdit={() => {
            const item: DetailListItem = {
              module: detail.module, slug: detail.slug, title: detail.title, kit: detail.kit,
              since_api_level: detail.since_api_level, deprecated: detail.deprecated,
              has_import: !!detail.import_snippet, has_examples: !!detail.examples,
              member_count: detail.members.length, source_url: detail.source_url,
            }
            setDetail(null)
            setEditingDetail(item)
            setShowDetailForm(true)
          }}
        />
      )}

      {/* Doc add form modal */}
      {showDocForm && (
        <DocFormModal
          onClose={() => setShowDocForm(false)}
          onSaved={() => { setShowDocForm(false); reload() }}
        />
      )}

      {/* Detail add/edit form modal */}
      {showDetailForm && (
        <DetailFormModal
          editing={editingDetail}
          onClose={() => { setShowDetailForm(false); setEditingDetail(null) }}
          onSaved={() => { setShowDetailForm(false); setEditingDetail(null); reload() }}
        />
      )}
    </div>
  )
}

// ───────────────────────── 子组件 ─────────────────────────

function StatCard({ label, value, small }: { label: string; value: string; small?: boolean }) {
  return (
    <div className="modern-card rounded-xl px-3 py-2.5">
      <div className="text-[10px] text-[var(--text-muted)] uppercase tracking-wide">{label}</div>
      <div className={`mt-1 font-semibold text-[var(--text-primary)] ${small ? 'text-[11px]' : 'text-[18px]'} truncate`} title={value}>
        {value}
      </div>
    </div>
  )
}

function DocsTable({ items, onDelete }: { items: ApiEntry[]; onDelete: (e: ApiEntry) => void }) {
  const { t } = useTranslation()
  if (items.length === 0) {
    return <div className="p-8 text-center text-sm text-[var(--text-secondary)]">{t('apiKb.empty')}</div>
  }
  return (
    <div className="overflow-x-auto">
      <table className="w-full text-[12px]">
        <thead className="modern-card-b border-[var(--border)]">
          <tr className="text-left text-[var(--text-muted)]">
            <th className="px-3 py-2 font-medium whitespace-nowrap">{t('apiKb.colKit')}</th>
            <th className="px-3 py-2 font-medium whitespace-nowrap">{t('apiKb.colModule')}</th>
            <th className="px-3 py-2 font-medium whitespace-nowrap">{t('apiKb.colDeclaration')}</th>
            <th className="px-3 py-2 font-medium whitespace-nowrap">{t('apiKb.colChange')}</th>
            <th className="px-3 py-2 font-medium whitespace-nowrap">{t('apiKb.colVersion')}</th>
            <th className="px-3 py-2 font-medium whitespace-nowrap">{t('apiKb.colLevel')}</th>
            <th className="px-3 py-2 font-medium"></th>
          </tr>
        </thead>
        <tbody>
          {items.map((e, i) => (
            <tr key={i} className="border-b border-[var(--border)]/50 hover:bg-[var(--bg-card)]/50">
              <td className="px-3 py-2 text-[var(--text-secondary)] whitespace-nowrap">{e.kit}</td>
              <td className="px-3 py-2 text-[var(--text-secondary)] whitespace-nowrap font-mono text-[11px]">{e.module ?? '-'}</td>
              <td className="px-3 py-2">
                <div className="font-mono text-[11px] text-[var(--text-primary)] break-all line-clamp-2" title={e.declaration}>
                  {e.declaration}
                </div>
                {e.old_declaration && (
                  <div className="font-mono text-[10px] text-[var(--text-muted)] break-all line-through mt-0.5 line-clamp-1">
                    {e.old_declaration}
                  </div>
                )}
              </td>
              <td className="px-3 py-2 whitespace-nowrap">
                <span className={`px-1.5 py-0.5 rounded border text-[10px] ${changeTypeColor(e.change_type)}`}>
                  {e.change_type}
                </span>
              </td>
              <td className="px-3 py-2 text-[var(--text-secondary)] whitespace-nowrap">{e.version_label}</td>
              <td className="px-3 py-2 text-[var(--text-secondary)] whitespace-nowrap font-mono">{e.api_level ?? '-'}</td>
              <td className="px-3 py-2 whitespace-nowrap text-right">
                <button
                  onClick={() => onDelete(e)}
                  className="px-2 py-1 text-[10px] border border-[var(--danger)]/40 text-[var(--danger)] rounded hover:bg-[var(--danger)]/10 transition-colors"
                >
                  {t('apiKb.delete')}
                </button>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  )
}

function DetailsTable({
  items, onOpen, onEdit, onDelete,
}: {
  items: DetailListItem[]
  onOpen: (i: DetailListItem) => void
  onEdit: (i: DetailListItem) => void
  onDelete: (i: DetailListItem) => void
}) {
  const { t } = useTranslation()
  if (items.length === 0) {
    return <div className="p-8 text-center text-sm text-[var(--text-secondary)]">{t('apiKb.empty')}</div>
  }
  return (
    <div className="overflow-x-auto">
      <table className="w-full text-[12px]">
        <thead className="modern-card-b border-[var(--border)]">
          <tr className="text-left text-[var(--text-muted)]">
            <th className="px-3 py-2 font-medium whitespace-nowrap">{t('apiKb.colModule')}</th>
            <th className="px-3 py-2 font-medium whitespace-nowrap">{t('apiKb.colTitle')}</th>
            <th className="px-3 py-2 font-medium whitespace-nowrap">{t('apiKb.colKit')}</th>
            <th className="px-3 py-2 font-medium whitespace-nowrap">{t('apiKb.colLevel')}</th>
            <th className="px-3 py-2 font-medium whitespace-nowrap">{t('apiKb.colMembers')}</th>
            <th className="px-3 py-2 font-medium"></th>
          </tr>
        </thead>
        <tbody>
          {items.map((e) => (
            <tr
              key={e.slug}
              className="border-b border-[var(--border)]/50 hover:bg-[var(--bg-card)]/50 cursor-pointer"
              onClick={() => onOpen(e)}
            >
              <td className="px-3 py-2 font-mono text-[11px] text-[var(--accent)] whitespace-nowrap">{e.module}</td>
              <td className="px-3 py-2 text-[var(--text-primary)]">
                <div className="flex items-center gap-1.5">
                  {e.title ?? e.slug}
                  {e.deprecated && (
                    <span className="px-1 py-px rounded bg-[var(--warning)]/15 text-[var(--warning)] text-[9px] border border-[var(--warning)]/40">
                      {t('apiKb.deprecated')}
                    </span>
                  )}
                </div>
              </td>
              <td className="px-3 py-2 text-[var(--text-secondary)] whitespace-nowrap">{e.kit ?? '-'}</td>
              <td className="px-3 py-2 text-[var(--text-secondary)] whitespace-nowrap font-mono">{e.since_api_level ?? '-'}</td>
              <td className="px-3 py-2 text-[var(--text-secondary)] whitespace-nowrap">{e.member_count}</td>
              <td className="px-3 py-2 whitespace-nowrap text-right" onClick={(ev) => ev.stopPropagation()}>
                <button
                  onClick={() => onEdit(e)}
                  className="px-2 py-1 mr-1 text-[10px] border border-[var(--accent)]/40 text-[var(--accent)] rounded hover:bg-[var(--accent-soft)] transition-colors"
                >
                  {t('apiKb.edit')}
                </button>
                <button
                  onClick={() => onDelete(e)}
                  className="px-2 py-1 text-[10px] border border-[var(--danger)]/40 text-[var(--danger)] rounded hover:bg-[var(--danger)]/10 transition-colors"
                >
                  {t('apiKb.delete')}
                </button>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  )
}

function DetailDrawer({
  detail, loading, onClose, onEdit,
}: {
  detail: ApiDetailFull
  loading: boolean
  onClose: () => void
  onEdit: () => void
}) {
  const { t } = useTranslation()
  return (
    <div className="fixed inset-0 z-50 flex justify-end bg-black/50 backdrop-blur-sm" onClick={onClose}>
      <div
        className="w-full max-w-2xl h-full bg-[var(--bg-primary)] border-l border-[var(--border)] overflow-y-auto animate-fade-in-up"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="sticky top-0 z-10 bg-[var(--bg-primary)] border-b border-[var(--border)] px-5 py-3 flex items-center justify-between">
          <div className="min-w-0">
            <div className="font-mono text-[12px] text-[var(--accent)]">{detail.module}</div>
            <h3 className="text-base font-semibold text-[var(--text-primary)] truncate">{detail.title ?? detail.slug}</h3>
          </div>
          <div className="flex items-center gap-2 shrink-0">
            <button
              onClick={onEdit}
              className="h-8 px-3 rounded-lg border border-[var(--accent)] text-[var(--accent)] text-[12px] hover:bg-[var(--accent-soft)] transition-colors"
            >
              {t('apiKb.edit')}
            </button>
            <button
              onClick={onClose}
              className="h-8 w-8 rounded-lg border border-[var(--border)] text-[var(--text-secondary)] hover:bg-[var(--bg-hover)] transition-colors"
            >
              ✕
            </button>
          </div>
        </div>

        {loading ? (
          <div className="p-8 text-center text-sm text-[var(--text-secondary)]">{t('apiKb.loading')}</div>
        ) : (
          <div className="p-5 space-y-4">
            <div className="flex flex-wrap gap-2 text-[11px]">
              {detail.kit && <Tag label={t('apiKb.colKit')} value={detail.kit} />}
              {detail.since_api_level != null && <Tag label={t('apiKb.colLevel')} value={`API ${detail.since_api_level}`} />}
              {detail.deprecated && <Tag label={t('apiKb.deprecated')} value="⚠" warn />}
              {detail.syscap && <Tag label="SysCap" value={detail.syscap} />}
              {detail.device_types && <Tag label={t('apiKb.deviceTypes')} value={detail.device_types} />}
            </div>

            {detail.import_snippet && (
              <Section title={t('apiKb.importSnippet')}>
                <pre className="modern-card rounded-lg p-3 text-[11px] font-mono text-[var(--text-primary)] overflow-x-auto whitespace-pre-wrap">
                  {detail.import_snippet}
                </pre>
              </Section>
            )}

            {detail.permissions && (
              <Section title={t('apiKb.permissions')}>
                <pre className="modern-card rounded-lg p-3 text-[11px] font-mono text-[var(--text-primary)] overflow-x-auto whitespace-pre-wrap">
                  {detail.permissions}
                </pre>
              </Section>
            )}

            <Section title={t('apiKb.body')}>
              <div className="modern-card rounded-lg p-3 text-[12px] text-[var(--text-primary)] whitespace-pre-wrap leading-relaxed max-h-96 overflow-y-auto">
                {detail.body}
              </div>
            </Section>

            {detail.members.length > 0 && (
              <Section title={t('apiKb.members', { n: detail.members.length })}>
                <div className="space-y-1.5">
                  {detail.members.map((m, i) => (
                    <div key={i} className="modern-card rounded-lg p-2.5">
                      <div className="flex items-center gap-2 flex-wrap">
                        <span className="px-1.5 py-0.5 rounded bg-[var(--bg-secondary)] text-[var(--text-muted)] text-[9px] border border-[var(--border)]">
                          {m.kind}
                        </span>
                        <span className="font-mono text-[12px] text-[var(--text-primary)]">{m.member_name}</span>
                        {m.parent_name && (
                          <span className="text-[10px] text-[var(--text-muted)]">← {m.parent_name}</span>
                        )}
                        {m.deprecated && (
                          <span className="px-1 py-px rounded bg-[var(--warning)]/15 text-[var(--warning)] text-[9px]">
                            {t('apiKb.deprecated')}
                          </span>
                        )}
                        {m.since_api_level != null && (
                          <span className="text-[10px] text-[var(--text-muted)] ml-auto">API {m.since_api_level}</span>
                        )}
                      </div>
                      {m.declaration && (
                        <pre className="mt-1.5 text-[10px] font-mono text-[var(--text-secondary)] whitespace-pre-wrap break-all">
                          {m.declaration}
                        </pre>
                      )}
                      {m.description && (
                        <p className="mt-1 text-[11px] text-[var(--text-secondary)] whitespace-pre-wrap">{m.description}</p>
                      )}
                      {m.permission && (
                        <p className="mt-1 text-[10px] text-[var(--warning)] font-mono break-all">🔒 {m.permission}</p>
                      )}
                    </div>
                  ))}
                </div>
              </Section>
            )}

            {detail.examples && (
              <Section title={t('apiKb.examples')}>
                <pre className="modern-card rounded-lg p-3 text-[11px] font-mono text-[var(--text-primary)] overflow-x-auto whitespace-pre-wrap max-h-96 overflow-y-auto">
                  {detail.examples}
                </pre>
              </Section>
            )}

            <div className="pt-2 border-t border-[var(--border)]">
              <a
                href={detail.source_url}
                target="_blank"
                rel="noreferrer"
                className="text-[11px] text-[var(--accent)] hover:underline break-all"
              >
                {t('apiKb.viewSource')}: {detail.source_url}
              </a>
              <div className="text-[10px] text-[var(--text-muted)] mt-1">
                {t('apiKb.fetchedAt')}: {formatTime(detail.fetched_at)}
              </div>
            </div>
          </div>
        )}
      </div>
    </div>
  )
}

function Tag({ label, value, warn }: { label: string; value: string; warn?: boolean }) {
  return (
    <span className={`inline-flex items-center gap-1 px-2 py-1 rounded-lg border text-[10px] ${
      warn
        ? 'bg-[var(--warning)]/15 text-[var(--warning)] border-[var(--warning)]/40'
        : 'bg-[var(--bg-card)] text-[var(--text-secondary)] border-[var(--border)]'
    }`}>
      <span className="text-[var(--text-muted)]">{label}:</span>
      <span className="font-mono">{value}</span>
    </span>
  )
}

function Section({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <div>
      <div className="text-[11px] font-medium text-[var(--text-muted)] mb-1.5 uppercase tracking-wide">{title}</div>
      {children}
    </div>
  )
}

function DocFormModal({ onClose, onSaved }: { onClose: () => void; onSaved: () => void }) {
  const { t } = useTranslation()
  const [form, setForm] = useState<DocInput>({
    kit: '', declaration: '', changeType: 'added', versionLabel: '',
  })
  const [saving, setSaving] = useState(false)

  const save = async () => {
    if (!form.kit.trim() || !form.declaration.trim() || !form.versionLabel.trim()) {
      alert(t('apiKb.fieldsRequired'))
      return
    }
    setSaving(true)
    try {
      await apiDocAdd(form)
      onSaved()
    } catch (e) {
      alert(String(e))
    } finally {
      setSaving(false)
    }
  }

  return (
    <Modal title={t('apiKb.addDoc')} onClose={onClose}>
      <div className="grid grid-cols-2 gap-3">
        <Field label={t('apiKb.colKit')} required>
          <input className={inputCls} value={form.kit} onChange={(e) => setForm({ ...form, kit: e.target.value })} />
        </Field>
        <Field label={t('apiKb.colVersion')} required>
          <input className={inputCls} value={form.versionLabel} onChange={(e) => setForm({ ...form, versionLabel: e.target.value })} placeholder="5.0.0(12)" />
        </Field>
        <Field label={t('apiKb.colModule')}>
          <input className={inputCls} value={form.module ?? ''} onChange={(e) => setForm({ ...form, module: e.target.value })} placeholder="@ohos.module" />
        </Field>
        <Field label={t('apiKb.colClassName')}>
          <input className={inputCls} value={form.className ?? ''} onChange={(e) => setForm({ ...form, className: e.target.value })} />
        </Field>
        <Field label={t('apiKb.colChange')}>
          <select className={inputCls} value={form.changeType} onChange={(e) => setForm({ ...form, changeType: e.target.value })}>
            <option value="added">{t('apiKb.ctAdded')}</option>
            <option value="modified">{t('apiKb.ctModified')}</option>
            <option value="removed">{t('apiKb.ctRemoved')}</option>
            <option value="deprecated">{t('apiKb.ctDeprecated')}</option>
          </select>
        </Field>
        <Field label={t('apiKb.colLevel')}>
          <input className={inputCls} value={form.apiLevel ?? ''} onChange={(e) => setForm({ ...form, apiLevel: e.target.value ? Number(e.target.value) : undefined })} />
        </Field>
        <Field label={t('apiKb.apiName')}>
          <input className={inputCls} value={form.apiName ?? ''} onChange={(e) => setForm({ ...form, apiName: e.target.value })} />
        </Field>
        <Field label={t('apiKb.dtsFile')}>
          <input className={inputCls} value={form.dtsFile ?? ''} onChange={(e) => setForm({ ...form, dtsFile: e.target.value })} />
        </Field>
        <div className="col-span-2">
          <Field label={t('apiKb.colDeclaration')} required>
            <textarea className={`${inputCls} h-24 resize-y font-mono`} value={form.declaration} onChange={(e) => setForm({ ...form, declaration: e.target.value })} />
          </Field>
        </div>
        <div className="col-span-2">
          <Field label={t('apiKb.oldDeclaration')}>
            <textarea className={`${inputCls} h-16 resize-y font-mono`} value={form.oldDeclaration ?? ''} onChange={(e) => setForm({ ...form, oldDeclaration: e.target.value })} />
          </Field>
        </div>
      </div>
      <ModalActions onClose={onClose} onSave={save} saving={saving} saveLabel={t('apiKb.save')} />
    </Modal>
  )
}

function DetailFormModal({
  editing, onClose, onSaved,
}: {
  editing: DetailListItem | null
  onClose: () => void
  onSaved: () => void
}) {
  const { t } = useTranslation()
  const [form, setForm] = useState<DetailInput>(() => ({
    module: editing?.module ?? '',
    title: editing?.title ?? '',
    kit: editing?.kit ?? '',
    sinceApiLevel: editing?.since_api_level ?? undefined,
    deprecated: editing?.deprecated ?? false,
    body: '',
    sourceUrl: editing?.source_url ?? '',
  }))
  const [loading, setLoading] = useState(false)
  const [saving, setSaving] = useState(false)

  useEffect(() => {
    if (editing) {
      setLoading(true)
      apiDetailGet(editing.slug)
        .then((d) => {
          setForm({
            module: d.module,
            title: d.title ?? '',
            kit: d.kit ?? '',
            sinceApiLevel: d.since_api_level ?? undefined,
            deprecated: d.deprecated,
            importSnippet: d.import_snippet ?? '',
            syscap: d.syscap ?? '',
            permissions: d.permissions ?? '',
            deviceTypes: d.device_types ?? '',
            body: d.body,
            examples: d.examples ?? '',
            sourceUrl: d.source_url,
          })
        })
        .catch((e) => alert(String(e)))
        .finally(() => setLoading(false))
    }
  }, [editing])

  const save = async () => {
    if (!form.module.trim() || !form.body.trim()) {
      alert(t('apiKb.fieldsRequired'))
      return
    }
    setSaving(true)
    try {
      await apiDetailUpsert(form)
      onSaved()
    } catch (e) {
      alert(String(e))
    } finally {
      setSaving(false)
    }
  }

  return (
    <Modal title={editing ? t('apiKb.editDetail') : t('apiKb.addDetail')} onClose={onClose} wide>
      {loading ? (
        <div className="py-8 text-center text-sm text-[var(--text-secondary)]">{t('apiKb.loading')}</div>
      ) : (
        <div className="grid grid-cols-2 gap-3">
          <Field label={t('apiKb.colModule')} required>
            <input className={inputCls} value={form.module} onChange={(e) => setForm({ ...form, module: e.target.value })} disabled={!!editing} />
          </Field>
          <Field label={t('apiKb.colTitle')}>
            <input className={inputCls} value={form.title ?? ''} onChange={(e) => setForm({ ...form, title: e.target.value })} />
          </Field>
          <Field label={t('apiKb.colKit')}>
            <input className={inputCls} value={form.kit ?? ''} onChange={(e) => setForm({ ...form, kit: e.target.value })} />
          </Field>
          <Field label={t('apiKb.colLevel')}>
            <input className={inputCls} value={form.sinceApiLevel ?? ''} onChange={(e) => setForm({ ...form, sinceApiLevel: e.target.value ? Number(e.target.value) : undefined })} />
          </Field>
          <Field label={t('apiKb.sourceUrl')}>
            <input className={`${inputCls} font-mono`} value={form.sourceUrl} onChange={(e) => setForm({ ...form, sourceUrl: e.target.value })} />
          </Field>
          <Field label={t('apiKb.syscap')}>
            <input className={inputCls} value={form.syscap ?? ''} onChange={(e) => setForm({ ...form, syscap: e.target.value })} />
          </Field>
          <div className="col-span-2 flex items-center gap-4">
            <label className="flex items-center gap-1.5 text-[12px] text-[var(--text-secondary)] cursor-pointer">
              <input type="checkbox" checked={form.deprecated} onChange={(e) => setForm({ ...form, deprecated: e.target.checked })} className="w-3.5 h-3.5 accent-[var(--accent)]" />
              {t('apiKb.deprecated')}
            </label>
            <Field label={t('apiKb.deviceTypes')}>
              <input className={`${inputCls} w-48`} value={form.deviceTypes ?? ''} onChange={(e) => setForm({ ...form, deviceTypes: e.target.value })} />
            </Field>
          </div>
          <div className="col-span-2">
            <Field label={t('apiKb.importSnippet')}>
              <textarea className={`${inputCls} h-20 resize-y font-mono`} value={form.importSnippet ?? ''} onChange={(e) => setForm({ ...form, importSnippet: e.target.value })} />
            </Field>
          </div>
          <div className="col-span-2">
            <Field label={t('apiKb.permissions')}>
              <textarea className={`${inputCls} h-16 resize-y font-mono`} value={form.permissions ?? ''} onChange={(e) => setForm({ ...form, permissions: e.target.value })} />
            </Field>
          </div>
          <div className="col-span-2">
            <Field label={t('apiKb.body')} required>
              <textarea className={`${inputCls} h-48 resize-y font-mono`} value={form.body} onChange={(e) => setForm({ ...form, body: e.target.value })} />
            </Field>
          </div>
          <div className="col-span-2">
            <Field label={t('apiKb.examples')}>
              <textarea className={`${inputCls} h-32 resize-y font-mono`} value={form.examples ?? ''} onChange={(e) => setForm({ ...form, examples: e.target.value })} />
            </Field>
          </div>
        </div>
      )}
      <ModalActions onClose={onClose} onSave={save} saving={saving || loading} saveLabel={t('apiKb.save')} />
    </Modal>
  )
}

const inputCls = "w-full h-9 px-3 modern-card rounded-lg text-[12px] text-[var(--text-primary)] placeholder:text-[var(--text-muted)] focus:outline-none focus:border-[var(--accent)]"

function Field({ label, required, children }: { label: string; required?: boolean; children: React.ReactNode }) {
  return (
    <div className="space-y-1">
      <label className="block text-[11px] text-[var(--text-muted)]">
        {label}{required && <span className="text-[var(--danger)] ml-0.5">*</span>}
      </label>
      {children}
    </div>
  )
}

function Modal({ title, onClose, children, wide }: { title: string; onClose: () => void; children: React.ReactNode; wide?: boolean }) {
  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 backdrop-blur-sm p-4" onClick={onClose}>
      <div
        className={`w-full ${wide ? 'max-w-3xl' : 'max-w-xl'} max-h-[90vh] overflow-y-auto bg-[var(--bg-primary)] border border-[var(--border)] rounded-2xl shadow-2xl animate-fade-in-up`}
        onClick={(e) => e.stopPropagation()}
      >
        <div className="sticky top-0 bg-[var(--bg-primary)] border-b border-[var(--border)] px-5 py-3 flex items-center justify-between z-10">
          <h3 className="text-base font-semibold text-[var(--text-primary)]">{title}</h3>
          <button onClick={onClose} className="h-8 w-8 rounded-lg border border-[var(--border)] text-[var(--text-secondary)] hover:bg-[var(--bg-hover)] transition-colors">✕</button>
        </div>
        <div className="p-5">{children}</div>
      </div>
    </div>
  )
}

function ModalActions({ onClose, onSave, saving, saveLabel }: { onClose: () => void; onSave: () => void; saving: boolean; saveLabel: string }) {
  const { t } = useTranslation()
  return (
    <div className="flex justify-end gap-2 mt-5 pt-4 border-t border-[var(--border)]">
      <button
        onClick={onClose}
        className="h-9 px-4 rounded-lg border border-[var(--border)] text-[12px] text-[var(--text-secondary)] hover:bg-[var(--bg-hover)] transition-colors"
      >
        {t('apiKb.cancel')}
      </button>
      <button
        onClick={onSave}
        disabled={saving}
        className="h-9 px-4 rounded-lg btn-primary text-[12px]  disabled:opacity-50 transition-colors"
      >
        {saving ? '...' : saveLabel}
      </button>
    </div>
  )
}







