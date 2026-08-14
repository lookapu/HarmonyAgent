import { useEffect, useMemo, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import type { TerminalEntry, BuildLogLine } from '../../stores/projectStore'
import type { ProjectMemory, ToolStat } from '../../api/project'
import { gitBranchInfo, type GitBranchInfo } from '../../api/git'
import Icon from '../../icons/Icon'
import { AnsiText, hasAnsi } from '../../components/AnsiText'
import { fmtElapsed } from '../chatUtils'

/* ============ 概览信息行 ============ */
export function OverviewRow({
  icon,
  label,
  value,
  mono,
  tone,
}: {
  icon: 'folder' | 'health' | 'check' | 'info'
  label: string
  value: string
  mono?: boolean
  tone?: 'ok' | 'warn'
}) {
  const color = tone === 'ok' ? 'text-[var(--success)]' : tone === 'warn' ? 'text-[var(--warning)]' : 'text-[var(--text-primary)]'
  return (
    <div className="flex items-start justify-between gap-3">
      <span className="flex items-center gap-1.5 text-[var(--text-secondary)] text-[11px] shrink-0 pt-px">
        <Icon name={icon} size={13} className="opacity-50" />
        {label}
      </span>
      <span className={`text-right text-[11px] break-all ${mono ? 'font-mono' : ''} ${color}`}>{value}</span>
    </div>
  )
}

/** 概览 Git 变更摘要：当前分支 + 已跟踪/未跟踪计数，点击进入 Git 面板 */
export function OverviewGitSummary({ projectPath, onOpenGit }: { projectPath: string; onOpenGit: () => void }) {
  const { t } = useTranslation()
  const [info, setInfo] = useState<GitBranchInfo | null>(null)

  useEffect(() => {
    let alive = true
    setInfo(null)
    gitBranchInfo(projectPath)
      .then((v) => {
        if (alive) setInfo(v)
      })
      .catch(() => {})
    return () => {
      alive = false
    }
  }, [projectPath])

  return (
    <div className="rounded-xl border border-[var(--border)] bg-[var(--bg-card)] p-3">
      <div className="flex items-center gap-1.5">
        <Icon name="git-branch" size={12} className="text-[var(--text-secondary)]" />
        <span className="text-[12px] font-medium">{t('home.gitChanges')}</span>
        <button
          type="button"
          onClick={onOpenGit}
          className="ml-auto text-[10.5px] px-2 py-0.5 rounded-md text-[var(--accent)] bg-[var(--accent-soft)] hover:bg-[var(--accent)]/15 transition-colors"
        >
          {t('home.viewChanges')}
        </button>
      </div>
      {info ? (
        info.is_repo ? (
          <div className="mt-2 space-y-1.5">
            <OverviewRow icon="folder" label={t('home.gitBranch')} value={info.current} mono />
            <OverviewRow
              icon="check"
              label={t('home.gitChanged')}
              value={`${t('home.gitTracked', { n: info.changed })} · ${t('home.gitUntracked', { n: info.untracked })}`}
              tone={info.changed > 0 || info.untracked > 0 ? 'warn' : 'ok'}
            />
          </div>
        ) : (
          <div className="mt-2 text-[11px] text-[var(--text-muted)]">{t('home.gitNotRepo')}</div>
        )
      ) : (
        <div className="mt-2 text-[11px] text-[var(--text-muted)] animate-pulse">{t('home.loading')}</div>
      )}
    </div>
  )
}

/* ============ 项目记忆面板（Memory：增删改/启用开关） ============ */
export function MemoriesPanel({
  memories,
  onSave,
  onDelete,
  onToggle,
  onRefresh,
}: {
  memories: ProjectMemory[]
  onSave: (input: { id?: string; category: string; title: string; content: string }) => Promise<void>
  onDelete: (id: string) => Promise<void>
  onToggle: (id: string, enabled: boolean) => Promise<void>
  onRefresh: () => Promise<void>
}) {
  const { t } = useTranslation()
  const [editing, setEditing] = useState<ProjectMemory | null>(null)
  const [showForm, setShowForm] = useState(false)
  const [title, setTitle] = useState('')
  const [category, setCategory] = useState('general')
  const [content, setContent] = useState('')
  const [busy, setBusy] = useState(false)
  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null)

  const categories = ['general', 'code', 'build', 'deploy', 'decision', 'pitfall', 'path']

  const startEdit = (m: ProjectMemory) => {
    setEditing(m)
    setTitle(m.title)
    setCategory(m.category)
    setContent(m.content)
    setShowForm(true)
  }

  const startAdd = () => {
    setEditing(null)
    setTitle('')
    setCategory('general')
    setContent('')
    setShowForm(true)
  }

  const submit = async () => {
    if (!title.trim() || !content.trim()) return
    setBusy(true)
    try {
      await onSave({ id: editing?.id, category, title: title.trim(), content: content.trim() })
      setShowForm(false)
      setEditing(null)
    } finally {
      setBusy(false)
    }
  }

  const handleDelete = async (id: string) => {
    if (confirmDeleteId !== id) {
      setConfirmDeleteId(id)
      setTimeout(() => setConfirmDeleteId((cur) => (cur === id ? null : cur)), 3000)
      return
    }
    setConfirmDeleteId(null)
    await onDelete(id)
  }

  return (
    <div className="p-3 space-y-2.5">
      {/* 说明 + 新增 */}
      <div className="flex items-center justify-between gap-2">
        <p className="text-[11px] text-[var(--text-muted)] leading-relaxed">{t('home.memoriesTip')}</p>
        <button
          onClick={startAdd}
          className="shrink-0 h-7 px-2.5 rounded-lg bg-[var(--accent)]/10 text-[var(--accent)] text-[11px] font-medium flex items-center gap-1 hover:bg-[var(--accent)]/20 active:scale-95 transition-all"
        >
          <Icon name="plus" size={12} />
          {t('home.memoryAdd')}
        </button>
      </div>

      {/* 新增/编辑表单 */}
      {showForm && (
        <div className="rounded-xl border border-[var(--accent)]/25 bg-[var(--bg-card)] p-3 space-y-2 animate-fade-in-up">
          <input
            value={title}
            onChange={(e) => setTitle(e.target.value)}
            placeholder={t('home.memoryTitlePlaceholder')}
            className="w-full h-8 px-2.5 rounded-lg bg-[var(--bg-primary)] border border-[var(--border)] text-[12px] outline-none focus:border-[var(--accent)] transition-colors"
          />
          <select
            value={category}
            onChange={(e) => setCategory(e.target.value)}
            className="w-full h-8 px-2 rounded-lg bg-[var(--bg-primary)] border border-[var(--border)] text-[12px] outline-none focus:border-[var(--accent)] transition-colors cursor-pointer"
          >
            {categories.map((c) => (
              <option key={c} value={c}>
                {t(`home.memoryCat.${c}`)}
              </option>
            ))}
          </select>
          <textarea
            value={content}
            onChange={(e) => setContent(e.target.value)}
            placeholder={t('home.memoryContentPlaceholder')}
            rows={4}
            className="w-full px-2.5 py-2 rounded-lg bg-[var(--bg-primary)] border border-[var(--border)] text-[12px] outline-none focus:border-[var(--accent)] transition-colors resize-y"
          />
          <div className="flex items-center justify-end gap-2">
            <button
              onClick={() => setShowForm(false)}
              className="h-7 px-3 rounded-lg text-[11px] text-[var(--text-secondary)] hover:bg-[var(--bg-hover)] transition-colors"
            >
              {t('common.cancel')}
            </button>
            <button
              onClick={submit}
              disabled={busy || !title.trim() || !content.trim()}
              className="h-7 px-3 rounded-lg bg-[var(--accent)] text-white text-[11px] font-medium hover:bg-[var(--accent-hover)] active:scale-95 disabled:opacity-40 transition-all"
            >
              {t('common.save')}
            </button>
          </div>
        </div>
      )}

      {/* 记忆列表 */}
      {memories.length === 0 ? (
        <div className="rounded-xl border border-dashed border-[var(--border)] p-5 text-center">
          <p className="text-[12px] text-[var(--text-muted)]">{t('home.memoriesEmpty')}</p>
        </div>
      ) : (
        memories.map((m) => (
          <div
            key={m.id}
            className={`rounded-xl border border-[var(--border)] bg-[var(--bg-card)] p-3 transition-opacity ${m.enabled ? '' : 'opacity-55'}`}
          >
            <div className="flex items-center gap-2">
              <span className="shrink-0 text-[9px] px-1.5 py-0.5 rounded bg-[var(--accent-soft)] text-[var(--accent)]">
                {t(`home.memoryCat.${m.category}`)}
              </span>
              {m.title.startsWith('构建错误自动修复记录') && (
                <span className="shrink-0 text-[9px] px-1.5 py-0.5 rounded bg-emerald-500/10 text-emerald-500">
                  {t('home.memoryAuto')}
                </span>
              )}
              <span className="flex-1 min-w-0 text-[12px] font-medium truncate">{m.title}</span>
              <button
                onClick={() => onToggle(m.id, !m.enabled)}
                className={`shrink-0 w-7 h-4 rounded-full transition-colors relative ${m.enabled ? 'bg-[var(--accent)]' : 'bg-[var(--border-strong)]'}`}
                title={m.enabled ? t('home.memoryDisable') : t('home.memoryEnable')}
              >
                <span
                  className={`absolute top-0.5 w-3 h-3 rounded-full bg-white transition-all ${m.enabled ? 'left-3.5' : 'left-0.5'}`}
                />
              </button>
              <button
                onClick={() => startEdit(m)}
                className="shrink-0 p-1 rounded-md text-[var(--text-muted)] hover:text-[var(--accent)] hover:bg-[var(--bg-hover)] transition-colors"
                title={t('home.memoryEdit')}
              >
                <Icon name="edit" size={12} />
              </button>
              <button
                onClick={() => handleDelete(m.id)}
                className={`shrink-0 p-1 rounded-md transition-colors ${confirmDeleteId === m.id ? 'text-[var(--danger)]' : 'text-[var(--text-muted)] hover:text-[var(--danger)] hover:bg-[var(--bg-hover)]'}`}
                title={confirmDeleteId === m.id ? t('home.memoryDeleteConfirm') : t('home.memoryDelete')}
              >
                <Icon name="delete" size={12} />
              </button>
            </div>
            <p className="mt-1.5 text-[11px] text-[var(--text-secondary)] leading-relaxed whitespace-pre-wrap break-all">
              {m.content}
            </p>
            <div className="mt-1.5 flex items-center justify-between">
              <span className="text-[9px] text-[var(--text-muted)]">
                {new Date(m.updated_at * 1000).toLocaleDateString()}
              </span>
              <button onClick={onRefresh} className="text-[9px] text-[var(--text-muted)] hover:text-[var(--accent)] transition-colors">
                {t('home.refresh')}
              </button>
            </div>
          </div>
        ))
      )}
    </div>
  )
}

/* ============ 工具调用统计面板（Evaluation） ============ */
export function ToolStatsPanel({ stats, onRefresh }: { stats: ToolStat[]; onRefresh: () => Promise<void> }) {
  const { t } = useTranslation()
  const totalCalls = stats.reduce((s, x) => s + x.call_count, 0)
  const totalFail = stats.reduce((s, x) => s + x.fail_count, 0)

  return (
    <div className="p-3 space-y-2.5">
      <div className="flex items-center justify-between gap-2">
        <p className="text-[11px] text-[var(--text-muted)] leading-relaxed">{t('home.toolStatsTip')}</p>
        <button
          onClick={onRefresh}
          className="shrink-0 p-1.5 rounded-lg text-[var(--text-muted)] hover:text-[var(--accent)] hover:bg-[var(--bg-hover)] transition-colors"
          title={t('home.refresh')}
        >
          <Icon name="refresh" size={13} />
        </button>
      </div>

      {/* 汇总 */}
      <div className="grid grid-cols-3 gap-2">
        <div className="rounded-xl border border-[var(--border)] bg-[var(--bg-card)] p-2.5 text-center">
          <div className="text-lg font-semibold tabular-nums">{totalCalls}</div>
          <div className="text-[10px] text-[var(--text-muted)]">{t('home.statsTotalCalls')}</div>
        </div>
        <div className="rounded-xl border border-[var(--border)] bg-[var(--bg-card)] p-2.5 text-center">
          <div className="text-lg font-semibold tabular-nums text-[var(--success)]">{totalCalls - totalFail}</div>
          <div className="text-[10px] text-[var(--text-muted)]">{t('home.statsSuccess')}</div>
        </div>
        <div className="rounded-xl border border-[var(--border)] bg-[var(--bg-card)] p-2.5 text-center">
          <div className="text-lg font-semibold tabular-nums text-[var(--danger)]">{totalFail}</div>
          <div className="text-[10px] text-[var(--text-muted)]">{t('home.statsFailed')}</div>
        </div>
      </div>

      {/* 按工具明细 */}
      {stats.length === 0 ? (
        <div className="rounded-xl border border-dashed border-[var(--border)] p-5 text-center">
          <p className="text-[12px] text-[var(--text-muted)]">{t('home.statsEmpty')}</p>
        </div>
      ) : (
        <div className="space-y-2">
          {stats.map((s) => {
            const rate = s.call_count > 0 ? Math.round((s.success_count / s.call_count) * 100) : 0
            return (
              <div key={s.tool_name} className="rounded-xl border border-[var(--border)] bg-[var(--bg-card)] p-3">
                <div className="flex items-center justify-between gap-2">
                  <span className="text-[12px] font-medium font-mono truncate">{s.tool_name}</span>
                  <span className="shrink-0 text-[11px] tabular-nums">
                    {s.call_count}×
                  </span>
                </div>
                <div className="mt-2 h-1.5 rounded-full bg-[var(--bg-primary)] overflow-hidden">
                  <div
                    className={`h-full rounded-full transition-all ${rate >= 80 ? 'bg-[var(--success)]' : rate >= 50 ? 'bg-[var(--warning)]' : 'bg-[var(--danger)]'}`}
                    style={{ width: `${rate}%` }}
                  />
                </div>
                <div className="mt-1.5 flex items-center justify-between text-[10px] text-[var(--text-muted)]">
                  <span>
                    {t('home.statsRate', { rate })} · {t('home.statsAvgMs', { ms: s.avg_duration_ms ?? '—' })}
                  </span>
                  {s.last_called_at && (
                    <span>{t('home.statsLastAt', { time: new Date(s.last_called_at * 1000).toLocaleDateString() })}</span>
                  )}
                </div>
              </div>
            )
          })}
        </div>
      )}
    </div>
  )
}

/* ============ Web 预览面板：右侧栏内嵌 iframe 加载 http/https 地址 ============ */
export function PreviewPanel({
  url,
  setUrl,
  src,
  onOpen,
}: {
  url: string
  setUrl: (v: string) => void
  src: string
  onOpen: () => void
}) {
  const { t } = useTranslation()
  const [reloadKey, setReloadKey] = useState(0)
  return (
    <div className="flex flex-col h-full min-h-0">
      <div className="flex items-center gap-1.5 p-2 border-b border-[var(--border)] shrink-0">
        <input
          value={url}
          onChange={(e) => setUrl(e.target.value)}
          onKeyDown={(e) => e.key === 'Enter' && onOpen()}
          placeholder={t('home.previewPlaceholder')}
          spellCheck={false}
          className="flex-1 min-w-0 h-8 px-2.5 rounded-lg border border-[var(--border)] bg-[var(--bg-primary)] text-[12px] outline-none focus:border-[var(--accent)] transition-colors"
        />
        <button
          onClick={onOpen}
          disabled={!url.trim()}
          title={t('home.open')}
          className="h-8 px-3 rounded-lg bg-[var(--accent)] text-white text-[12px] font-medium hover:bg-[var(--accent-hover)] active:scale-[0.98] transition-all disabled:opacity-40 shrink-0"
        >
          {t('home.open')}
        </button>
        {src && (
          <button
            onClick={() => setReloadKey((k) => k + 1)}
            title={t('home.previewReload')}
            className="w-8 h-8 flex items-center justify-center rounded-lg border border-[var(--border)] text-[var(--text-secondary)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] transition-colors shrink-0"
          >
            <Icon name="refresh" size={13} />
          </button>
        )}
      </div>
      <div className="flex-1 min-h-0 bg-white">
        {src ? (
          <iframe
            key={reloadKey}
            src={src}
            title="web preview"
            className="w-full h-full border-0"
          />
        ) : (
          <div className="h-full flex flex-col items-center justify-center gap-2 text-center px-6">
            <Icon name="devices" size={28} className="opacity-40" />
            <span className="text-[12px] text-[var(--text-muted)]">{t('home.previewEmpty')}</span>
            <span className="text-[11px] text-[var(--text-muted)]/70">{t('home.previewBlocked')}</span>
          </div>
        )}
      </div>
    </div>
  )
}

/* ============ 终端面板：工具执行实时记录（黑底终端样式，自动滚动） ============ */
export function TerminalPanel({
  entries,
  onClear,
  buildLogs,
  onClearBuild,
}: {
  entries: TerminalEntry[]
  onClear: () => void
  buildLogs: BuildLogLine[]
  onClearBuild: () => void
}) {
  const { t } = useTranslation()
  const bottomRef = useRef<HTMLDivElement>(null)
  const [tab, setTab] = useState<'tools' | 'build'>('tools')
  // 日志关键词过滤（tool/参数/输出 / 构建日志行）
  const [filter, setFilter] = useState('')
  const kw = filter.trim().toLowerCase()
  const filteredEntries = useMemo(() => {
    if (!kw) return entries
    return entries.filter(
      (e) =>
        e.tool.toLowerCase().includes(kw) ||
        (e.args ?? '').toLowerCase().includes(kw) ||
        (e.output ?? '').toLowerCase().includes(kw) ||
        (e.liveOutput ?? '').toLowerCase().includes(kw),
    )
  }, [entries, kw])
  const filteredBuild = useMemo(() => (kw ? buildLogs.filter((l) => l.line.toLowerCase().includes(kw)) : buildLogs), [buildLogs, kw])
  // 构建/部署进行中（system 行出现开始标记，或最近 2s 内有新日志）时自动切到构建日志
  const building = buildLogs.some((l) => l.stream === 'system' && /开始|部署|安装|启动/.test(l.line))
  useEffect(() => {
    if (building) setTab('build')
  }, [building])
  // 运行中条目的耗时每秒刷新（强制重渲染，不依赖 store 变化）
  const [, setTick] = useState(0)
  useEffect(() => {
    if (!entries.some((e) => e.status === 'running')) return
    const timer = setInterval(() => setTick((x) => x + 1), 1000)
    return () => clearInterval(timer)
  }, [entries])
  // 新条目追加/运行中输出增长时自动滚动到底部，追踪最新执行输出
  const lastEntry = filteredEntries[filteredEntries.length - 1]
  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: 'smooth', block: 'end' })
  }, [
    filteredEntries.length,
    lastEntry?.status,
    lastEntry?.liveOutput?.length,
    filteredBuild.length,
    tab,
  ])
  const statusColor: Record<TerminalEntry['status'], string> = {
    running: 'text-[#58a6ff]',
    done: 'text-[#3fb950]',
    error: 'text-[#f85149]',
  }
  const statusDot: Record<TerminalEntry['status'], string> = {
    running: 'bg-[#58a6ff] animate-pulse',
    done: 'bg-[#3fb950]',
    error: 'bg-[#f85149]',
  }
  const runningNow = entries.filter((e) => e.status === 'running').length
  return (
    <div className="flex flex-col h-full min-h-0">
      <div className="flex items-center justify-between px-2.5 py-1.5 border-b border-[var(--border)] shrink-0">
        <div className="flex items-center gap-1">
          <button
            onClick={() => setTab('tools')}
            className={`px-2 h-6 rounded-md text-[11px] transition-colors flex items-center gap-1.5 ${tab === 'tools' ? 'bg-[var(--bg-hover)] text-[var(--text-primary)]' : 'text-[var(--text-secondary)] hover:text-[var(--text-primary)]'}`}
          >
            <Icon name="terminal" size={12} />
            {t('home.terminal')}
            {runningNow > 0 && (
              <span className="flex items-center gap-1 text-[var(--accent)]">
                <span className="w-1 h-1 rounded-full bg-[var(--accent)] animate-pulse" />
                {runningNow}
              </span>
            )}
          </button>
          <button
            onClick={() => setTab('build')}
            className={`px-2 h-6 rounded-md text-[11px] transition-colors flex items-center gap-1.5 ${tab === 'build' ? 'bg-[var(--bg-hover)] text-[var(--text-primary)]' : 'text-[var(--text-secondary)] hover:text-[var(--text-primary)]'}`}
          >
            <Icon name="package" size={12} />
            {t('home.buildLog')}
            {buildLogs.length > 0 && (
              <span className="text-[var(--text-muted)]">{buildLogs.length}</span>
            )}
          </button>
        </div>
        <div className="flex items-center gap-1.5 ml-2 flex-1 min-w-0">
          <div className="relative flex-1 min-w-0 max-w-44">
            <Icon name="search" size={11} className="absolute left-2 top-1/2 -translate-y-1/2 text-[var(--text-muted)] pointer-events-none" />
            <input
              value={filter}
              onChange={(e) => setFilter(e.target.value)}
              placeholder={t('home.terminalFilter')}
              spellCheck={false}
              className="w-full h-6 pl-6 pr-6 rounded-md bg-[var(--bg-primary)] border border-[var(--border)] text-[10.5px] text-[var(--text-primary)] placeholder:text-[var(--text-muted)] outline-none focus:border-[var(--accent)] transition-colors"
            />
            {filter && (
              <button
                onClick={() => setFilter('')}
                className="absolute right-1 top-1/2 -translate-y-1/2 p-0.5 rounded text-[var(--text-muted)] hover:text-[var(--text-primary)]"
              >
                <Icon name="close" size={9} />
              </button>
            )}
          </div>
        </div>
        <button
          onClick={() => (tab === 'tools' ? onClear() : onClearBuild())}
          disabled={tab === 'tools' ? filteredEntries.length === 0 : filteredBuild.length === 0}
          title={t('home.terminalClear')}
          className="px-2 h-6 rounded-md text-[11px] text-[var(--text-muted)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] transition-colors disabled:opacity-30"
        >
          {t('home.terminalClear')}
        </button>
      </div>
      {tab === 'tools' ? (
        <div className="flex-1 min-h-0 overflow-y-auto bg-[#0d1117] p-2.5 font-mono text-[11.5px] leading-relaxed">
          {filteredEntries.length === 0 ? (
            <div className="h-full flex flex-col items-center justify-center gap-2 text-center px-6">
              <Icon name="terminal" size={26} className="opacity-30" />
              <span className="text-[11.5px] text-[#8b949e]">
                {kw ? t('home.terminalFilterEmpty') : t('home.terminalEmpty')}
              </span>
            </div>
          ) : (
            <div className="space-y-3">
              {filteredEntries.map((e) => (
                <div key={e.id}>
                  <div className="flex items-center gap-2">
                    <span className="text-[#3fb950] select-none">$</span>
                    <span className="text-[#e6edf3] break-all">
                      {e.tool}
                      {e.args && <span className="text-[#8b949e] ml-1">{e.args.slice(0, 200)}{e.args.length > 200 ? '…' : ''}</span>}
                    </span>
                    <span className={`w-1.5 h-1.5 rounded-full shrink-0 ${statusDot[e.status]}`} />
                    <span className={`text-[10.5px] shrink-0 ${statusColor[e.status]}`}>
                      {e.status === 'running'
                        ? t('home.taskElapsed', { time: fmtElapsed(Math.max(0, Math.floor((Date.now() - e.startedAt) / 1000))) })
                        : fmtElapsed(Math.max(0, Math.floor((e.durationMs ?? 0) / 1000)))}
                    </span>
                  </div>
                  {e.output || (e.status === 'running' && e.liveOutput) ? (
                    <pre className="mt-1 ml-4 text-[#c9d1d9] whitespace-pre-wrap break-all max-h-56 overflow-y-auto">
                      {(e.status === 'running' ? e.liveOutput ?? '' : e.output || e.liveOutput || '').slice(0, 3000)}
                      {e.status === 'running' && e.liveOutput && e.liveOutput.length > 3000 ? '\n…（实时输出过长已截断）' : ''}
                      {e.status !== 'running' && (e.output || e.liveOutput || '').length > 3000 ? '\n…（输出过长已截断）' : ''}
                    </pre>
                  ) : null}
                </div>
              ))}
            </div>
          )}
          <div ref={bottomRef} />
        </div>
      ) : (
        <div className="flex-1 min-h-0 overflow-y-auto bg-[#0d1117] p-2.5 font-mono text-[11.5px] leading-relaxed">
          {filteredBuild.length === 0 ? (
            <div className="h-full flex flex-col items-center justify-center gap-2 text-center px-6">
              <Icon name="package" size={26} className="opacity-30" />
              <span className="text-[11.5px] text-[#8b949e]">
                {kw ? t('home.terminalFilterEmpty') : t('home.buildLogEmpty')}
              </span>
            </div>
          ) : (
            <div>
              {filteredBuild.map((l, i) => {
                const hasColor = hasAnsi(l.line)
                const color =
                  hasColor
                    ? ''
                    : l.stream === 'stderr'
                      ? 'text-[#f85149]'
                      : l.stream === 'system'
                        ? 'text-[#58a6ff] font-semibold'
                        : 'text-[#c9d1d9]'
                return (
                  <div key={i} className={`whitespace-pre-wrap break-all ${color}`}>
                    {hasColor ? <AnsiText text={l.line} /> : l.line}
                  </div>
                )
              })}
            </div>
          )}
          <div ref={bottomRef} />
        </div>
      )}
    </div>
  )
}
