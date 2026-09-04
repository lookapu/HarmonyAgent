/**
 * 表单控件：输入框与多行文本域
 *
 * 补齐此前全库 0 处的 error / disabled 态：error 会同时改边框色、置
 * aria-invalid，并把错误文案用 aria-describedby 关联到控件上——读屏用户此前
 * 完全拿不到校验失败的原因。
 *
 * 不写 focus 类：index.css 里那条无层的 input:focus-visible 规则（border 变
 * accent-500 + 2px ring）优先级高于 utilities，写了也是死类。副作用是错误态
 * 聚焦时边框会被 accent 覆盖，但 aria-invalid 与错误文案仍在，可接受。
 *
 * 底色统一 --bg-secondary：此前 11 处输入框在 --bg-primary / --bg-secondary /
 * .modern-card 三种底之间随机分布。
 */

import { useId } from 'react'
import type { InputHTMLAttributes, ReactNode, Ref, TextareaHTMLAttributes } from 'react'
import { cn } from '../../utils/cn'

const controlCls =
  'w-full rounded-md border bg-[var(--bg-secondary)] text-[var(--text-primary)] placeholder:text-[var(--text-muted)] disabled:cursor-not-allowed disabled:opacity-50'

const inputSizeCls = {
  xs: 'h-6 px-1.5 text-[length:var(--app-text-xs)]',
  sm: 'h-7 px-2 text-[length:var(--app-text-sm)]',
  md: 'h-8 px-2.5 text-[length:var(--app-text-md)]',
} as const

const areaSizeCls = {
  xs: 'px-1.5 py-1 text-[length:var(--app-text-xs)]',
  sm: 'px-2 py-1.5 text-[length:var(--app-text-sm)]',
  md: 'px-2.5 py-1.5 text-[length:var(--app-text-md)]',
} as const

export type FieldSize = keyof typeof inputSizeCls

interface SharedProps {
  label?: ReactNode
  /** 常驻辅助说明，与 error 可同时存在 */
  hint?: ReactNode
  /** 校验失败文案；置位即进入错误态 */
  error?: ReactNode
  mono?: boolean
  fieldSize?: FieldSize
  className?: string
}

function controlClass(
  size: FieldSize,
  mono: boolean,
  error: boolean,
  sizes: Record<FieldSize, string>,
  extra?: string,
): string {
  return cn(
    controlCls,
    error ? 'border-[var(--danger)]' : 'border-[var(--border)]',
    sizes[size],
    mono && 'font-mono',
    extra,
  )
}

function Wrap({
  label,
  hint,
  error,
  ids,
  className,
  children,
}: SharedProps & { ids: { input: string; hint?: string; err?: string }; children: ReactNode }) {
  return (
    <div className={cn('min-w-0', className)}>
      {label != null && (
        <label
          htmlFor={ids.input}
          className="mb-1 block text-[length:var(--app-text-xs)] text-[var(--text-muted)]"
        >
          {label}
        </label>
      )}
      {children}
      {hint != null && (
        <p
          id={ids.hint}
          className="mt-1 text-[length:var(--app-text-2xs)] leading-relaxed text-[var(--text-muted)]"
        >
          {hint}
        </p>
      )}
      {error != null && (
        <p
          id={ids.err}
          className="mt-1 text-[length:var(--app-text-2xs)] leading-relaxed text-[var(--danger)]"
        >
          {error}
        </p>
      )}
    </div>
  )
}

export interface FieldProps extends SharedProps, InputHTMLAttributes<HTMLInputElement> {
  ref?: Ref<HTMLInputElement>
}

export function Field({
  label,
  hint,
  error,
  mono = false,
  fieldSize = 'sm',
  className,
  id,
  ...rest
}: FieldProps) {
  const autoId = useId()
  const inputId = id ?? autoId
  const ids = {
    input: inputId,
    hint: hint != null ? `${inputId}-hint` : undefined,
    err: error != null ? `${inputId}-err` : undefined,
  }
  return (
    <Wrap label={label} hint={hint} error={error} ids={ids} className={className}>
      <input
        {...rest}
        id={inputId}
        aria-invalid={error != null || undefined}
        aria-describedby={cn(ids.hint, ids.err) || undefined}
        className={controlClass(fieldSize, mono, error != null, inputSizeCls)}
      />
    </Wrap>
  )
}

export interface TextAreaProps extends SharedProps, TextareaHTMLAttributes<HTMLTextAreaElement> {
  ref?: Ref<HTMLTextAreaElement>
}

export function TextArea({
  label,
  hint,
  error,
  mono = false,
  fieldSize = 'sm',
  className,
  id,
  ...rest
}: TextAreaProps) {
  const autoId = useId()
  const inputId = id ?? autoId
  const ids = {
    input: inputId,
    hint: hint != null ? `${inputId}-hint` : undefined,
    err: error != null ? `${inputId}-err` : undefined,
  }
  return (
    <Wrap label={label} hint={hint} error={error} ids={ids} className={className}>
      <textarea
        {...rest}
        id={inputId}
        aria-invalid={error != null || undefined}
        aria-describedby={cn(ids.hint, ids.err) || undefined}
        className={controlClass(fieldSize, mono, error != null, areaSizeCls, 'resize-y leading-relaxed min-h-[80px]')}
      />
    </Wrap>
  )
}
