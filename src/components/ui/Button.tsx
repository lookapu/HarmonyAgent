/**
 * 按钮：全库唯一的按钮实现
 *
 * variant 与既有 CSS 类的对应关系（不重造样式，直接背书 index.css 里的分层）：
 *   primary   → .btn-primary（已含亮色覆盖与 .render-tier-low 降级）
 *   secondary → .btn-ghost  （同上，已有亮色覆盖）
 *   ghost / danger / soft → 本文件内的 Tailwind 组合
 *
 * className 约定（见 utils/cn.ts）：内部类名在前，外部 className 只可**附加**
 * margin / 宽度 / 定位类，不要传颜色或尺寸去覆盖——变体一律走 variant / size。
 *
 * 不写 transition-colors：index.css 里那条无层的 button 过渡规则优先级更高，
 * utilities 层的 transition-* 在按钮上根本不生效，写了只是徒增体积。
 *
 * type 默认 'button' 而非 HTML 的 'submit'：本项目没有 <form> 提交路径，
 * 默认 submit 只会在将来嵌进表单时制造意外刷新。
 */

import { useEffect, useState } from 'react'
import type { ButtonHTMLAttributes, Ref } from 'react'
import Icon from '../../icons/Icon'
import type { IconName } from '../../icons/Icon'
import { cn } from '../../utils/cn'
import { Spinner } from './Spinner'

const variantCls = {
  primary: 'btn-primary',
  secondary: 'btn-ghost',
  ghost:
    'border border-transparent bg-transparent text-[var(--text-secondary)] hover:bg-[var(--bg-hover)] hover:text-[var(--text-primary)]',
  danger:
    'border border-[var(--border)] bg-[var(--danger-50)] text-[var(--danger)] hover:border-[var(--danger)]/45',
  soft: 'border border-transparent bg-[var(--accent-100)] text-[var(--accent)] hover:bg-[var(--accent-soft)]',
} as const

const sizeCls = {
  xs: 'h-6 gap-1 px-2 text-[length:var(--app-text-xs)]',
  sm: 'h-7 gap-1.5 px-2.5 text-[length:var(--app-text-sm)]',
  md: 'h-8 gap-1.5 px-3 text-[length:var(--app-text-md)]',
} as const

/** 危险按钮 armed 态：实心 danger 底。文字用 --bg-window 而非 #fff——
 *  暗色主题下 --danger 是 #f87171（浅红），白字对比度只有 2.9:1；
 *  用主题自己的窗口底色（暗色近黑 / 亮色近白）两套主题都能过 AA。 */
const armedCls =
  'border border-transparent bg-[var(--danger)] text-[var(--bg-window)] hover:bg-[var(--danger)]'

const iconSizeFor = { xs: 11, sm: 12, md: 13 } as const

export interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  /** React 19 把 ref 当普通 prop；随 rest 透传到原生 button 上即可生效 */
  ref?: Ref<HTMLButtonElement>
  variant?: keyof typeof variantCls
  size?: keyof typeof sizeCls
  icon?: IconName
  iconRight?: IconName
  /** 进行中：显示 Spinner 并禁用点击。全库此前 0 处按钮有 loading 态。 */
  loading?: boolean
  /** 二次确认：首次点击进入 armed 态而不触发 onClick，2.5s 未确认自动解除 */
  confirm?: boolean
  /** 受控 armed 态；用于「列表内同时只允许一个按钮 armed」的跨按钮共享状态 */
  armed?: boolean
  /** 受控模式下的 arm 回调；不传则退化为组件内部 state */
  onArm?: () => void
  /** armed 态显示的文案，默认沿用 children */
  confirmLabel?: string
}

export function Button({
  variant = 'secondary',
  size = 'sm',
  icon,
  iconRight,
  loading = false,
  confirm = false,
  armed,
  onArm,
  confirmLabel,
  type = 'button',
  disabled,
  onClick,
  className,
  children,
  ...rest
}: ButtonProps) {
  const [armedLocal, setArmedLocal] = useState(false)
  const isArmed = confirm && (armed ?? armedLocal)

  useEffect(() => {
    if (!armedLocal) return
    const id = setTimeout(() => setArmedLocal(false), 2500)
    return () => clearTimeout(id)
  }, [armedLocal])

  const isDisabled = disabled || loading
  // 实心底色（primary / armed）上图标必须反白：Icon 是 <img> + filter 实现，
  // 默认 filter 跟随主题，在 accent 底上会发灰
  const whiteIcon = variant === 'primary' || isArmed

  return (
    <button
      {...rest}
      type={type}
      disabled={isDisabled}
      aria-busy={loading || undefined}
      onClick={(e) => {
        if (confirm && !isArmed) {
          if (onArm) onArm()
          else setArmedLocal(true)
          return
        }
        onClick?.(e)
      }}
      className={cn(
        'inline-flex shrink-0 select-none items-center justify-center whitespace-nowrap rounded-md font-medium disabled:cursor-not-allowed disabled:opacity-45',
        isArmed ? armedCls : variantCls[variant],
        sizeCls[size],
        loading && 'opacity-70',
        className,
      )}
    >
      {loading ? (
        <Spinner size={iconSizeFor[size]} />
      ) : (
        icon && (
          <span aria-hidden="true" className="shrink-0">
            <Icon name={icon} size={iconSizeFor[size]} white={whiteIcon} />
          </span>
        )
      )}
      {isArmed && confirmLabel ? confirmLabel : children}
      {iconRight && (
        <span aria-hidden="true" className="shrink-0">
          <Icon name={iconRight} size={iconSizeFor[size]} white={whiteIcon} />
        </span>
      )}
    </button>
  )
}
