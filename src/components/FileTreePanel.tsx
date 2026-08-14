import { useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import type { FileTreeNode } from '../api/project'
import { symbolCounts } from '../api/symbols'
import Icon from '../icons/Icon'
import FilePreviewDialog from './FilePreviewDialog'

interface Props {
  tree: FileTreeNode | null
  building: boolean
  projectId: string
  projectPath: string
  /** 懒加载缓存：目录相对路径 -> 该层子项列表 */
  dirCache: Record<string, FileTreeNode[]>
  /** 读取单层目录（带缓存），返回该层子项 */
  onLoadDir: (path: string) => Promise<FileTreeNode[]>
  onRefresh: () => void
  onReference: (path: string) => void
  /** 把文件中的某段选区引用到对话（path + 起止行 + 文本片段） */
  onReferenceSelection?: (payload: { path: string; startLine: number; endLine: number; snippet: string }) => void
}

/** 右侧面板：项目文件树（懒加载：先读根目录，展开时逐级按需请求，无数量上限不截断） */
export default function FileTreePanel({
  tree,
  building,
  projectId,
  projectPath,
  dirCache,
  onLoadDir,
  onRefresh,
  onReference,
  onReferenceSelection,
}: Props) {
  const { t } = useTranslation()
  const [expanded, setExpanded] = useState<Set<string>>(new Set(['']))
  const [preview, setPreview] = useState<FileTreeNode | null>(null)
  // 预览定位行号（来自代码块行号跳转的 deveco:open-file 事件）
  const [previewLine, setPreviewLine] = useState<number | undefined>(undefined)
  const [menu, setMenu] = useState<{ x: number; y: number; node: FileTreeNode } | null>(null)
  const [copied, setCopied] = useState(false)
  const [loadingAll, setLoadingAll] = useState(false)
  const menuRef = useRef<HTMLDivElement>(null)
  // 文件级符号数量（徽标展示，后台异步拉取，失败静默）
  const [symCounts, setSymCounts] = useState<Record<string, number>>({})

  useEffect(() => {
    let cancelled = false
    symbolCounts(projectId)
      .then((list) => {
        if (cancelled) return
        const map: Record<string, number> = {}
        for (const c of list) map[c.file] = c.count
        setSymCounts(map)
      })
      .catch(() => {})
    return () => {
      cancelled = true
    }
  }, [projectId])

  // 切换项目时重置展开/菜单状态
  useEffect(() => {
    setExpanded(new Set(['']))
    setMenu(null)
  }, [projectId])

  /**
   * 监听全局"打开文件"事件（代码块文件路径/行号点击触发）：
   * 在已加载的目录缓存中查找节点，找到则打开预览并定位到行；未加载的目录逐级懒加载。
   */
  useEffect(() => {
    const handler = (e: Event) => {
      const detail = (e as CustomEvent<{ path: string; line?: number }>).detail
      if (!detail?.path) return
      const target = detail.path.replace(/^\/+/, '').replace(/\\/g, '/')
      setPreviewLine(detail.line)
      // 先在缓存中查找
      let found: FileTreeNode | null = null
      for (const items of Object.values(dirCache)) {
        const hit = items.find((it) => it.type === 'file' && it.path.replace(/\\/g, '/') === target)
        if (hit) {
          found = hit
          break
        }
      }
      if (found) {
        setPreview(found)
        return
      }
      // 未缓存：逐级展开并加载目标路径所在目录
      const parts = target.split('/')
      const fileName = parts.pop() ?? ''
      const dirPath = parts.join('/')
      ;(async () => {
        // 确保父级目录都展开
        let acc = ''
        for (const seg of parts) {
          acc = acc ? `${acc}/${seg}` : seg
          setExpanded((prev) => {
            if (prev.has(acc)) return prev
            const next = new Set(prev)
            next.add(acc)
            return next
          })
        }
        try {
          const items = await onLoadDir(dirPath)
          const hit = items.find((it) => it.type === 'file' && it.name === fileName)
          if (hit) setPreview(hit)
        } catch {
          // 目录加载失败时静默
        }
      })()
    }
    window.addEventListener('deveco:open-file', handler as EventListener)
    return () => window.removeEventListener('deveco:open-file', handler as EventListener)
  }, [dirCache, onLoadDir])

  const toggle = (path: string) => {
    setExpanded((prev) => {
      const next = new Set(prev)
      if (next.has(path)) next.delete(path)
      else next.add(path)
      return next
    })
  }

  // 右键菜单外部点击/失焦关闭
  useEffect(() => {
    const close = () => setMenu(null)
    window.addEventListener('mousedown', close)
    window.addEventListener('blur', close)
    return () => {
      window.removeEventListener('mousedown', close)
      window.removeEventListener('blur', close)
    }
  }, [])

  // 全部展开：逐级递归加载目录（上限 300 个防爆），加载完成后全展开
  const expandAll = async () => {
    if (!tree || loadingAll) return
    setLoadingAll(true)
    try {
      const dirs: string[] = ['']
      const all: string[] = []
      while (dirs.length > 0 && all.length < 300) {
        const p = dirs.shift()!
        all.push(p)
        const children = await onLoadDir(p)
        children.filter((c) => c.type === 'dir').forEach((c) => dirs.push(c.path))
      }
      setExpanded(new Set(all))
    } finally {
      setLoadingAll(false)
    }
  }
  const collapseAll = () => setExpanded(new Set([]))

  const openContextMenu = (e: React.MouseEvent, node: FileTreeNode) => {
    e.preventDefault()
    e.stopPropagation()
    setCopied(false)
    setMenu({ x: e.clientX, y: e.clientY, node })
  }

  const copyPath = async () => {
    if (!menu) return
    const sep = navigator.platform?.toLowerCase().includes('win') ? '\\' : '/'
    const full = `${projectPath.replace(/[\\/]+$/, '')}${sep}${menu.node.path.replace(/\//g, sep)}`
    try {
      await navigator.clipboard.writeText(full)
      setCopied(true)
      setTimeout(() => setMenu(null), 800)
    } catch {
      setMenu(null)
    }
  }

  const sendToChat = () => {
    if (!menu) return
    onReference(menu.node.path)
    setMenu(null)
  }

  if (building) {
    return (
      <div className="flex flex-col items-center justify-center gap-3 py-14 text-center">
        <span className="w-6 h-6 rounded-full border-2 border-[var(--accent)] border-t-transparent animate-spin" />
        <p className="text-[11px] text-[var(--text-muted)]">{t('home.indexBuilding')}</p>
      </div>
    )
  }

  if (!tree) {
    return (
      <div className="flex flex-col items-center justify-center gap-3 py-14 text-center">
        <Icon name="folder" size={22} className="opacity-40" />
        <p className="text-[11px] text-[var(--text-muted)]">{t('home.indexEmpty')}</p>
        <button
          onClick={onRefresh}
          className="h-7 px-3 rounded-lg bg-[var(--accent-soft)] text-[var(--accent)] text-[11px] font-medium hover:bg-[var(--accent)] hover:text-white transition-colors"
        >
          {t('home.rebuildIndex')}
        </button>
      </div>
    )
  }

  return (
    <div className="p-2 pb-4">
      {/* 头部：项目名 + 展开/收起 + 刷新 */}
      <div className="flex items-center justify-between px-2 pb-2">
        <span className="text-[11px] font-medium text-[var(--text-secondary)] truncate">{tree.name}</span>
        <div className="flex items-center gap-0.5 shrink-0">
          <button
            onClick={expanded.size > 1 ? collapseAll : expandAll}
            title={expanded.size > 1 ? t('home.collapseAll') : t('home.expandAll')}
            className="p-1 rounded-md text-[var(--text-muted)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] transition-colors"
          >
            {loadingAll ? (
              <span className="block w-3 h-3 rounded-full border border-[var(--accent)] border-t-transparent animate-spin" />
            ) : (
              <Icon name={expanded.size > 1 ? 'chevron-left' : 'chevron-right'} size={13} className="rotate-90" />
            )}
          </button>
          <button
            onClick={onRefresh}
            title={t('home.rebuildIndex')}
            className="p-1 rounded-md text-[var(--text-muted)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] transition-colors"
          >
            <Icon name="refresh" size={13} />
          </button>
        </div>
      </div>
      <div className="space-y-px">
        {tree.children?.map((node) => (
          <TreeNodeItem
            key={node.path}
            node={node}
            depth={0}
            expanded={expanded}
            dirCache={dirCache}
            onLoadDir={onLoadDir}
            onToggle={toggle}
            onReference={onReference}
            onPreview={(n) => setPreview(n)}
            onContextMenu={openContextMenu}
            counts={symCounts}
          />
        ))}
      </div>

      {/* 右键菜单 */}
      {menu && (
        <div
          ref={menuRef}
          className="fixed z-[80] w-44 rounded-xl border border-[var(--border)] bg-[var(--bg-card)] shadow-2xl shadow-black/40 py-1 animate-modal-in"
          style={{
            left: Math.min(menu.x, window.innerWidth - 190),
            top: Math.min(menu.y, window.innerHeight - 140),
          }}
          onMouseDown={(e) => e.stopPropagation()}
        >
          {menu.node.type === 'file' && (
            <button
              onClick={() => {
                setPreview(menu.node)
                setMenu(null)
              }}
              className="w-full flex items-center gap-2.5 px-3 py-2 text-[12px] text-[var(--text-secondary)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] transition-colors"
            >
              <Icon name="devices" size={13} className="opacity-60" />
              {t('home.preview')}
            </button>
          )}
          <button
            onClick={copyPath}
            className="w-full flex items-center gap-2.5 px-3 py-2 text-[12px] text-[var(--text-secondary)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] transition-colors"
          >
            <Icon name="copy" size={13} className="opacity-60" />
            {copied ? t('home.copied') : t('home.copyPath')}
          </button>
          <button
            onClick={sendToChat}
            className="w-full flex items-center gap-2.5 px-3 py-2 text-[12px] text-[var(--text-secondary)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-hover)] transition-colors"
          >
            <Icon name="plus" size={13} className="opacity-60" />
            {t('home.sendToChat')}
          </button>
        </div>
      )}

      {/* 文件预览弹窗 */}
      {preview && (
        <FilePreviewDialog
          node={preview}
          projectId={projectId}
          projectPath={projectPath}
          onClose={() => {
            setPreview(null)
            setPreviewLine(undefined)
          }}
          onReference={onReference}
          onReferenceSelection={onReferenceSelection}
          focusLine={previewLine}
        />
      )}
    </div>
  )
}

/* ============ 递归节点（懒加载） ============ */
function TreeNodeItem({
  node,
  depth,
  expanded,
  dirCache,
  onLoadDir,
  onToggle,
  onReference,
  onPreview,
  onContextMenu,
  counts,
}: {
  node: FileTreeNode
  depth: number
  expanded: Set<string>
  dirCache: Record<string, FileTreeNode[]>
  onLoadDir: (path: string) => Promise<FileTreeNode[]>
  onToggle: (path: string) => void
  onReference: (path: string) => void
  onPreview: (node: FileTreeNode) => void
  onContextMenu: (e: React.MouseEvent, node: FileTreeNode) => void
  /** 文件级符号数量（相对路径 -> 数量），用于文件行徽标 */
  counts: Record<string, number>
}) {
  const { t } = useTranslation()
  const isDir = node.type === 'dir'
  const isOpen = expanded.has(node.path)
  const cached = dirCache[node.path]
  const [loading, setLoading] = useState(false)
  const [failed, setFailed] = useState(false)

  // 首次展开时按需读取该层目录（已缓存/加载中则跳过）
  const ensureLoaded = async () => {
    if (cached || loading) return
    setLoading(true)
    setFailed(false)
    try {
      await onLoadDir(node.path)
      setLoading(false)
    } catch {
      setFailed(true)
      setLoading(false)
    }
  }

  const handleClick = () => {
    if (isDir) {
      onToggle(node.path)
      ensureLoaded()
    } else {
      onPreview(node)
    }
  }

  return (
    <div>
      <div
        className={`group flex items-center gap-1 rounded-md py-[3px] pr-1 text-[12px] cursor-pointer select-none transition-colors ${
          isDir ? '' : 'hover:bg-[var(--bg-hover)]'
        } ${depth === 0 ? 'font-medium text-[var(--text-primary)]' : 'text-[var(--text-secondary)]'}`}
        style={{ paddingLeft: depth * 14 + 8 }}
        onClick={handleClick}
        onContextMenu={(e) => onContextMenu(e, node)}
        title={isDir ? undefined : t('home.previewFileTip')}
      >
        {isDir ? (
          <>
            <Icon
              name="chevron-right"
              size={11}
              className={`shrink-0 transition-transform duration-150 ${isOpen ? 'rotate-90' : ''} opacity-60`}
            />
            <Icon name="folder" size={13} className="shrink-0 opacity-70" />
          </>
        ) : (
          <>
            <span className="w-[11px] shrink-0" />
            <Icon name="file" size={13} className="shrink-0 opacity-60" />
          </>
        )}
        <span className="flex-1 truncate" title={node.path}>
          {node.name}
        </span>
        {!isDir && (counts[node.path] ?? 0) > 0 && (
          <span
            className="text-[9px] text-[var(--text-muted)] shrink-0 tabular-nums opacity-70 group-hover:opacity-100"
            title={`${counts[node.path]} ${t('home.symbols')}`}
          >
            {counts[node.path]}
          </span>
        )}
        {!isDir && (
          <button
            onClick={(e) => {
              e.stopPropagation()
              onReference(node.path)
            }}
            title={t('home.sendToChat')}
            className="opacity-0 group-hover:opacity-100 p-0.5 rounded text-[var(--text-muted)] hover:text-[var(--accent)] hover:bg-[var(--accent-soft)] transition-all shrink-0"
          >
            <Icon name="plus" size={12} />
          </button>
        )}
      </div>
      {isDir && isOpen && (
        <div className="space-y-px">
          {loading && !cached ? (
            <div className="flex items-center gap-1.5 py-1 pl-6 text-[11px] text-[var(--text-muted)]">
              <span className="w-3 h-3 rounded-full border border-[var(--accent)] border-t-transparent animate-spin" />
              {t('home.loadingDir')}
            </div>
          ) : failed ? (
            <button
              onClick={ensureLoaded}
              className="flex items-center gap-1.5 py-1 pl-6 text-[11px] text-[var(--text-muted)] hover:text-[var(--accent)] transition-colors"
            >
              <Icon name="refresh" size={11} />
              {t('home.loadRetry')}
            </button>
          ) : cached ? (
            cached.length === 0 ? (
              <div className="py-1 pl-6 text-[11px] text-[var(--text-muted)] italic">{t('home.emptyDir')}</div>
            ) : (
              cached.map((child) => (
                <TreeNodeItem
                  key={child.path}
                  node={child}
                  depth={depth + 1}
                  expanded={expanded}
                  dirCache={dirCache}
                  onLoadDir={onLoadDir}
                  onToggle={onToggle}
                  onReference={onReference}
                  onPreview={onPreview}
                  onContextMenu={onContextMenu}
                  counts={counts}
                />
              ))
            )
          ) : null}
        </div>
      )}
    </div>
  )
}
