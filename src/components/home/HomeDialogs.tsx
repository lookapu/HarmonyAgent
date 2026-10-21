/**
 * 批量发送 / 导入会话 / 审计日志三个模态
 *
 * 从 Home.tsx 搬出（原 L8700-8950、L9141-9279）。**纯代码移动，逻辑未改。**
 * 审计对话框原本被夹在会话笔记栏之后，搬出时归拢到本文件。
 */

import { useEffect, useMemo, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useAuditStore, type AuditCategory } from '../../stores/auditStore'
import { useEscapeKey } from '../../hooks/useEscapeKey'
import Icon from '../../icons/Icon'
import { Button } from '../ui/Button'
import { IconButton } from '../ui/IconButton'

/* ============ 批量任务浮层：每行一条入队（agentOwned=false 让任务结束后自动续跑） ============
 * - 多行文本 → 逐行入队 → Agent 串行处理
 * - 入参 initial：当前输入框草稿（默认填入，用户可继续编辑）
 * - 入参 onSubmit：用户点"入队"时触发，把行数组交给外层；返回成功的条数与失败行
 * - 有失败行时留在弹窗里（只保留失败行），用户修改后可重试；成功行已入队不会重复
 * - Esc 关闭、Cmd/Ctrl+Enter 直接入队
 */
export type BatchSendResult = { ok: number; failed: string[] }

export function BatchSendDialog({ initial, onClose, onSubmit }: {
  initial: string
  onClose: () => void
  onSubmit: (lines: string[]) => BatchSendResult | Promise<BatchSendResult>
}) {
  const { t } = useTranslation()
  const [text, setText] = useState(initial)
  // 拆行：去空白、去空行；保留用户原顺序
  const lines = useMemo(
    () => text.split('\n').map((s) => s.trim()).filter(Boolean),
    [text]
  )
  const submittingRef = useRef(false)

  // 提交：避免重复触发（点按钮 / 按快捷键同时按）
  const submit = async () => {
    if (submittingRef.current) return
    if (lines.length === 0) return
    submittingRef.current = true
    try {
      const result = await onSubmit(lines)
      if (result.failed.length > 0) setText(result.failed.join('\n'))
    } finally {
      submittingRef.current = false
    }
  }

  // 快捷键：Ctrl+Enter 提交（Esc 走全局栈，见下）。
  // 用 ref 指向最新 submit：只在挂载时注册一次监听；若依赖 lines.length，
  // 同行数内的编辑不会重建监听，快捷键会提交上一版文本。
  const submitRef = useRef(submit)
  submitRef.current = submit
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) {
        e.preventDefault()
        void submitRef.current()
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [])
  useEscapeKey(onClose)

  return (
    <div className="cmdk-backdrop" onClick={onClose}>
      <div
        className="w-[600px] max-w-[92vw] glass-card p-4 animate-modal-in"
        onClick={(e) => e.stopPropagation()}
      >
        {/* 标题栏 */}
        <div className="flex items-center justify-between mb-2">
          <div className="flex items-center gap-2">
            <div className="w-6 h-6 rounded-md bg-[var(--accent)]/15 text-[var(--accent)] flex items-center justify-center">
              <Icon name="package" size={13} />
            </div>
            <h2 className="text-[length:var(--app-text-base)] font-semibold">{t('home.batch')}</h2>
          </div>
          {/* h-6 w-6 锁死 24px 盒：裸 IconButton 只有 13px 图标 + 8px padding = 21px */}
          <IconButton icon="close" label={t('home.cancel')} iconSize={13} className="h-6 w-6" onClick={onClose} />
        </div>

        <p className="text-[length:var(--app-text-xs)] text-[var(--text-muted)] mb-2.5 leading-relaxed">
          {t('home.batchHint')}
        </p>

        {/* 多行输入框：等宽字体，按行解析 */}
        <textarea
          value={text}
          onChange={(e) => setText(e.target.value)}
          rows={10}
          autoFocus
          spellCheck={false}
          className="w-full rounded-lg modern-card p-3 text-[length:var(--app-text-md)] font-mono leading-relaxed outline-none focus:border-[var(--accent)] resize-y min-h-[180px] max-h-[50vh]"
          placeholder={t('home.batchPlaceholder')}
        />

        {/* 底部状态行 + 操作按钮 */}
        <div className="flex items-center justify-between mt-3">
          <div className="flex items-center gap-3 text-[length:var(--app-text-xs)] text-[var(--text-muted)] tnum">
            <span className="flex items-center gap-1">
              {t('home.batchLines', { count: lines.length })}
            </span>
            {lines.length > 1 && (
              <span className="text-[length:var(--app-text-xs)] px-1.5 py-0.5 rounded bg-[var(--accent)]/10 text-[var(--accent)]">
                {t('home.batchQueue', { count: lines.length })}
              </span>
            )}
          </div>
          <div className="flex items-center gap-2">
            <kbd className="text-[length:var(--app-text-2xs)] px-1.5 py-0.5 rounded border border-[var(--border)] bg-[var(--bg-hover)] text-[var(--text-muted)] tnum">
              Ctrl+Enter
            </kbd>
            <Button variant="secondary" size="sm" onClick={onClose}>
              {t('home.cancel')}
            </Button>
            <Button variant="primary" size="sm" disabled={lines.length === 0} onClick={submit}>
              {t('home.batchSend')}
            </Button>
          </div>
        </div>
      </div>
    </div>
  )
}


/* ============ 会话导入预览弹层：解析 md/json → 只读预览 → 复制全文 ============
 * - 不写数据库（避免污染历史/触发 agent）
 * - 用户可复制后粘贴到任意会话作为参考上下文
 * - 顶部展示元信息（文件名 / 消息数 / 角色分布）+ 中间消息列表 + 底部"复制全文"按钮
 */
export function ImportDialog({ data, onClose }: {
  data: { title: string; messages: Array<{ role: string; content: string }> }
  onClose: () => void
}) {
  const { t } = useTranslation()
  const [copied, setCopied] = useState(false)
  const copyRef = useRef<HTMLButtonElement>(null)

  // 复制全文为 markdown（与 exportConversationMd 输出格式兼容）
  const copyAll = async () => {
    const md = data.messages
      .map((m) => {
        const role = m.role === 'user' ? '👤 User' : m.role === 'assistant' ? '🤖 Assistant' : `⚙️ ${m.role}`
        return `## ${role}\n\n${m.content}`
      })
      .join('\n\n---\n\n')
    try {
      await navigator.clipboard.writeText(md)
      setCopied(true)
      setTimeout(() => setCopied(false), 1500)
    } catch (e) {
      console.error('copy failed', e)
    }
  }

  // 统计
  const stats = useMemo(() => {
    let user = 0, assistant = 0, other = 0
    for (const m of data.messages) {
      if (m.role === 'user') user++
      else if (m.role === 'assistant') assistant++
      else other++
    }
    return { user, assistant, other }
  }, [data.messages])

  useEscapeKey(onClose)

  return (
    <div className="cmdk-backdrop" onClick={onClose}>
      <div
        className="w-[720px] max-w-[94vw] max-h-[86vh] flex flex-col glass-card animate-modal-in"
        onClick={(e) => e.stopPropagation()}
      >
        {/* 标题栏 */}
        <div className="flex items-center justify-between px-4 h-12 border-b border-[var(--border)] shrink-0">
          <div className="flex items-center gap-2 min-w-0">
            <div className="w-6 h-6 rounded-md bg-[var(--accent)]/15 text-[var(--accent)] flex items-center justify-center shrink-0">
              <Icon name="file" size={13} />
            </div>
            <h2 className="text-[length:var(--app-text-base)] font-semibold truncate" title={data.title}>{data.title}</h2>
            <span className="text-[length:var(--app-text-xs)] text-[var(--text-muted)] shrink-0 tnum">
              {t('home.importMsgCount', { count: data.messages.length })}
            </span>
          </div>
          <IconButton icon="close" label={t('home.cancel')} iconSize={13} className="h-6 w-6" onClick={onClose} />
        </div>

        {/* 角色分布小条 */}
        <div className="flex items-center gap-3 px-4 py-2 border-b border-[var(--border)] text-[length:var(--app-text-xs)] text-[var(--text-muted)] tnum shrink-0">
          <span className="flex items-center gap-1">
            <span className="w-1.5 h-1.5 rounded-full bg-[var(--accent)]" />
            {t('home.importRoleUser')}: {stats.user}
          </span>
          <span className="flex items-center gap-1">
            <span className="w-1.5 h-1.5 rounded-full bg-[var(--success)]" />
            {t('home.importRoleAssistant')}: {stats.assistant}
          </span>
          {stats.other > 0 && (
            <span className="flex items-center gap-1">
              <span className="w-1.5 h-1.5 rounded-full bg-[var(--text-muted)]" />
              {t('home.importRoleOther')}: {stats.other}
            </span>
          )}
          <span className="ml-auto text-[length:var(--app-text-2xs)] text-[var(--text-muted)]">{t('home.importPreviewHint')}</span>
        </div>

        {/* 消息列表（只读） */}
        <div className="flex-1 overflow-y-auto p-4 space-y-3 min-h-0">
          {data.messages.map((m, i) => {
            const isUser = m.role === 'user'
            return (
              <div key={i} className={`flex gap-2.5 ${isUser ? 'flex-row-reverse' : ''}`}>
                <div
                  className={`shrink-0 w-6 h-6 rounded-md flex items-center justify-center text-[length:var(--app-text-xs)] ${
                    isUser
                      ? 'bg-[var(--accent)]/15 text-[var(--accent)]'
                      : 'bg-[var(--success)]/15 text-[var(--success)]'
                  }`}
                  title={m.role}
                >
                  {isUser ? '👤' : '🤖'}
                </div>
                <div
                  className={`flex-1 min-w-0 rounded-lg p-2.5 text-[length:var(--app-text-sm)] leading-relaxed whitespace-pre-wrap break-words modern-card ${
                    isUser ? 'bg-[var(--accent)]/5' : ''
                  }`}
                >
                  {m.content || <span className="text-[var(--text-muted)] italic">（空消息）</span>}
                </div>
              </div>
            )
          })}
        </div>

        {/* 底部操作栏 */}
        <div className="flex items-center justify-between px-4 h-12 border-t border-[var(--border)] shrink-0">
          <span className="text-[length:var(--app-text-xs)] text-[var(--text-muted)]">{t('home.importReadOnly')}</span>
          <div className="flex items-center gap-2">
            <Button variant="secondary" size="sm" onClick={onClose}>
              {t('home.cancel')}
            </Button>
            <Button
              variant="primary"
              size="sm"
              ref={copyRef}
              icon={copied ? 'check' : 'copy'}
              onClick={copyAll}
            >
              {copied ? t('home.importCopied') : t('home.importCopyAll')}
            </Button>
          </div>
        </div>
      </div>
    </div>
  )
}
/* ============ 审计日志查看页：表格列出所有敏感操作（localStorage 持久化，上限 200） ============
 * - 顶部：标题 + 清空按钮 + 分类过滤 chips
 * - 主体：时间 / 分类 / 名称 / 详情（按时间倒序）
 * - 底部：记录条数
 */
export function AuditDialog({ onClose }: { onClose: () => void }) {
  const { t } = useTranslation()
  const entries = useAuditStore((s) => s.entries)
  const clear = useAuditStore((s) => s.clear)
  const [filter, setFilter] = useState<'all' | AuditCategory>('all')

  useEscapeKey(onClose)

  const filtered = useMemo(
    () => (filter === 'all' ? entries : entries.filter((e) => e.category === filter)).slice().sort((a, b) => b.ts - a.ts),
    [entries, filter],
  )

  // 分类 chips 计数
  const counts = useMemo(() => {
    const map: Record<string, number> = { all: entries.length }
    for (const e of entries) map[e.category] = (map[e.category] ?? 0) + 1
    return map
  }, [entries])

  // 时间格式：YYYY-MM-DD HH:mm:ss
  const fmtTs = (ts: number) => {
    const d = new Date(ts)
    const pad = (n: number) => String(n).padStart(2, '0')
    return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`
  }

  // 分类 → i18n label
  const catLabel = (c: AuditCategory) => t(`home.auditCat.${c}`)

  return (
    <div className="cmdk-backdrop" onClick={onClose}>
      <div
        className="w-[820px] max-w-[94vw] max-h-[86vh] flex flex-col glass-card animate-modal-in"
        onClick={(e) => e.stopPropagation()}
      >
        {/* 标题栏 */}
        <div className="flex items-center justify-between px-4 h-12 border-b border-[var(--border)] shrink-0">
          <div className="flex items-center gap-2">
            <div className="w-6 h-6 rounded-md bg-[var(--accent)]/15 text-[var(--accent)] flex items-center justify-center">
              <Icon name="history" size={13} />
            </div>
            <h2 className="text-[length:var(--app-text-base)] font-semibold">{t('home.auditTitle')}</h2>
            <span className="text-[length:var(--app-text-xs)] text-[var(--text-muted)] tnum">
              {t('home.auditCount', { count: entries.length })}
            </span>
          </div>
          <div className="flex items-center gap-2">
            <Button
              variant="danger"
              size="sm"
              confirm
              confirmLabel={t('common.confirmAgain')}
              title={t('home.auditClearConfirm')}
              disabled={entries.length === 0}
              onClick={() => {
                if (entries.length === 0) return
                clear()
              }}
            >
              {t('home.auditClear')}
            </Button>
            <IconButton icon="close" label={t('home.cancel')} iconSize={13} className="h-6 w-6" onClick={onClose} />
          </div>
        </div>

        {/* 分类过滤 chips */}
        <div className="flex items-center gap-1.5 px-4 py-2 border-b border-[var(--border)] overflow-x-auto shrink-0">
          <button
            onClick={() => setFilter('all')}
            className={`shrink-0 px-2 py-0.5 rounded-md text-[length:var(--app-text-xs)] transition-colors ${
              filter === 'all' ? 'tab-active' : 'tab-inactive'
            }`}
          >
            {t('home.auditCatAll')} ({counts.all ?? 0})
          </button>
          {(Object.keys(counts) as string[]).filter((c) => c !== 'all' && counts[c] > 0).map((c) => (
            <button
              key={c}
              onClick={() => setFilter(c as AuditCategory)}
              className={`shrink-0 px-2 py-0.5 rounded-md text-[length:var(--app-text-xs)] transition-colors ${
                filter === c ? 'tab-active' : 'tab-inactive'
              }`}
            >
              {catLabel(c as AuditCategory)} ({counts[c]})
            </button>
          ))}
        </div>

        {/* 列表 */}
        <div className="flex-1 overflow-y-auto min-h-0">
          {filtered.length === 0 ? (
            <p className="px-4 py-12 text-center text-[length:var(--app-text-md)] text-[var(--text-muted)]">
              {t('home.auditEmpty')}
            </p>
          ) : (
            <table className="w-full text-[length:var(--app-text-sm)]">
              <thead className="sticky top-0 bg-[var(--bg-card)] z-10">
                <tr className="border-b border-[var(--border)] text-[var(--text-muted)]">
                  <th className="text-left px-3 py-2 font-medium">{t('home.auditTime')}</th>
                  <th className="text-left px-3 py-2 font-medium">{t('home.auditCategory')}</th>
                  <th className="text-left px-3 py-2 font-medium">{t('home.auditName')}</th>
                  <th className="text-left px-3 py-2 font-medium">{t('home.auditDetail')}</th>
                </tr>
              </thead>
              <tbody>
                {filtered.map((e) => (
                  <tr key={e.id} className="border-b border-[var(--border)] last:border-0 align-top hover:bg-[var(--bg-hover)]">
                    <td className="px-3 py-2 whitespace-nowrap font-mono text-[length:var(--app-text-xs)] text-[var(--text-secondary)] tnum">{fmtTs(e.ts)}</td>
                    <td className="px-3 py-2">
                      <span className="px-1.5 py-0.5 rounded text-[length:var(--app-text-xs)] bg-[var(--accent)]/10 text-[var(--accent)]">
                        {catLabel(e.category)}
                      </span>
                    </td>
                    <td className="px-3 py-2 max-w-[180px] truncate" title={e.label}>{e.label}</td>
                    <td className="px-3 py-2 max-w-[320px] text-[var(--text-secondary)]">
                      {e.detail ? (
                        <span className="line-clamp-2" title={e.detail}>{e.detail}</span>
                      ) : (
                        <span className="text-[var(--text-muted)]">—</span>
                      )}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </div>
      </div>
    </div>
  )
}
