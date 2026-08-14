import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import type { ChatErrorDetail } from '../../stores/projectStore'
import Icon from '../../icons/Icon'
import Markdown from '../../components/Markdown'
import { gitDiffStat, gitFileDiff, gitAcceptChanges, gitRevertFile } from '../../api/git'
import { sanitizeToolMarkers } from '../chatUtils'

/** diff 文本按行着色：+ 绿 / - 红 / @@ 蓝（未跟踪新文件预览保持默认色） */
export function DiffText({ text }: { text: string }) {
  return (
    <>
      {text.split('\n').map((line, i) => {
        let cls = ''
        if (line.startsWith('+') && !line.startsWith('+++')) cls = 'text-[#3fb950]'
        else if (line.startsWith('-') && !line.startsWith('---')) cls = 'text-[#f85149]'
        else if (line.startsWith('@@')) cls = 'text-[#58a6ff]'
        return (
          <div key={i} className={cls}>
            {line || '\u00A0'}
          </div>
        )
      })}
    </>
  )
}

/* ============ 思考过程（推理模型 reasoning 折叠展示） ============ */
export function ThinkingBlock({ content }: { content: string }) {
  const { t } = useTranslation()
  const [open, setOpen] = useState(false)
  const lines = content.split('\n').filter((l) => l.trim())
  const preview = lines.slice(0, 3).join(' ').slice(0, 60)
  return (
    <div className={`thinking-block ${open ? 'open' : ''}`}>
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        className="thinking-block-head"
        title={t('home.toggleThinking')}
      >
        <span className="thinking-block-icon">
          <Icon name="spark" size={11} />
        </span>
        <span className="text-[11px] font-medium">{t('home.thinking')}</span>
        {!open && preview && <span className="thinking-block-preview">{preview}…</span>}
        <Icon name="chevron-right" size={12} className={`thinking-block-caret ${open ? 'rotate-90' : ''}`} />
      </button>
      {open && (
        <div className="thinking-block-content">
          <Markdown className="md-body-sm">{content}</Markdown>
        </div>
      )}
    </div>
  )
}

/* 点赞/点踩内联图标（Icon 组件无对应图样） */
export function ThumbUpIcon({ filled }: { filled?: boolean }) {
  return (
    <svg width="13" height="13" viewBox="0 0 24 24" fill={filled ? 'currentColor' : 'none'} stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <path d="M7 10v12" />
      <path d="M15 5.88 14 10h5.83a2 2 0 0 1 1.92 2.56l-2.33 8A2 2 0 0 1 17.5 22H4a2 2 0 0 1-2-2v-8a2 2 0 0 1 2-2h2.76a2 2 0 0 0 1.79-1.11L12 2a3.13 3.13 0 0 1 3 3.88Z" />
    </svg>
  )
}

export function ThumbDownIcon({ filled }: { filled?: boolean }) {
  return (
    <svg width="13" height="13" viewBox="0 0 24 24" fill={filled ? 'currentColor' : 'none'} stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <path d="M17 14V2" />
      <path d="M9 18.12 10 14H4.17a2 2 0 0 1-1.92-2.56l2.33-8A2 2 0 0 1 6.5 2H20a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2h-2.76a2 2 0 0 0-1.79 1.11L12 22a3.13 3.13 0 0 1-3-3.88Z" />
    </svg>
  )
}

/* ============ 修改文件折叠卡片（ChatGPT 式：正文下方列出本次修改的文件） ============ */
/** 变更审查卡片：修改文件列表 + 逐文件 diff/接受/还原（Qoder 式变更审核） */
export function ModifiedFilesCard({ files, projectPath }: { files: string[]; projectPath?: string }) {
  const { t } = useTranslation()
  const [open, setOpen] = useState(false)
  const [copied, setCopied] = useState<string | null>(null)
  // 变更统计（+N/-M）：挂载时对文件列表拉取增删行数（ChatGPT 式“N 个文件已更改 +N -M”）
  const [stat, setStat] = useState<{ files: number; insertions: number; deletions: number } | null>(null)
  useEffect(() => {
    if (!projectPath || files.length === 0) {
      setStat(null)
      return
    }
    let alive = true
    gitDiffStat(projectPath, files)
      .then((s) => {
        if (alive) setStat(s)
      })
      .catch(() => {
        if (alive) setStat(null)
      })
    return () => {
      alive = false
    }
  }, [projectPath, files])
  // 变更审查状态：file -> diff 文本（undefined=未加载 / null=已加载为空）
  const [diffMap, setDiffMap] = useState<Record<string, string | null>>({})
  const [diffLoading, setDiffLoading] = useState<string | null>(null)
  const [busy, setBusy] = useState<string | null>(null)
  const [accepted, setAccepted] = useState<Set<string>>(new Set())
  const [reverted, setReverted] = useState<Set<string>>(new Set())
  const [errorMsg, setErrorMsg] = useState<string | null>(null)
  const canReview = !!projectPath

  const copyPath = async (p: string) => {
    try {
      await navigator.clipboard.writeText(p)
      setCopied(p)
      setTimeout(() => setCopied((c) => (c === p ? null : c)), 1200)
    } catch {
      // 剪贴板不可用时静默失败
    }
  }

  /** 加载单文件 diff（已加载过直接复用） */
  const loadDiff = async (p: string) => {
    if (!projectPath || diffMap[p] !== undefined) return
    setDiffLoading(p)
    try {
      const d = await gitFileDiff(projectPath, p)
      setDiffMap((m) => ({ ...m, [p]: d }))
    } catch (e) {
      setDiffMap((m) => ({ ...m, [p]: String(e) }))
    } finally {
      setDiffLoading(null)
    }
  }

  /** 接受变更：git add 单文件 */
  const acceptFile = async (p: string) => {
    if (!projectPath || busy) return
    setBusy(p)
    try {
      await gitAcceptChanges(projectPath, [p])
      setAccepted((s) => new Set(s).add(p))
    } catch (e) {
      setErrorMsg(String(e))
    } finally {
      setBusy(null)
    }
  }

  /** 还原变更：已跟踪文件丢弃改动（未跟踪新文件后端拒绝） */
  const revertFile = async (p: string) => {
    if (!projectPath || busy) return
    if (!window.confirm(t('home.revertConfirm', { file: p }))) return
    setBusy(p)
    try {
      await gitRevertFile(projectPath, p)
      setReverted((s) => new Set(s).add(p))
      setDiffMap((m) => ({ ...m, [p]: null }))
    } catch (e) {
      setErrorMsg(String(e))
    } finally {
      setBusy(null)
    }
  }

  /** 全部接受：未处理文件批量 git add */
  const acceptAll = async () => {
    if (!projectPath || busy) return
    const pending = files.filter((f) => !accepted.has(f) && !reverted.has(f))
    if (!pending.length) return
    setBusy('__all__')
    try {
      await gitAcceptChanges(projectPath, pending)
      setAccepted((s) => new Set([...s, ...pending]))
    } catch (e) {
      setErrorMsg(String(e))
    } finally {
      setBusy(null)
    }
  }

  return (
    <div className="mt-2 rounded-xl border border-[var(--border)] bg-[var(--bg-window)]/60 overflow-hidden">
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        className="w-full flex items-center gap-1.5 px-2.5 py-1.5 text-left hover:bg-[var(--bg-hover)] transition-colors"
        title={t('home.modifiedFiles', { n: files.length })}
      >
        <Icon name="file" size={12} className="shrink-0 text-[var(--text-secondary)]" />
        <span className="text-[11px] text-[var(--text-secondary)] font-medium">
          {t('home.modifiedFiles', { n: files.length })}
        </span>
        {stat && (stat.insertions > 0 || stat.deletions > 0) && (
          <span className="text-[10px] tabular-nums shrink-0">
            <span className="text-[var(--success)]">+{stat.insertions}</span>
            <span className="text-[var(--danger)]"> −{stat.deletions}</span>
          </span>
        )}
        {canReview && accepted.size > 0 && (
          <span className="text-[10px] px-1.5 py-px rounded bg-[var(--success)]/10 text-[var(--success)]">
            {t('home.changesAccepted', { n: accepted.size })}
          </span>
        )}
        <Icon
          name="chevron-right"
          size={11}
          className={`ml-auto shrink-0 text-[var(--text-muted)] transition-transform ${open ? 'rotate-90' : ''}`}
        />
      </button>
      {open && (
        <div className="max-h-64 overflow-y-auto border-t border-[var(--border)] py-1">
          {files.map((p) => {
            const isAccepted = accepted.has(p)
            const isReverted = reverted.has(p)
            const diff = diffMap[p]
            return (
              <div key={p} className="px-3 py-1 hover:bg-[var(--bg-hover)] transition-colors">
                <div className="flex items-center gap-1.5">
                  <button
                    type="button"
                    onClick={() => copyPath(p)}
                    className="flex-1 min-w-0 flex items-center gap-1.5 text-left"
                    title={copied === p ? t('home.copied') : `${t('home.copyPath')}：${p}`}
                  >
                    <Icon name="file" size={10} className="shrink-0 opacity-40" />
                    <span className="flex-1 min-w-0 truncate text-[10.5px] font-mono text-[var(--text-secondary)]">{p}</span>
                    <Icon
                      name="copy"
                      size={10}
                      className={`shrink-0 ${copied === p ? 'text-[var(--success)]' : 'opacity-0 group-hover:opacity-60 text-[var(--text-muted)]'}`}
                    />
                  </button>
                  {canReview && !isReverted && (
                    <span className="flex items-center gap-0.5 shrink-0">
                      <button
                        type="button"
                        onClick={() => loadDiff(p)}
                        disabled={diffLoading === p}
                        title={t('home.viewDiff')}
                        className="p-1 rounded-md text-[var(--text-muted)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] transition-colors"
                      >
                        {diffLoading === p ? (
                          <span className="inline-block w-2.5 h-2.5 rounded-full border border-[var(--text-muted)] border-t-transparent animate-spin align-middle" />
                        ) : (
                          <Icon name="git-branch" size={11} />
                        )}
                      </button>
                      <button
                        type="button"
                        onClick={() => acceptFile(p)}
                        disabled={busy === p || isAccepted}
                        title={isAccepted ? t('home.changeAccepted') : t('home.acceptChange')}
                        className={`p-1 rounded-md transition-colors ${
                          isAccepted
                            ? 'text-[var(--success)]'
                            : 'text-[var(--text-muted)] hover:text-[var(--success)] hover:bg-[var(--success)]/10'
                        }`}
                      >
                        <Icon name="check" size={11} />
                      </button>
                      <button
                        type="button"
                        onClick={() => revertFile(p)}
                        disabled={busy === p || isAccepted}
                        title={t('home.revertChange')}
                        className="p-1 rounded-md text-[var(--text-muted)] hover:text-[var(--danger)] hover:bg-[var(--danger)]/10 transition-colors"
                      >
                        <Icon name="close" size={11} />
                      </button>
                    </span>
                  )}
                </div>
                {isReverted && <div className="text-[10px] text-[var(--danger)] mt-0.5 pl-4">{t('home.changeReverted')}</div>}
                {diff && !isReverted && (
                  <pre className="mt-1 ml-4 mr-1 rounded-lg bg-[var(--bg-primary)] border border-[var(--border)] p-2 text-[10.5px] font-mono whitespace-pre-wrap break-all leading-relaxed max-h-40 overflow-y-auto">
                    <DiffText text={diff} />
                  </pre>
                )}
              </div>
            )
          })}
          {errorMsg && <div className="px-3 py-1.5 text-[10.5px] text-[var(--danger)]">{errorMsg}</div>}
          {canReview && files.length > 1 && (
            <div className="flex items-center justify-end px-3 py-1.5 border-t border-[var(--border)]">
              <button
                type="button"
                onClick={acceptAll}
                disabled={busy !== null}
                className="h-6 px-2.5 rounded-md bg-[var(--success)]/12 text-[var(--success)] text-[10.5px] font-medium hover:bg-[var(--success)]/20 disabled:opacity-40 transition-colors"
              >
                {busy === '__all__' ? t('home.changesAccepting') : t('home.acceptAllChanges')}
              </button>
            </div>
          )}
        </div>
      )}
    </div>
  )
}

/* ============ 流式回复（打字机效果 + 思考过程） ============ */
export function StreamingMessage({ content, reasoning }: { content: string; reasoning: string }) {
  const { t } = useTranslation()
  // 流式渲染节流：高频 delta 先缓存，每 60ms 才同步一次到渲染副本。
  // 避免每个 token 都触发重型 Markdown 全量解析（长回复时渲染跟不上会卡顿，显得响应慢）
  const [display, setDisplay] = useState({ content, reasoning })
  useEffect(() => {
    const timer = setTimeout(() => setDisplay({ content, reasoning }), 60)
    return () => clearTimeout(timer)
  }, [content, reasoning])
  const shown = display
  return (
    <div className="flex gap-2.5 animate-fade-in-up">
      <div className="w-7 h-7 rounded-lg bg-gradient-to-br from-[var(--accent)] to-[#8b5cf6] flex items-center justify-center shrink-0 mt-0.5 shadow-md shadow-[var(--accent)]/20">
        <Icon name="spark" size={13} white />
      </div>
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-2 mb-1.5">
          <span className="text-[11px] font-medium text-[var(--text-secondary)]">{t('home.agent')}</span>
          <span className="text-[10px] text-[var(--text-muted)] flex items-center gap-1">
            <span className="inline-flex gap-0.5">
              <span className="w-1 h-1 rounded-full bg-[var(--accent)] animate-bounce" style={{ animationDelay: '0ms' }} />
              <span className="w-1 h-1 rounded-full bg-[var(--accent)] animate-bounce" style={{ animationDelay: '150ms' }} />
              <span className="w-1 h-1 rounded-full bg-[var(--accent)] animate-bounce" style={{ animationDelay: '300ms' }} />
            </span>
            {t('home.typing')}
          </span>
        </div>
        {shown.reasoning && <ThinkingBlock content={shown.reasoning} />}
        {shown.content.trim() ? (
          <div className="text-sm break-words leading-relaxed text-[var(--text-primary)]">
            <Markdown>{sanitizeToolMarkers(shown.content)}</Markdown>
            <span className="typing-cursor" />
          </div>
        ) : (
          <div className="text-sm text-[var(--text-muted)] italic">{t('home.thinkingShort')}</div>
        )}
      </div>
    </div>
  )
}

/* ============ 结构化错误卡片（chat-error 事件友好展示） ============ */
export function ErrorCard({
  error,
  detail,
  onRetry,
  retryLabel,
}: {
  error: string
  detail: ChatErrorDetail | null
  onRetry: () => void
  retryLabel: string
}) {
  const { t } = useTranslation()
  // 按错误分类着色：认证/请求被拒=红；限流/超时/网络/服务端=橙；上下文超长=黄；其余=红
  const color =
    detail?.kind === 'rate_limited' || detail?.kind === 'timeout' || detail?.kind === 'network' || detail?.kind === 'server'
      ? 'var(--warning)'
      : detail?.kind === 'context_overflow'
        ? 'var(--warning)'
        : 'var(--danger)'
  const showRetry = !detail || detail.retryable
  return (
    <div
      className="rounded-xl border px-3.5 py-3 text-[12px]"
      style={{ borderColor: `${color}55`, backgroundColor: `${color}1a`, color }}
    >
      <div className="flex items-start gap-2">
        <Icon name="info" size={14} className="shrink-0 mt-0.5" />
        <div className="flex-1 min-w-0 space-y-1">
          {detail ? (
            <>
              <p className="font-semibold leading-snug">{detail.title}</p>
              <p className="break-all leading-snug opacity-90">{detail.reason}</p>
              {detail.suggestion && (
                <p className="leading-snug opacity-75">{t('home.suggestion', { text: detail.suggestion })}</p>
              )}
            </>
          ) : (
            <p className="break-all leading-snug">{error}</p>
          )}
        </div>
        {showRetry && (
          <button
            onClick={onRetry}
            className="shrink-0 h-7 px-3 rounded-lg text-white text-[11px] font-medium hover:opacity-90 active:scale-95 transition-all"
            style={{ backgroundColor: color }}
          >
            {retryLabel}
          </button>
        )}
      </div>
    </div>
  )
}

/* ============ 空状态：未添加项目 ============ */
export function EmptyState({ onAdd }: { onAdd: () => void }) {
  const { t } = useTranslation()
  return (
    <div className="h-full flex flex-col items-center justify-center text-center">
      <div className="relative mb-7">
        <div className="absolute inset-0 blur-3xl bg-[var(--accent)]/25 rounded-full scale-150" />
        <div className="relative w-16 h-16 rounded-2xl bg-gradient-to-br from-[var(--accent)] to-[#8b5cf6] flex items-center justify-center shadow-2xl shadow-[var(--accent)]/30">
          <Icon name="spark" size={30} white />
        </div>
      </div>
      <h2 className="text-lg font-semibold">{t('home.welcome')}</h2>
      <p className="text-[13px] text-[var(--text-secondary)] mt-1.5 max-w-sm leading-relaxed">{t('home.welcomeDesc')}</p>
      <button
        onClick={onAdd}
        className="mt-7 h-10 px-5 rounded-[10px] bg-[var(--accent)] text-white text-[13px] font-medium flex items-center gap-1.5 hover:bg-[var(--accent-hover)] active:scale-[0.98] transition-all shadow-lg shadow-[var(--accent)]/20"
      >
        <Icon name="plus" size={15} white /> {t('home.addProject')}
      </button>
    </div>
  )
}

/* ============ 空状态：已选项目、无消息 ============ */
export function ChatEmptyState({ onQuick }: { onQuick: (text: string) => void }) {
  const { t } = useTranslation()

  const quickActions = [
    {
      icon: 'bolt' as const,
      title: t('home.quickBuild'),
      desc: t('home.quickBuildDesc'),
      prompt: t('home.quickBuildPrompt'),
    },
    {
      icon: 'devices' as const,
      title: t('home.quickDeploy'),
      desc: t('home.quickDeployDesc'),
      prompt: t('home.quickDeployPrompt'),
    },
    {
      icon: 'add-circle' as const,
      title: t('home.quickPage'),
      desc: t('home.quickPageDesc'),
      prompt: t('home.quickPagePrompt'),
    },
  ]

  return (
    <div className="h-full flex flex-col items-center justify-center text-center">
      <div className="relative mb-6">
        <div className="absolute inset-0 blur-3xl bg-[var(--accent)]/20 rounded-full scale-150" />
        <div className="relative w-14 h-14 rounded-2xl bg-gradient-to-br from-[var(--accent)] to-[#8b5cf6] flex items-center justify-center shadow-xl shadow-[var(--accent)]/25">
          <Icon name="spark" size={26} white />
        </div>
      </div>
      <h2 className="text-lg font-semibold">{t('home.startNewChat')}</h2>
      <p className="text-[13px] text-[var(--text-secondary)] mt-1.5 max-w-sm leading-relaxed">{t('home.chatHint')}</p>

      {/* 快捷操作 */}
      <div className="grid grid-cols-3 gap-3 mt-8 w-full max-w-xl animate-fade-in-up">
        {quickActions.map((a, i) => (
          <button
            key={a.title}
            onClick={() => onQuick(a.prompt)}
            style={{ animationDelay: `${i * 60}ms` }}
            className="group p-3.5 rounded-xl border border-[var(--border)] bg-[var(--bg-secondary)]/60 text-left hover:border-[var(--accent)]/40 hover:bg-[var(--bg-card)] hover:-translate-y-0.5 hover:shadow-lg hover:shadow-[var(--accent)]/5 active:translate-y-0 transition-all duration-200"
          >
            <div className="w-9 h-9 rounded-[10px] bg-[var(--accent-soft)] flex items-center justify-center mb-2.5 group-hover:bg-[var(--accent)] group-hover:shadow-md group-hover:shadow-[var(--accent)]/25 transition-all">
              <Icon name={a.icon} size={17} className="transition-all group-hover:[filter:brightness(0)_invert(1)]!" />
            </div>
            <div className="text-[13px] font-medium text-[var(--text-primary)]">{a.title}</div>
            <div className="text-[11px] text-[var(--text-muted)] mt-0.5 leading-relaxed">{a.desc}</div>
          </button>
        ))}
      </div>

      {/* 示例提示词 */}
      <div className="mt-8 flex items-center gap-2 flex-wrap justify-center">
        <span className="text-[11px] text-[var(--text-muted)]">{t('home.tryExamples')}</span>
        <button
          onClick={() => onQuick(t('home.example1'))}
          className="px-3 py-1.5 rounded-full text-[11px] text-[var(--text-secondary)] bg-[var(--bg-card)] border border-[var(--border)] hover:text-[var(--accent)] hover:border-[var(--accent)]/40 transition-colors"
        >
          {t('home.example1')}
        </button>
        <button
          onClick={() => onQuick(t('home.example2'))}
          className="px-3 py-1.5 rounded-full text-[11px] text-[var(--text-secondary)] bg-[var(--bg-card)] border border-[var(--border)] hover:text-[var(--accent)] hover:border-[var(--accent)]/40 transition-colors"
        >
          {t('home.example2')}
        </button>
      </div>
    </div>
  )
}
