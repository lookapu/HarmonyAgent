/**
 * 通用确认弹层：替代 window.confirm，从 Home.tsx 原样迁出
 *
 * 公开 props 与迁移前一字未改（含 tone 的 'danger' | 'warn' | 'info' 三个字面量、
 * requireInput 的「键入指定短语才解锁确认」语义），调用点无需改动。
 * i18n 仍走 home.* 键而非 common.*：home.confirm 是「确定」而 common.confirm 是
 * 「确认」，换命名空间会改用户可见文案。
 *
 * 三处行为修正：
 * 1. 图标底色的 `${accent}20` 是坏值——accent 已经是 "var(--danger)" 这种字符串，
 *    拼出来的 "var(--danger)20" 不是合法颜色，浏览器直接丢弃，所以那个方块此前
 *    一直是透明的。改为把 tone 图标交给 Modal 的头部，不再自绘底色块。
 * 2. 原来同时「自动聚焦确认按钮」和「window 上监听 Enter 调 onConfirm」，Enter 会
 *    触发两次 onConfirm（原生 click + 全局 handler）。去掉全局 Enter，保留自动聚焦，
 *    原生行为已经覆盖。
 * 3. 重置输入态的 setTimeout 没有清理，弹层在 30ms 内卸载会往已卸载节点上聚焦。
 *
 * 确认按钮的文字色用 --bg-window 而非 #fff：暗色主题下 --danger 是 #f87171，
 * 白字对比度只有 2.9:1；主题窗口底色在两套主题下都能过 AA。
 */

import { useEffect, useRef, useState } from 'react'
import type { ReactNode } from 'react'
import { useTranslation } from 'react-i18next'
import { Button } from './Button'
import { Field } from './Field'
import { Modal } from './Modal'

export interface ConfirmDialogProps {
  open: boolean
  title: string
  body: ReactNode
  tone?: 'danger' | 'warn' | 'info'
  confirmLabel?: string
  cancelLabel?: string
  /** 若提供此短语，用户必须在输入框中键入完全匹配的字符串才解锁确认 */
  requireInput?: string
  onConfirm: () => void
  onCancel: () => void
}

export function ConfirmDialog({
  open,
  title,
  body,
  tone = 'danger',
  confirmLabel,
  cancelLabel,
  requireInput,
  onConfirm,
  onCancel,
}: ConfirmDialogProps) {
  const { t } = useTranslation()
  const [typed, setTyped] = useState('')
  const inputRef = useRef<HTMLInputElement>(null)
  const confirmBtnRef = useRef<HTMLButtonElement>(null)

  useEffect(() => {
    if (!open) return
    setTyped('')
    const id = setTimeout(
      () => (requireInput ? inputRef.current?.focus() : confirmBtnRef.current?.focus()),
      30,
    )
    return () => clearTimeout(id)
  }, [open, requireInput])

  if (!open) return null

  const canConfirm = !requireInput || typed === requireInput
  const accent = tone === 'danger' ? 'var(--danger)' : tone === 'warn' ? 'var(--warning)' : 'var(--accent)'

  return (
    <Modal
      open={open}
      onClose={onCancel}
      title={title}
      icon={tone === 'info' ? 'info' : 'archive'}
      size="sm"
      align="top"
      maxHeight="none"
      footer={
        <>
          <Button variant="secondary" size="md" onClick={onCancel}>
            {cancelLabel ?? t('home.cancel')}
          </Button>
          <Button
            ref={confirmBtnRef}
            variant="primary"
            size="md"
            disabled={!canConfirm}
            onClick={onConfirm}
            style={
              canConfirm
                ? { background: accent, color: 'var(--bg-window)' }
                : { background: 'var(--bg-hover)', color: 'var(--text-muted)' }
            }
          >
            {confirmLabel ?? t('home.confirm')}
          </Button>
        </>
      }
    >
      <div className="text-[length:var(--app-text-sm)] leading-relaxed text-[var(--text-secondary)]">
        {body}
      </div>
      {requireInput && (
        <Field
          ref={inputRef}
          className="mt-3"
          mono
          fieldSize="md"
          label={t('home.confirmTypePhrase', { phrase: requireInput })}
          value={typed}
          onChange={(e) => setTyped(e.target.value)}
          spellCheck={false}
          autoComplete="off"
          placeholder={requireInput}
        />
      )}
    </Modal>
  )
}
