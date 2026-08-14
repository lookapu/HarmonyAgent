import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import {
  listSdkApiModules,
  searchSdkApi,
  readSdkApiModule,
  type ApiModule,
  type ApiIndex,
} from '../api/harmonyEnv'
import Markdown from './Markdown'

const DEBOUNCE_MS = 280

/**
 * SDK API 浏览器：浏览/检索本地 HarmonyOS SDK 的 @ohos.*.d.ts 声明模块。
 * - 顶部搜索框：实时按关键字检索模块/kit/syscap/声明名
 * - 左侧：模块列表（可按 kit 筛选），右侧：选中模块的完整 d.ts 内容
 */
export default function SdkApiBrowser() {
  const { t } = useTranslation()
  const [index, setIndex] = useState<ApiIndex | null>(null)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [query, setQuery] = useState('')
  const [results, setResults] = useState<ApiModule[]>([])
  const [searching, setSearching] = useState(false)
  const [activeKit, setActiveKit] = useState<string | null>(null)
  const [selected, setSelected] = useState<ApiModule | null>(null)
  const [content, setContent] = useState<string | null>(null)
  const [contentLoading, setContentLoading] = useState(false)
  const debounceRef = useRef<number | null>(null)

  const loadIndex = useCallback(async () => {
    setLoading(true)
    setError(null)
    try {
      const idx = await listSdkApiModules()
      setIndex(idx)
    } catch (e) {
      setError(String(e))
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => {
    loadIndex()
  }, [loadIndex])

  useEffect(() => {
    if (!query.trim()) {
      setResults([])
      setSearching(false)
      return
    }
    setSearching(true)
    if (debounceRef.current) clearTimeout(debounceRef.current)
    debounceRef.current = window.setTimeout(async () => {
      try {
        const hits = await searchSdkApi(query.trim(), 40)
        setResults(hits)
      } catch {
        setResults([])
      } finally {
        setSearching(false)
      }
    }, DEBOUNCE_MS)
    return () => {
      if (debounceRef.current) clearTimeout(debounceRef.current)
    }
  }, [query])

  const visibleModules = useMemo<ApiModule[]>(() => {
    if (!index) return []
    let list = index.modules
    if (activeKit) {
      list = list.filter((m) => m.kit === activeKit)
    }
    return list
  }, [index, activeKit])

  const shown = query.trim() ? results : visibleModules
  const kits = index ? Object.keys(index.by_kit).sort() : []

  const selectModule = useCallback(async (m: ApiModule) => {
    setSelected(m)
    setContent(null)
    setContentLoading(true)
    try {
      const text = await readSdkApiModule(m.module)
      setContent('```typescript\n' + text + '\n```')
    } catch (e) {
      setContent(t('health.apiReadFailed', { err: String(e) }))
    } finally {
      setContentLoading(false)
    }
  }, [t])

  if (error) {
    return (
      <div className="rounded-lg border border-[var(--warning)]/30 bg-[var(--warning)]/8 px-3 py-2 text-[12px] text-[var(--warning)]">
        {error}
      </div>
    )
  }

  return (
    <div className="bg-[var(--bg-secondary)] border border-[var(--border)] rounded-lg overflow-hidden">
      <div className="flex items-center gap-2 p-3 border-b border-[var(--border)]">
        <input
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder={t('health.apiSearchPlaceholder')}
          className="flex-1 h-8 rounded-lg bg-[var(--bg-primary)] border border-[var(--border)] px-3 text-[12px] outline-none focus:border-[var(--accent)]"
        />
        <button
          onClick={loadIndex}
          disabled={loading}
          className="h-8 px-3 rounded-lg border border-[var(--border)] text-[12px] text-[var(--text-secondary)] hover:bg-[var(--bg-hover)] disabled:opacity-50"
        >
          {loading ? '…' : t('health.apiRefresh')}
        </button>
      </div>

      <div className="flex" style={{ minHeight: 320, maxHeight: 520 }}>
        <div className="w-64 shrink-0 border-r border-[var(--border)] flex flex-col">
          {kits.length > 0 && !query.trim() && (
            <div className="flex flex-wrap gap-1 p-2 border-b border-[var(--border)] max-h-24 overflow-auto">
              <button
                onClick={() => setActiveKit(null)}
                className={`px-2 h-6 rounded-md text-[11px] transition-colors ${activeKit === null ? 'bg-[var(--accent)] text-white' : 'bg-[var(--bg-primary)] text-[var(--text-secondary)] hover:bg-[var(--bg-hover)]'}`}
              >
                {t('health.apiAllKits')}
              </button>
              {kits.slice(0, 30).map((k) => (
                <button
                  key={k}
                  onClick={() => setActiveKit(k === activeKit ? null : k)}
                  className={`px-2 h-6 rounded-md text-[11px] transition-colors ${k === activeKit ? 'bg-[var(--accent)] text-white' : 'bg-[var(--bg-primary)] text-[var(--text-secondary)] hover:bg-[var(--bg-hover)]'}`}
                  title={k}
                >
                  {k}
                </button>
              ))}
            </div>
          )}
          <div className="flex-1 overflow-auto">
            {searching && <div className="text-[11px] text-[var(--text-muted)] p-3">{t('health.apiSearching')}</div>}
            {!searching && shown.length === 0 && (
              <div className="text-[11px] text-[var(--text-muted)] p-3">
                {query.trim() ? t('health.apiNoMatch') : index ? t('health.apiModuleCount', { count: index.modules.length }) : t('health.apiLoading')}
              </div>
            )}
            {shown.map((m) => (
              <button
                key={m.module}
                onClick={() => selectModule(m)}
                className={`w-full text-left px-3 py-1.5 text-[11px] border-l-2 transition-colors ${selected?.module === m.module ? 'border-[var(--accent)] bg-[var(--accent)]/8' : 'border-transparent hover:bg-[var(--bg-hover)]'}`}
              >
                <div className="font-mono text-[var(--text-primary)] truncate">{m.module}</div>
                <div className="flex items-center gap-1.5 text-[10px] text-[var(--text-muted)] mt-0.5">
                  {m.kit && <span className="text-[var(--accent)]">{m.kit}</span>}
                  {m.since_min != null && <span>{t('health.apiSince', { version: m.since_min })}</span>}
                  {m.deprecated && <span className="text-[var(--warning)]">{t('health.apiDeprecated')}</span>}
                </div>
              </button>
            ))}
          </div>
        </div>

        <div className="flex-1 min-w-0 overflow-auto bg-[var(--bg-primary)]">
          {!selected ? (
            <div className="text-[12px] text-[var(--text-muted)] p-4">
              {index
                ? t('health.apiSelectHint', { count: index.modules.length, dir: index.api_dir })
                : t('health.apiLoadingIndex')}
            </div>
          ) : (
            <div>
              <div className="sticky top-0 z-10 bg-[var(--bg-secondary)] border-b border-[var(--border)] px-3 py-2 flex items-center gap-2 flex-wrap">
                <span className="font-mono text-[12px] text-[var(--accent)]">{selected.module}</span>
                {selected.kit && <span className="text-[10px] px-1.5 py-0.5 rounded bg-[var(--accent)]/10 text-[var(--accent)]">{selected.kit}</span>}
                {selected.syscap && <span className="text-[10px] text-[var(--text-muted)] font-mono truncate">{selected.syscap}</span>}
                {selected.since_min != null && (
                  <span className="text-[10px] text-[var(--text-muted)]">{t('health.apiSinceRange', { min: selected.since_min, max: selected.since_max && selected.since_max !== selected.since_min ? `–${selected.since_max}` : '' })}</span>
                )}
              </div>
              {contentLoading ? (
                <div className="text-[11px] text-[var(--text-muted)] p-4">{t('health.apiReading')}</div>
              ) : content ? (
                <div className="text-[11px]">
                  <Markdown>{content}</Markdown>
                </div>
              ) : null}
            </div>
          )}
        </div>
      </div>
    </div>
  )
}
