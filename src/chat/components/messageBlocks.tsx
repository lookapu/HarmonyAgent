import { memo, useEffect, useMemo, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import type { ChatErrorDetail } from '../../stores/projectStore'
import Icon from '../../icons/Icon'
import Markdown from '../../components/Markdown'
import { gitDiffStat, gitFileDiff, gitAcceptChanges, gitRevertFile } from '../../api/git'
import { sanitizeToolMarkers } from '../chatUtils'
import { createPortal } from 'react-dom'
import { getItem, setItem } from '../../utils/storage'
import { STORAGE_KEYS } from '../../constants'
import { Button } from '../../components/ui/Button'

/** 流式补全未闭合的代码围栏：``` 未配对时补一个闭合，否则 react-markdown 把整块当纯文本，
 *  代码块刚开头时裸露 ``` 符号闪烁；只影响展示层，不改真实内容 */
function closeOpenFence(md: string): string {
  if ((md.match(/```/g) ?? []).length % 2 === 1) return md + `\n` + '```'
  return md
}

/** 流式过程中，模型可能正在写入工具标记（如结尾停在“【TO”“【TOOL|read_file|{”），
 *  sanitizeToolMarkers 只能清理结构完整的标记，未写完整的标记前缀会作为正文碎片闪烁。
 *  仅在“最后一个【TOOL 起始符之后再无结束符】且其后内容看起来是标记体（无句号/段落边界）”时，
 *  从该前缀起截断展示（真实内容不变，下一块到达后会重新计算）。 */
function hideIncompleteToolMarker(md: string): string {
  const lastStart = md.lastIndexOf('【TOOL')
  if (lastStart < 0) return md
  const tail = md.slice(lastStart)
  // 已有完整标记结束符就交给 sanitizeToolMarkers，不在此处理
  if (tail.includes('】') || tail.includes(']}')) return md
  // 标记前缀之后出现了明显的正文结束标点/段落，说明【TOOL 可能是正文内容而非标记，保留
  if (/[。！？\n]/.test(tail)) return md
  // 标记体过长（超过 400 字符仍无结束符），多半是正文，不再截断避免误删
  if (tail.length > 400) return md
  return md.slice(0, lastStart)
}

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
export const ThinkingBlock = memo(function ThinkingBlock({ content }: { content: string }) {
  const { t } = useTranslation()
  // 展开偏好记忆：用户手动开合后跨会话记住（localStorage）
  const [open, setOpen] = useState(() => getItem(STORAGE_KEYS.THINKING_OPEN) === '1')
  const toggle = () => {
    setOpen((v) => {
      const next = !v
      setItem(STORAGE_KEYS.THINKING_OPEN, next ? '1' : '0')
      return next
    })
  }
  const lines = useMemo(() => content.split('\n').filter((l) => l.trim()), [content])
  const preview = useMemo(() => lines.slice(0, 3).join(' ').slice(0, 60), [lines])
  return (
    <div className={`thinking-block ${open ? 'open' : ''}`}>
      <button
        type="button"
        onClick={toggle}
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
})

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
export const ModifiedFilesCard = memo(function ModifiedFilesCard({ files, projectPath }: { files: string[]; projectPath?: string }) {
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
  // 已展开 diff 的文件集合（与 diffMap 分离：已加载 ≠ 展开中，再次点击可收缩）
  const [openDiffs, setOpenDiffs] = useState<Set<string>>(new Set())
  // 独立审核窗口开关（portal 到 document.body，不受父容器裁剪/层级影响）
  const [reviewOpen, setReviewOpen] = useState(false)
  // 审核窗口 Esc 关闭（与文件预览弹窗行为一致）
  useEffect(() => {
    if (!reviewOpen) return
    const handler = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setReviewOpen(false)
    }
    window.addEventListener('keydown', handler)
    return () => window.removeEventListener('keydown', handler)
  }, [reviewOpen])
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

  /** 切换单文件 diff：展开中再点收缩；未加载则拉取后展开 */
  const toggleDiff = async (p: string) => {
    if (!projectPath) return
    if (openDiffs.has(p)) {
      setOpenDiffs((s) => {
        const n = new Set(s)
        n.delete(p)
        return n
      })
      return
    }
    setOpenDiffs((s) => new Set(s).add(p))
    if (diffMap[p] === undefined) {
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
        {/* 审核窗口入口：stopPropagation 避免误触卡片展开/收缩 */}
        {canReview && files.length > 0 && (
          <button
            type="button"
            onClick={(e) => {
              e.stopPropagation()
              setReviewOpen(true)
            }}
            title={t('home.reviewChanges')}
            className="shrink-0 h-5 px-1.5 rounded-md text-[10px] font-medium text-[var(--text-secondary)] hover:text-[var(--accent)] hover:bg-[var(--bg-hover)] transition-colors flex items-center gap-1"
          >
            <Icon name="git-branch" size={10} />
            {t('home.reviewChanges')}
          </button>
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
                        onClick={() => toggleDiff(p)}
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
                      {/* 危险操作分隔线：还原会丢弃改动，与常规操作拉开视觉距离，降低误触 */}
                      <span className="mx-0.5 w-px h-3 bg-[var(--border)] shrink-0" aria-hidden="true" />
                      <button
                        type="button"
                        onClick={() => revertFile(p)}
                        disabled={busy === p || isAccepted}
                        title={t('home.revertChange')}
                        className="p-1 ml-0.5 rounded-md text-[var(--text-muted)] hover:text-[var(--danger)] hover:bg-[var(--danger)]/10 transition-colors"
                      >
                        <Icon name="close" size={11} />
                      </button>
                    </span>
                  )}
                </div>
                {isReverted && <div className="text-[10px] text-[var(--danger)] mt-0.5 pl-4">{t('home.changeReverted')}</div>}
                {diff && !isReverted && openDiffs.has(p) && (
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

      {/* 独立审核窗口：portal 到 body 顶层，全屏遮罩 + 文件列表逐项审查 */}
      {reviewOpen && canReview &&
        createPortal(
          <div
            className="fixed inset-0 z-[var(--app-z-modal)] flex items-center justify-center bg-black/50 backdrop-blur-[2px] p-6"
            onMouseDown={(e) => {
              if (e.target === e.currentTarget) setReviewOpen(false)
            }}
          >
            <div className="w-[860px] max-w-[95vw] h-[82vh] max-h-[82vh] rounded-2xl border border-[var(--border)] bg-[var(--bg-secondary)] shadow-2xl flex flex-col overflow-hidden animate-modal-in">
              {/* 头部 */}
              <div className="h-12 shrink-0 px-4 flex items-center gap-3 border-b border-[var(--border)]">
                <div className="w-7 h-7 rounded-lg bg-[var(--accent-soft)] flex items-center justify-center shrink-0">
                  <Icon name="git-branch" size={13} />
                </div>
                <div className="min-w-0 flex-1">
                  <div className="text-[13px] font-medium truncate leading-tight">{t('home.reviewChanges')}</div>
                  <div className="text-[10px] text-[var(--text-muted)] truncate">{t('home.reviewDesc')}</div>
                </div>
                <span className="shrink-0 text-[10px] text-[var(--text-muted)] tabular-nums bg-[var(--bg-card)] rounded-md px-2 py-1">
                  {t('home.changesAccepted', { n: accepted.size })} / {files.length}
                </span>
                <button
                  onClick={() => setReviewOpen(false)}
                  className="shrink-0 w-7 h-7 rounded-lg flex items-center justify-center text-[var(--text-muted)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] transition-colors"
                  title={t('common.close')}
                >
                  <Icon name="close" size={14} />
                </button>
              </div>
              {/* 文件列表 */}
              <div className="flex-1 min-h-0 overflow-y-auto p-3 space-y-2">
                {files.map((p) => {
                  const isAccepted = accepted.has(p)
                  const isReverted = reverted.has(p)
                  const diffOpen = openDiffs.has(p)
                  const diff = diffMap[p]
                  return (
                    <div key={p} className="rounded-lg border border-[var(--border)] bg-[var(--bg-primary)] overflow-hidden">
                      <div className="flex items-center gap-1.5 px-2.5 py-1.5">
                        <button
                          type="button"
                          onClick={() => toggleDiff(p)}
                          className="flex-1 min-w-0 flex items-center gap-1.5 text-left"
                          title={t('home.viewDiff')}
                        >
                          <Icon name="file" size={11} className="shrink-0 opacity-40" />
                          <span className="flex-1 min-w-0 truncate text-[11px] font-mono text-[var(--text-secondary)]">{p}</span>
                          <Icon
                            name="chevron-right"
                            size={10}
                            className={`shrink-0 text-[var(--text-muted)] transition-transform ${diffOpen ? 'rotate-90' : ''}`}
                          />
                        </button>
                        {isAccepted && (
                          <span className="shrink-0 text-[9.5px] px-1.5 py-px rounded bg-[var(--success)]/10 text-[var(--success)]">
                            {t('home.changeAccepted')}
                          </span>
                        )}
                        {isReverted && (
                          <span className="shrink-0 text-[9.5px] px-1.5 py-px rounded bg-[var(--danger)]/10 text-[var(--danger)]">
                            {t('home.changeReverted')}
                          </span>
                        )}
                        {!isReverted && (
                          <span className="flex items-center gap-0.5 shrink-0">
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
                            <span className="mx-0.5 w-px h-3 bg-[var(--border)] shrink-0" aria-hidden="true" />
                            <button
                              type="button"
                              onClick={() => revertFile(p)}
                              disabled={busy === p || isAccepted}
                              title={t('home.revertChange')}
                              className="p-1 ml-0.5 rounded-md text-[var(--text-muted)] hover:text-[var(--danger)] hover:bg-[var(--danger)]/10 transition-colors"
                            >
                              <Icon name="close" size={11} />
                            </button>
                          </span>
                        )}
                      </div>
                      {diffOpen && !isReverted && (
                        <div className="px-3 pb-2">
                          {diffLoading === p ? (
                            <div className="flex items-center gap-2 text-[10.5px] text-[var(--text-muted)] py-2">
                              <span className="inline-block w-2.5 h-2.5 rounded-full border border-[var(--text-muted)] border-t-transparent animate-spin" />
                              {t('common.loading')}
                            </div>
                          ) : diff ? (
                            <pre className="rounded-lg bg-[var(--bg-window)] border border-[var(--border)] p-2 text-[10.5px] font-mono whitespace-pre-wrap break-all leading-relaxed max-h-72 overflow-y-auto">
                              <DiffText text={diff} />
                            </pre>
                          ) : null}
                        </div>
                      )}
                    </div>
                  )
                })}
                {errorMsg && <div className="px-3 py-1.5 text-[10.5px] text-[var(--danger)]">{errorMsg}</div>}
              </div>
              {/* 底部：全部接受 */}
              {files.length > 1 && (
                <div className="shrink-0 px-4 py-2.5 border-t border-[var(--border)] flex items-center justify-end">
                  <button
                    type="button"
                    onClick={acceptAll}
                    disabled={busy !== null}
                    className="h-7 px-3.5 rounded-lg bg-[var(--success)]/12 text-[var(--success)] text-[11px] font-medium hover:bg-[var(--success)]/20 disabled:opacity-40 transition-colors"
                  >
                    {busy === '__all__' ? t('home.changesAccepting') : t('home.acceptAllChanges')}
                  </button>
                </div>
              )}
            </div>
          </div>,
          document.body,
        )}
    </div>
  )
})

/* ============ 流式回复（打字机效果 + 思考过程） ============ */
export const StreamingMessage = memo(function StreamingMessage({
  content,
  reasoning,
  speed = 1,
  active = true,
}: {
  content: string
  reasoning: string
  speed?: number
  active?: boolean
}) {
  const { t } = useTranslation()
  // 流式渲染节流：高频 delta 先缓存，按内容长度自适应间隔（短文本 60ms，长文本降频），
  // 避免每个 token 都触发重型 Markdown 全量解析（长回复时渲染跟不上会卡顿，显得响应慢）
  const [display, setDisplay] = useState({ content, reasoning })
  // 节流（throttle）而非防抖：delta 到达后缓存最新值，距上次渲染超过间隔才刷新。
  // 防抖（clearTimeout 重排）在 delta 间隔小于延迟时会不断重置计时器，
  // 内容迟迟不渲染，直到流停顿才"跳"出一大段（快速流/本地模型场景明显）
  const latestRef = useRef({ content, reasoning })
  const rafRef = useRef<number | null>(null)
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  const lastFlushRef = useRef(0)
  useEffect(() => {
    latestRef.current = { content, reasoning }
    // 速度倍率：0.5x=慢、1x=正常、2x=快、4x=极速（节流间隔 × 倍率 = 实际间隔）
    const baseDelay = content.length > 8000 ? 220 : content.length > 3000 ? 130 : 60
    const delay = Math.max(8, Math.round(baseDelay * speed))
    if (performance.now() - lastFlushRef.current >= delay) {
      lastFlushRef.current = performance.now()
      setDisplay(latestRef.current)
      return
    }
    // 间隔内到达的更新：rAF 合并到帧边界，再排一个尾随刷新（保证流停顿后内容不滞留）
    if (rafRef.current == null) {
      rafRef.current = requestAnimationFrame(() => {
        rafRef.current = null
        if (timerRef.current == null) {
          timerRef.current = setTimeout(() => {
            timerRef.current = null
            lastFlushRef.current = performance.now()
            setDisplay(latestRef.current)
          }, delay)
        }
      })
    }
  }, [content, reasoning, speed])
  // 卸载时清理 rAF 与尾随计时器
  useEffect(
    () => () => {
      if (rafRef.current != null) cancelAnimationFrame(rafRef.current)
      if (timerRef.current != null) clearTimeout(timerRef.current)
    },
    [],
  )
  const shown = display
  // 缓存 sanitizeToolMarkers + closeOpenFence 结果：节流渲染时避免重复正则处理长文本
  const processedContent = useMemo(
    () => hideIncompleteToolMarker(closeOpenFence(sanitizeToolMarkers(shown.content))),
    [shown.content],
  )
  return (
    <div className="flex gap-2.5 msg-row msg-role msg-role-agent">
      <div className="w-7 h-7 rounded-lg bg-gradient-to-br from-[var(--role-agent)] to-[#6d28d9] flex items-center justify-center shrink-0 mt-0.5 shadow-md shadow-[var(--role-agent)]/25">
        <Icon name="spark" size={13} white />
      </div>
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-2 mb-1.5">
          <span className="role-tag role-tag-agent">{t('home.agent')}</span>
          {active && (
            <span className="text-[10px] text-[var(--text-muted)] flex items-center gap-1 tnum">
              <span className="inline-flex gap-0.5">
                <span className="w-1 h-1 rounded-full bg-[var(--role-agent)] animate-bounce" style={{ animationDelay: '0ms' }} />
                <span className="w-1 h-1 rounded-full bg-[var(--role-agent)] animate-bounce" style={{ animationDelay: '150ms' }} />
                <span className="w-1 h-1 rounded-full bg-[var(--role-agent)] animate-bounce" style={{ animationDelay: '300ms' }} />
              </span>
              {t('home.typing')}
            </span>
          )}
          <span className="msg-timestamp ml-auto tnum">{new Date().toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}</span>
        </div>
        {shown.reasoning && <ThinkingBlock content={shown.reasoning} />}
        {shown.content.trim() ? (
          <div className="text-sm break-words leading-relaxed text-[var(--text-primary)]">
            <Markdown streaming={active}>{processedContent}</Markdown>
            {active && <span className="typing-cursor" />}
          </div>
        ) : active ? (
          <div className="text-sm text-[var(--text-muted)] italic">{t('home.thinkingShort')}</div>
        ) : null}
      </div>
    </div>
  )
})

/* ============ 结构化错误卡片（chat-error 事件友好展示） ============ */
export const ErrorCard = memo(function ErrorCard({
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
      className="px-3 py-2.5 text-[12px] animate-fade-in-up rounded-lg border border-[var(--border)] bg-[var(--bg-secondary)]/60"
      style={{ color }}
    >
      <div className="flex items-start gap-2">
        <Icon name="info" size={13} className="shrink-0 mt-0.5" />
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
})

/* ============ 空状态：未添加项目 ============
 * 顶部：欢迎 + 添加项目主 CTA
 * 下方：3 个"快速操作"卡（导入会话/查看审计/打开成本页）—— 让新用户不用看完文档就能上手 */
export const EmptyState = memo(function EmptyState({
  onAdd,
  onImport,
  onAudit,
  onCost,
}: {
  onAdd: () => void
  onImport?: () => void
  onAudit?: () => void
  onCost?: () => void
}) {
  const { t } = useTranslation()
  const secondaryActions = [
    onImport && { icon: 'file' as const, title: t('home.quickImport'), desc: t('home.quickImportDesc'), onClick: onImport },
    onAudit && { icon: 'history' as const, title: t('home.quickAudit'), desc: t('home.quickAuditDesc'), onClick: onAudit },
    onCost && { icon: 'receipt' as const, title: t('home.quickCost'), desc: t('home.quickCostDesc'), onClick: onCost },
  ].filter((x): x is NonNullable<typeof x> => Boolean(x))
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
      {/* Button 基类的 shrink-0 在这里是承重墙：外层容器矮视口下会溢出，删了它 CTA 会被压到 21.5px（比一行文字还矮） */}
      <Button variant="primary" size="md" icon="plus" className="mt-7" onClick={onAdd}>
        {t('home.addProject')}
      </Button>
      {secondaryActions.length > 0 && (
        <div className="mt-8 grid grid-cols-3 gap-2.5 w-full max-w-md animate-fade-in-up">
          {secondaryActions.map((a, i) => (
            <button
              key={a.title}
              onClick={a.onClick}
              style={{ animationDelay: `${i * 60}ms` }}
              className="group p-2.5 rounded-lg border border-[var(--border)] bg-[var(--bg-secondary)]/60 text-left hover:border-[var(--accent)]/40 hover:bg-[var(--bg-card)] hover:-translate-y-0.5 hover:shadow-md transition-all duration-200"
            >
              <div className="w-7 h-7 rounded-md bg-[var(--accent-soft)] flex items-center justify-center mb-1.5 group-hover:bg-[var(--accent)] transition-all">
                <Icon name={a.icon} size={13} className="group-hover:[filter:brightness(0)_invert(1)]!" />
              </div>
              <div className="text-[12px] font-medium text-[var(--text-primary)]">{a.title}</div>
              <div className="text-[10px] text-[var(--text-muted)] mt-0.5 leading-snug line-clamp-2">{a.desc}</div>
            </button>
          ))}
        </div>
      )}
    </div>
  )
})

/* ============ 空状态：已选项目、无消息 ============ */
export const ChatEmptyState = memo(function ChatEmptyState({ onQuick }: { onQuick: (text: string) => void }) {
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
    {
      icon: 'language' as const,
      title: t('home.quickTranslate'),
      desc: t('home.quickTranslateDesc'),
      prompt: t('home.quickTranslatePrompt'),
    },
    {
      icon: 'receipt' as const,
      title: t('home.quickCostCheck'),
      desc: t('home.quickCostCheckDesc'),
      prompt: t('home.quickCostCheckPrompt'),
    },
    {
      icon: 'history' as const,
      title: t('home.quickRecentTasks'),
      desc: t('home.quickRecentTasksDesc'),
      prompt: t('home.quickRecentTasksPrompt'),
    },
  ]

  return (
    <div className="h-full overflow-y-auto">
      <div className="min-h-full flex flex-col items-center justify-center text-center px-6 py-6">
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
          className="px-3 py-1.5 rounded-full text-[11px] text-[var(--text-secondary)] modern-card border-[var(--border)] hover:text-[var(--accent)] hover:border-[var(--accent)]/40 transition-colors"
        >
          {t('home.example1')}
        </button>
        <button
          onClick={() => onQuick(t('home.example2'))}
          className="px-3 py-1.5 rounded-full text-[11px] text-[var(--text-secondary)] modern-card border-[var(--border)] hover:text-[var(--accent)] hover:border-[var(--accent)]/40 transition-colors"
        >
          {t('home.example2')}
        </button>
      </div>
      </div>
    </div>
  )
})

