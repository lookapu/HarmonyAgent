import { useEffect, useMemo, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { convertFileSrc } from '@tauri-apps/api/core'
import { save } from '@tauri-apps/plugin-dialog'
import { useTranslation } from 'react-i18next'
import { readProjectFile, saveProjectFile, deleteProjectFile } from '../api/project'
import type { FileTreeNode } from '../api/project'
import Icon from '../icons/Icon'
import Markdown from './Markdown'

const IMAGE_EXTS = ['png', 'jpg', 'jpeg', 'gif', 'webp', 'svg', 'bmp', 'ico', 'tif', 'tiff', 'psd', 'avif']
const AUDIO_EXTS = ['mp3', 'wav', 'ogg', 'm4a', 'flac', 'aac']
const VIDEO_EXTS = ['mp4', 'webm', 'mov', 'mkv', 'avi', 'm4v']
/** office/pdf 文档：由后端解析为纯文本后渲染 */
const DOC_EXTS = ['docx', 'pptx', 'xlsx', 'pdf']
const TEXT_EXTS = [
  'md', 'markdown', 'txt', 'text', 'xml', 'json', 'json5', 'jsonc', 'log',
  'java', 'kt', 'kts', 'ts', 'tsx', 'js', 'jsx', 'mjs', 'cjs', 'mts', 'cts',
  'css', 'scss', 'less', 'html', 'htm', 'rs', 'toml', 'yml', 'yaml',
  'sql', 'sh', 'bash', 'bat', 'cmd', 'ps1', 'py', 'go', 'c', 'cpp',
  'cc', 'h', 'hpp', 'properties', 'gradle', 'conf', 'cfg', 'ini',
  'csv', 'tsv', 'env', 'gitignore', 'editorconfig', 'ets', 'arkts',
  's', 'asm', 'vue', 'svelte', 'dart', 'swift', 'm', 'mm', 'rb', 'php',
  // 无扩展名/点文件与更多语言：Dockerfile、.env.local、Cargo.lock、.npmrc 等
  'dockerfile', 'dockerignore', 'npmrc', 'prettierrc', 'eslintrc', 'babelrc', 'yarnrc', 'lock',
  'cs', 'scala', 'groovy', 'lua', 'zig', 'proto', 'graphql', 'ejs', 'hbs', 'plist', 'tf', 'tfvars',
  'ex', 'exs', 'erl', 'clj', 'cljs', 'fs', 'fsx', 'r', 'jl', 'nix',
]

/** 扩展名 → highlight.js 语言名（未注册的语言由 Markdown 组件兜底为纯文本） */
const HL_LANG: Record<string, string> = {
  arkts: 'ts', ets: 'ts', mts: 'ts', cts: 'ts', tsx: 'ts', jsx: 'js', mjs: 'js', cjs: 'js',
  vue: 'html', svelte: 'html', json5: 'json', jsonc: 'json', mdx: 'markdown', lock: 'toml',
  plist: 'xml', tf: 'ini', tfvars: 'ini', kts: 'kotlin', gradle: 'groovy',
  yml: 'yaml', yaml: 'yaml', sh: 'shell', bash: 'shell', ps1: 'powershell', bat: 'bash', cmd: 'bash',
  log: 'plaintext', txt: 'plaintext', text: 'plaintext', h: 'c', hpp: 'cpp', cc: 'cpp',
}

interface Props {
  node: FileTreeNode
  projectId: string
  projectPath: string
  /** 会话工作目录（worktree 模式为 worktree 路径，本地模式为 undefined → 后端回退主仓库） */
  root?: string
  onClose: () => void
  onReference: (path: string) => void
  /** 引用某段选区到对话（起止行 + 文本片段） */
  onReferenceSelection?: (payload: { path: string; startLine: number; endLine: number; snippet: string }) => void
  /** 打开后定位并高亮的行号（1-based，来自代码块行号跳转） */
  focusLine?: number
  /** 文件删除成功后刷新文件树 */
  onRefresh?: () => void
}

/** 拼接平台格式的绝对路径（供 convertFileSrc 使用） */
function absolutePath(projectPath: string, rel: string) {
  const sep = navigator.platform?.toLowerCase().includes('win') ? '\\' : '/'
  return `${projectPath.replace(/[\\/]+$/, '')}${sep}${rel.replace(/\//g, sep)}`
}

function formatSize(bytes?: number) {
  if (bytes == null) return ''
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
  return `${(bytes / 1024 / 1024).toFixed(2)} MB`
}

/**
 * 文件预览弹窗：图片/音频/视频直接播放，文本与代码读取后渲染
 * （md 走 Markdown，其余走代码高亮），头部提供复制路径与发送到对话。
 */
export default function FilePreviewDialog({ node, projectId, projectPath, root, onClose, onReference, onReferenceSelection, focusLine, onRefresh }: Props) {
  const { t } = useTranslation()
  const ext = useMemo(() => {
    // 多段/点文件处理：.env.local → env；.npmrc → npmrc；Dockerfile → dockerfile；Cargo.lock → lock
    const s = node.name.split('.')
    if (s.length === 1) return s[0].toLowerCase()
    if (s[0] === '' && s.length > 2) return s[1].toLowerCase()
    return s[s.length - 1].toLowerCase()
  }, [node.name])
  const kind = useMemo(() => {
    if (IMAGE_EXTS.includes(ext)) return 'image'
    if (AUDIO_EXTS.includes(ext)) return 'audio'
    if (VIDEO_EXTS.includes(ext)) return 'video'
    if (DOC_EXTS.includes(ext)) return 'document'
    if (TEXT_EXTS.includes(ext)) return 'text'
    return 'unknown'
  }, [ext])
  /** 文本类（含文档解析文本）：统一走 readProjectFile → 代码块渲染，支持行号/选区 */
  const isTextLike = useMemo(() => kind === 'text' || kind === 'document', [kind])

  const [text, setText] = useState<string | null>(null)
  const [loading, setLoading] = useState(isTextLike)
  const [error, setError] = useState<string | null>(null)
  /** 内容因过大被保头保尾截断（含截断前总字符数） */
  const [truncated, setTruncated] = useState(false)
  const [totalChars, setTotalChars] = useState<number | null>(null)
  const [copied, setCopied] = useState(false)
  const contentRef = useRef<HTMLDivElement>(null)
  // 行选区：anchor 为起始行（普通点击点），focus 为结束行；两者构成连续区间高亮
  const [selAnchor, setSelAnchor] = useState<number | null>(null)
  const [selFocus, setSelFocus] = useState<number | null>(null)

  // focusLine（来自代码块跳转）作为初始选区（单行）
  useEffect(() => {
    if (focusLine && focusLine > 0) {
      setSelAnchor(focusLine)
      setSelFocus(focusLine)
    }
  }, [focusLine])

  /** 行号点击：普通点击重置为单行；Shift 点击扩展选区到该行 */
  const handleLineClick = (line: number, e: React.MouseEvent) => {
    if (e.shiftKey && selAnchor != null) {
      setSelFocus(line)
    } else {
      setSelAnchor(line)
      setSelFocus(line)
    }
  }

  /** 当前选区文本片段（从已加载 text 中按行截取，保留行号对齐） */
  const selectedSnippet = useMemo(() => {
    if (!text || selAnchor == null || selFocus == null) return null
    const lines = text.split('\n')
    const start = Math.max(1, Math.min(selAnchor, selFocus))
    const end = Math.min(lines.length, Math.max(selAnchor, selFocus))
    const snippet = lines.slice(start - 1, end).join('\n')
    return { start, end, snippet }
  }, [text, selAnchor, selFocus])

  /** 复制选区文本到剪贴板 */
  const copySelection = async () => {
    if (!selectedSnippet) return
    try {
      await navigator.clipboard.writeText(selectedSnippet.snippet)
      setSelCopied(true)
      setTimeout(() => setSelCopied(false), 1500)
    } catch {
      // 静默失败
    }
  }

  /** 把选区作为代码片段引用到对话输入框 */
  const referenceSelection = () => {
    if (!selectedSnippet || !onReferenceSelection) return
    onReferenceSelection({
      path: node.path,
      startLine: selectedSnippet.start,
      endLine: selectedSnippet.end,
      snippet: selectedSnippet.snippet,
    })
  }

  const [selCopied, setSelCopied] = useState(false)

  // 选区变化时：滚动到选区起始行并闪烁；focusLine 初次跳转也由此覆盖
  useEffect(() => {
    if (selAnchor == null || !isTextLike || loading || error || !text) return
    const t = setTimeout(() => {
      const container = contentRef.current
      if (!container) return
      const el = container.querySelector(`[data-line="${selAnchor}"]`) as HTMLElement | null
      if (el) {
        el.scrollIntoView({ block: 'center', behavior: 'smooth' })
        el.classList.add('code-line-flash')
        setTimeout(() => el.classList.remove('code-line-flash'), 2000)
      }
    }, 80)
    return () => clearTimeout(t)
  }, [selAnchor, isTextLike, loading, error, text])

  useEffect(() => {
    if (!isTextLike) return
    let cancelled = false
    setLoading(true)
    setError(null)
    readProjectFile(projectId, node.path, root)
      .then((res) => {
        if (!cancelled) {
          setText(res.content)
          setTruncated(res.truncated)
          setTotalChars(res.totalChars ?? null)
        }
      })
      .catch((e) => {
        if (!cancelled) setError(String(e).replace(/^Error:\s*/, ''))
      })
      .finally(() => {
        if (!cancelled) setLoading(false)
      })
    return () => {
      cancelled = true
    }
  }, [node.path, projectId, root, isTextLike])

  // Esc 关闭
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose()
    }
    window.addEventListener('keydown', handler)
    return () => window.removeEventListener('keydown', handler)
  }, [onClose])

  const copyPath = async () => {
    try {
      await navigator.clipboard.writeText(node.path)
      setCopied(true)
      setTimeout(() => setCopied(false), 1500)
    } catch {
      // 静默失败
    }
  }

  const [downloading, setDownloading] = useState(false)
  const [confirmDelete, setConfirmDelete] = useState(false)
  const [deleting, setDeleting] = useState(false)
  /** 下载/删除操作结果反馈（成功或失败消息，短暂显示） */
  const [feedback, setFeedback] = useState<string | null>(null)

  /** 下载：原生保存对话框选位置 → 后端复制原文件（WebView2 对 Blob URL 下载支持不稳，不走 a.download） */
  const handleDownload = async () => {
    if (downloading) return
    setDownloading(true)
    try {
      const dest = await save({ title: t('home.previewSaveAs'), defaultPath: node.name })
      if (dest) {
        const bytes = await saveProjectFile(projectId, node.path, dest, root)
        setFeedback(`${t('home.previewSaveDone')}（${formatSize(bytes)}）`)
        setTimeout(() => setFeedback(null), 3000)
      }
    } catch (e) {
      setFeedback(String(e).replace(/^Error:\s*/, ''))
      setTimeout(() => setFeedback(null), 4000)
    } finally {
      setDownloading(false)
    }
  }

  /** 删除：首次点击进入确认态（3 秒未确认自动复原），确认后移入系统回收站 */
  const handleDelete = async () => {
    if (deleting) return
    if (!confirmDelete) {
      setConfirmDelete(true)
      setTimeout(() => setConfirmDelete(false), 3000)
      return
    }
    setDeleting(true)
    try {
      await deleteProjectFile(projectId, node.path, root)
      onRefresh?.()
      onClose()
    } catch (e) {
      setFeedback(String(e).replace(/^Error:\s*/, ''))
      setConfirmDelete(false)
      setDeleting(false)
      setTimeout(() => setFeedback(null), 4000)
    }
  }

  const mediaSrc = kind === 'image' || kind === 'audio' || kind === 'video' ? convertFileSrc(absolutePath(projectPath, node.path)) : ''

  return createPortal(
    <div
      className="fixed inset-0 z-[70] flex items-center justify-center bg-black/50 backdrop-blur-[2px] p-6"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) onClose()
      }}
    >
      <div className="w-[880px] max-w-[95vw] h-[82vh] max-h-[82vh] rounded-2xl glass-card flex flex-col overflow-hidden animate-modal-in">
        {/* 头部 */}
        <div className="h-12 shrink-0 px-4 flex items-center gap-3 border-b border-[var(--border)]">
          <div className="w-7 h-7 rounded-lg bg-[var(--accent-soft)] flex items-center justify-center shrink-0">
            <Icon name="file" size={13} />
          </div>
          <div className="min-w-0 flex-1">
            <div className="text-[13px] font-medium truncate leading-tight">{node.name}</div>
            <div className="text-[10px] text-[var(--text-muted)] truncate font-mono">{node.path}</div>
          </div>
          {node.size != null && (
            <span className="shrink-0 text-[10px] text-[var(--text-muted)] tabular-nums bg-[var(--bg-card)] rounded-md px-2 py-1">
              {formatSize(node.size)}
            </span>
          )}
          <button
            onClick={copyPath}
            className="shrink-0 h-7 px-2.5 rounded-lg text-[11px] text-[var(--text-secondary)] hover:text-[var(--accent)] hover:bg-[var(--bg-hover)] transition-colors flex items-center gap-1"
            title={t('home.copyPath')}
          >
            {copied ? '✓' : <Icon name="copy" size={12} />}
            {copied ? t('home.copied') : t('home.copyPath')}
          </button>
          <button
            onClick={() => onReference(node.path)}
            className="shrink-0 h-7 px-2.5 rounded-lg text-[11px] text-[var(--text-secondary)] hover:text-[var(--accent)] hover:bg-[var(--bg-hover)] transition-colors flex items-center gap-1"
            title={t('home.sendToChat')}
          >
            <Icon name="plus" size={12} />
            {t('home.sendToChat')}
          </button>
          <button
            onClick={handleDownload}
            disabled={downloading}
            className="shrink-0 h-7 px-2.5 rounded-lg text-[11px] text-[var(--text-secondary)] hover:text-[var(--accent)] hover:bg-[var(--bg-hover)] transition-colors flex items-center gap-1 disabled:opacity-40"
            title={t('home.previewDownload')}
          >
            <Icon name="download" size={12} />
            {downloading ? '…' : t('home.previewDownload')}
          </button>
          <button
            onClick={handleDelete}
            disabled={deleting}
            className={`shrink-0 h-7 px-2.5 rounded-lg text-[11px] transition-colors flex items-center gap-1 disabled:opacity-40 ${confirmDelete ? 'text-white bg-[var(--danger)] hover:opacity-90' : 'text-[var(--text-secondary)] hover:text-[var(--danger)] hover:bg-[var(--danger)]/10'}`}
            title={t('home.previewDelete')}
          >
            <Icon name="delete" size={12} />
            {deleting ? '…' : confirmDelete ? t('home.previewDeleteConfirm') : t('home.previewDelete')}
          </button>
          <button
            onClick={onClose}
            className="shrink-0 w-7 h-7 rounded-lg flex items-center justify-center text-[var(--text-muted)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] transition-colors"
            title={t('common.close')}
          >
            <Icon name="close" size={14} />
          </button>
        </div>

        {/* 下载/删除操作反馈（成功/失败短暂提示） */}
        {feedback && (
          <div className="shrink-0 px-4 py-1.5 text-[11px] text-[var(--text-secondary)] bg-[var(--bg-window)] border-b border-[var(--border)] break-all">
            {feedback}
          </div>
        )}

        {/* 内容区 */}
        <div ref={contentRef} className="relative flex-1 min-h-0 overflow-auto bg-[var(--bg-primary)]">
          {/* 选区浮动操作栏：选中行后显示复制选区 / 引用到对话 */}
          {isTextLike && selectedSnippet && (
            <div className="sticky top-2 z-10 mx-auto w-fit flex items-center gap-1 rounded-lg border border-[var(--border)] bg-[var(--bg-elevated)] shadow-lg px-1.5 py-1">
              <span className="px-2 text-[11px] text-[var(--text-muted)] tabular-nums">
                {selectedSnippet.start === selectedSnippet.end
                  ? `第 ${selectedSnippet.start} 行`
                  : `第 ${selectedSnippet.start}–${selectedSnippet.end} 行 · ${selectedSnippet.end - selectedSnippet.start + 1} 行`}
              </span>
              <button
                onClick={copySelection}
                className="h-6 px-2 rounded-md text-[11px] text-[var(--text-secondary)] hover:text-[var(--accent)] hover:bg-[var(--bg-hover)] transition-colors flex items-center gap-1"
              >
                {selCopied ? <Icon name="check" size={12} /> : <Icon name="copy" size={12} />}
                {selCopied ? t('home.copied') : t('home.copySelection')}
              </button>
              {onReferenceSelection && (
                <button
                  onClick={referenceSelection}
                  className="h-6 px-2 rounded-md text-[11px] text-white bg-[var(--accent)] hover:opacity-90 transition-opacity flex items-center gap-1"
                >
                  <Icon name="plus" size={12} white />
                  {t('home.referenceSelection')}
                </button>
              )}
            </div>
          )}
          {kind === 'image' && (
            <div className="h-full flex items-center justify-center p-4">
              <img src={mediaSrc} alt={node.name} className="max-w-full max-h-full object-contain rounded-lg" />
            </div>
          )}
          {kind === 'audio' && (
            <div className="h-full flex flex-col items-center justify-center gap-4 p-6">
              <div className="w-16 h-16 rounded-2xl bg-[var(--accent-soft)] flex items-center justify-center">
                <Icon name="headphones" size={26} />
              </div>
              <audio src={mediaSrc} controls autoPlay className="w-full max-w-md" />
            </div>
          )}
          {kind === 'video' && (
            <div className="h-full flex items-center justify-center p-4">
              <video src={mediaSrc} controls autoPlay className="max-w-full max-h-full rounded-lg" />
            </div>
          )}
          {isTextLike && (
            <div className="p-4">
              {kind === 'document' && !loading && !error && (
                <div className="mb-3 rounded-lg border border-[var(--accent)]/20 bg-[var(--accent-soft)] px-3 py-2 text-[11px] text-[var(--text-secondary)]">
                  {truncated
                    ? t('home.previewTruncated', { count: totalChars ?? 0 })
                    : t('home.documentTextNote')}
                </div>
              )}
              {kind !== 'document' && truncated && !loading && !error && (
                <div className="mb-3 rounded-lg border border-[var(--warning)]/30 bg-[var(--warning)]/10 px-3 py-2 text-[11px] text-[var(--warning)]">
                  {t('home.previewTruncated', { count: totalChars ?? 0 })}
                </div>
              )}
              {loading ? (
                <div className="flex flex-col items-center justify-center gap-3 py-16 text-center">
                  <span className="w-6 h-6 rounded-full border-2 border-[var(--accent)] border-t-transparent animate-spin" />
                  <p className="text-[11px] text-[var(--text-muted)]">{t('common.loading')}</p>
                </div>
              ) : error ? (
                <div className="rounded-xl border border-[var(--danger)]/30 bg-[var(--danger)]/8 px-4 py-6 text-center">
                  <p className="text-[12px] text-[var(--danger)] break-all">{error}</p>
                </div>
              ) : ext === 'md' || ext === 'markdown' ? (
                <Markdown
                  focusLine={focusLine}
                  selectedLines={selAnchor != null && selFocus != null
                    ? [Math.min(selAnchor, selFocus), Math.max(selAnchor, selFocus)]
                    : undefined}
                  onLineClick={handleLineClick}
                >
                  {text ?? ''}
                </Markdown>
              ) : (
                <Markdown
                  focusLine={focusLine}
                  selectedLines={selAnchor != null && selFocus != null
                    ? [Math.min(selAnchor, selFocus), Math.max(selAnchor, selFocus)]
                    : undefined}
                  onLineClick={handleLineClick}
                >{`\`\`\`${kind === 'document' ? 'plaintext' : HL_LANG[ext] ?? ext}\n${text ?? ''}\n\`\`\``}</Markdown>
              )}
            </div>
          )}
          {kind === 'unknown' && (
            <div className="h-full flex flex-col items-center justify-center gap-3 p-6 text-center">
              <Icon name="file" size={32} className="opacity-30" />
              <p className="text-[12px] text-[var(--text-muted)]">{t('home.previewUnsupported')}</p>
              <p className="text-[10px] text-[var(--text-muted)] font-mono">.{ext}</p>
            </div>
          )}
        </div>
      </div>
    </div>,
    document.body,
  )
}
