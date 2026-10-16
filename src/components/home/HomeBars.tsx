/**
 * 会话顶部的两条信息条：会话笔记栏、Pin 消息条
 *
 * 从 Home.tsx 搬出（原 L8943-9140）。**纯代码移动，逻辑未改。**
 * 搬出时把两段说明注释各归其位：原文件里 Pinned 栏的注释排在会话笔记栏
 * 注释之前（历史遗留），现已分别贴到对应组件上方。
 */

import { useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { NOTE_MAX_LEN, useNoteStore } from '../../stores/noteStore'
import { usePinStore } from '../../stores/pinStore'
import { useProjectStore } from '../../stores/projectStore'
import { setConversationContextPin } from '../../api/project'
import Icon from '../../icons/Icon'
import { Button } from '../ui/Button'

/* ============ 会话笔记（顶部笔记区，per conversation 持久化） ============
 * - 默认收起（点 "📝 写笔记" 展开）
 * - 自动保存（debounce 600ms）
 * - 上限 4000 字（NOTE_MAX_LEN）
 * - 写过的会话：标题行显示 "📝" 标识 + 摘要预览
 * - 切换会话时表单值正确同步
 */
export function ConvNoteBar({ convId }: { convId: string }) {
  const { t } = useTranslation()
  const note = useNoteStore((s) => s.notes[convId] ?? null)
  const setNote = useNoteStore((s) => s.set)
  const [editing, setEditing] = useState(false)
  const [draft, setDraft] = useState(note?.text ?? '')
  const debounceRef = useRef<number | null>(null)

  // 切换会话：同步当前 draft 到最新值
  useEffect(() => {
    setDraft(note?.text ?? '')
    setEditing(false)
  }, [convId, note?.text])

  // debounce 自动保存
  useEffect(() => {
    if (!editing) return
    if (draft === (note?.text ?? '')) return
    if (debounceRef.current) window.clearTimeout(debounceRef.current)
    debounceRef.current = window.setTimeout(() => {
      setNote(convId, draft)
    }, 600)
    return () => {
      if (debounceRef.current) window.clearTimeout(debounceRef.current)
    }
  }, [draft, editing, convId, note?.text, setNote])

  // 编辑模式：全屏 textarea
  if (editing) {
    return (
      <div className="mb-3 rounded-lg glass-card border border-[var(--accent)]/30 p-2.5 animate-fade-in">
        <div className="flex items-center justify-between mb-1.5">
          <div className="flex items-center gap-1.5 text-[length:var(--app-text-xs)] font-medium text-[var(--accent)]">
            <Icon name="edit" size={11} />
            {t('home.noteEditingTitle')}
          </div>
          <div className="flex items-center gap-2">
            <span className={`text-[length:var(--app-text-2xs)] tnum ${draft.length > NOTE_MAX_LEN * 0.9 ? 'text-[var(--warning)]' : 'text-[var(--text-muted)]'}`}>
              {draft.length}/{NOTE_MAX_LEN}
            </span>
            <Button
              variant="primary"
              size="xs"
              onClick={() => {
                setNote(convId, draft)
                setEditing(false)
              }}
            >
              {t('home.noteDone')}
            </Button>
          </div>
        </div>
        <textarea
          value={draft}
          onChange={(e) => setDraft(e.target.value.slice(0, NOTE_MAX_LEN))}
          rows={4}
          autoFocus
          spellCheck={false}
          className="w-full resize-y rounded-md bg-[var(--bg-primary)] border border-[var(--border)] px-2.5 py-1.5 text-[length:var(--app-text-sm)] leading-relaxed outline-none focus:border-[var(--accent)] min-h-[80px] max-h-[200px]"
          placeholder={t('home.notePlaceholder')}
        />
      </div>
    )
  }

  // 折叠态：未写过 → 灰提示；写过 → 显示摘要 + 时间
  return (
    <div
      className={`mb-2 rounded-lg border transition-colors ${
        note
          ? 'border-[var(--accent)]/20 bg-[var(--accent)]/5'
          : 'border-dashed border-[var(--border)] bg-[var(--bg-secondary)]/40 hover:border-[var(--accent)]/30'
      }`}
    >
      <button
        onClick={() => setEditing(true)}
        className="w-full flex items-center gap-2 px-2.5 py-1.5 text-left"
      >
        <Icon name="edit" size={11} className={note ? 'text-[var(--accent)]' : 'text-[var(--text-muted)]'} />
        {note ? (
          <div className="flex-1 min-w-0">
            <div className="text-[length:var(--app-text-xs)] text-[var(--text-primary)] truncate">{note.text.split('\n')[0]}</div>
            <div className="text-[length:var(--app-text-2xs)] text-[var(--text-muted)] tnum mt-0.5">
              {t('home.noteUpdated', { time: new Date(note.updatedAt).toLocaleString() })}
            </div>
          </div>
        ) : (
          <span className="text-[length:var(--app-text-xs)] text-[var(--text-muted)] flex-1">{t('home.noteAdd')}</span>
        )}
        {note && (
          <span
            onClick={(e) => {
              e.stopPropagation()
              if (window.confirm(t('home.noteClearConfirm'))) {
                useNoteStore.getState().clear(convId)
              }
            }}
            className="p-1 text-[var(--text-muted)] hover:text-[var(--danger)] opacity-0 group-hover:opacity-100"
            aria-label={t('home.noteClear')}
            title={t('home.noteClear')}
          >
            <Icon name="close" size={11} />
          </span>
        )}
      </button>
    </div>
  )
}
/* ============ Pinned 消息条：会话顶部钉住关键消息（仅前端 localStorage） ============
 * - 会话长了之后找回"报错信息/关键决策/成功命令"很痛 → 让用户手动 pin
 * - 数据结构：usePinStore（per conversationId，localStorage 持久化，最多 8 条）
 * - 点 chip 滚动到该消息（onJump 回调）；点 × 取消 pin
 * - 静默跳过"已 pin 但消息被删"的孤儿 id
 */
export function PinnedBar({ convId, onJump }: { convId: string; onJump: (msgId: string) => void }) {
  const { t } = useTranslation()
  // ⚠️ ?? [] 必须在 selector 外：写在 selector 内时无 pin 会话每次返回新数组引用，
  // useSyncExternalStore 判定快照持续变化 → 无限重渲染（Maximum update depth exceeded）
  const pins = usePinStore((s) => s.pins[convId]) ?? []
  const unpin = usePinStore((s) => s.unpin)
  // 从 store 取消息（避免 prop drilling）
  const messages = useProjectStore((s) => s.messages)
  if (pins.length === 0) return null
  // 过滤已删除的消息 + 按 pin 顺序展示
  const items = pins
    .map((id) => messages.find((m) => m.id === id))
    .filter((m): m is NonNullable<typeof m> => m != null)
  if (items.length === 0) return null
  return (
    <div className="mb-3 rounded-lg glass-card p-2 animate-fade-in">
      <div className="flex items-center gap-1.5 px-1 mb-1.5">
        <Icon name="pin" size={11} className="text-[var(--accent)]" />
        <span className="text-[length:var(--app-text-xs)] font-medium text-[var(--text-muted)] uppercase tracking-wider">
          {t('home.pinnedTitle')} · {items.length}
        </span>
      </div>
      <div className="flex flex-wrap gap-1.5">
        {items.map((m) => {
          const isUser = m.role === 'user'
          const preview = m.content.replace(/[#*`>_~\n]+/g, ' ').replace(/\s+/g, ' ').trim().slice(0, 60)
          const tone = isUser ? 'var(--accent)' : 'var(--success)'
          return (
            <div
              key={m.id}
              className="group flex items-center gap-1.5 max-w-[280px] rounded-md bg-[var(--bg-card)] border border-[var(--border)] hover:border-[var(--accent)]/40 transition-colors"
            >
              <button
                onClick={() => onJump(m.id)}
                className="flex-1 min-w-0 flex items-center gap-1.5 px-2 py-1 text-left"
                title={m.content}
              >
                <span
                  className="shrink-0 w-1.5 h-1.5 rounded-full"
                  style={{ background: tone }}
                />
                <span className="text-[length:var(--app-text-xs)] text-[var(--text-muted)] shrink-0">
                  {isUser ? '👤' : '🤖'}
                </span>
                <span className="text-[length:var(--app-text-xs)] text-[var(--text-primary)] truncate">
                  {preview || t('home.pinnedEmpty')}
                </span>
              </button>
              <button
                onClick={() => {
                  unpin(convId, m.id)
                  void setConversationContextPin({
                    conversation_id: convId,
                    pin_kind: 'message',
                    source_ref: `message:${m.id}`,
                    label: m.role,
                    content: m.content,
                    pinned: false,
                  }).catch(() => {})
                }}
                className="opacity-0 group-hover:opacity-100 p-1 text-[var(--text-muted)] hover:text-[var(--danger)] transition-[color,background-color,border-color,opacity] shrink-0"
                aria-label={t('home.unpinFromTop')}
                title={t('home.unpinFromTop')}
              >
                <Icon name="close" size={10} />
              </button>
            </div>
          )
        })}
      </div>
    </div>
  )
}
