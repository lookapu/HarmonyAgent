import { Fragment, type ReactNode } from 'react'

/**
 * 轻量 ANSI SGR 转 React 节点：
 * - 支持 30-37 / 40-47 基础 8 色，90-97 / 100-107 亮色
 * - 支持 38;5;n / 48;5;n 256 色与 38;2;r;g;b / 48;2;r;g;b 真彩色
 * - 支持样式：1 加粗、2 暗淡、3 斜体、4 下划线、7 反色、9 删除线
 * - 自动剥离其他 CSI 序列（光标移动、清屏等）
 * 用于工具终端输出（run_command/hdc 等带颜色的构建/部署日志）。
 */

const ANSI_RE = /\x1b\[([0-9;]*)m/g
const CSI_STRIP_RE = /\x1b\[[0-9;?]*[A-Za-z]/g

// 标准 16 色（与终端一致）
const BASE_COLORS = [
  '#000000', '#cc0000', '#4e9a06', '#c4a000', '#3465a4', '#75507b', '#06989a', '#d3d7cf',
  '#555753', '#ef2929', '#8ae234', '#fce94f', '#729fcf', '#ad7fa8', '#34e2e2', '#eeeeec',
]

function color256(n: number): string {
  if (n < 16) return BASE_COLORS[n] ?? '#cccccc'
  if (n >= 232) {
    const v = 8 + (n - 232) * 10
    return `rgb(${v},${v},${v})`
  }
  const idx = n - 16
  const r = Math.floor(idx / 36)
  const g = Math.floor((idx % 36) / 6)
  const b = idx % 6
  const conv = (c: number) => (c === 0 ? 0 : 55 + c * 40)
  return `rgb(${conv(r)},${conv(g)},${conv(b)})`
}

interface Style {
  color?: string
  bg?: string
  bold?: boolean
  dim?: boolean
  italic?: boolean
  underline?: boolean
  strike?: boolean
  reverse?: boolean
}

function applySgr(style: Style, codes: number[]): Style {
  const next: Style = { ...style }
  let i = 0
  while (i < codes.length) {
    const c = codes[i]
    switch (c) {
      case 0:
        return {}
      case 1:
        next.bold = true
        break
      case 2:
        next.dim = true
        break
      case 3:
        next.italic = true
        break
      case 4:
        next.underline = true
        break
      case 7:
        next.reverse = true
        break
      case 9:
        next.strike = true
        break
      case 22:
        next.bold = false
        next.dim = false
        break
      case 23:
        next.italic = false
        break
      case 24:
        next.underline = false
        break
      case 27:
        next.reverse = false
        break
      case 29:
        next.strike = false
        break
      case 38: {
        // 38;5;n 或 38;2;r;g;b
        if (codes[i + 1] === 5) {
          next.color = color256(codes[i + 2] ?? 0)
          i += 2
        } else if (codes[i + 1] === 2) {
          next.color = `rgb(${codes[i + 2]},${codes[i + 3]},${codes[i + 4]})`
          i += 4
        }
        break
      }
      case 48: {
        if (codes[i + 1] === 5) {
          next.bg = color256(codes[i + 2] ?? 0)
          i += 2
        } else if (codes[i + 1] === 2) {
          next.bg = `rgb(${codes[i + 2]},${codes[i + 3]},${codes[i + 4]})`
          i += 4
        }
        break
      }
      case 39:
        next.color = undefined
        break
      case 49:
        next.bg = undefined
        break
      default:
        if (c >= 30 && c <= 37) next.color = BASE_COLORS[c - 30]
        else if (c >= 40 && c <= 47) next.bg = BASE_COLORS[c - 40]
        else if (c >= 90 && c <= 97) next.color = BASE_COLORS[c - 90 + 8]
        else if (c >= 100 && c <= 107) next.bg = BASE_COLORS[c - 100 + 8]
        break
    }
    i++
  }
  return next
}

function styleToCss(s: Style): React.CSSProperties {
  const css: React.CSSProperties = {}
  const fg = s.reverse ? s.bg : s.color
  const bg = s.reverse ? s.color : s.bg
  if (fg) css.color = fg
  if (bg) css.backgroundColor = bg
  if (s.bold) css.fontWeight = 600
  if (s.dim) css.opacity = 0.7
  if (s.italic) css.fontStyle = 'italic'
  if (s.underline) css.textDecoration = 'underline'
  if (s.strike) css.textDecorationLine = 'line-through'
  return css
}

/** 判断文本是否包含 ANSI SGR 颜色序列（供调用方决定是否走彩色渲染） */
export function hasAnsi(text: string): boolean {
  return /\x1b\[[0-9;]*m/.test(text)
}

/**
 * 将带 ANSI 转义码的字符串渲染为带样式的 React 节点。
 * 非 ANSI 的控制序列（光标移动/清屏等）直接剥离。
 */
export function AnsiText({ text, className }: { text: string; className?: string }): ReactNode {
  // 先剥离非 SGR 的 CSI 序列
  const cleaned = text.replace(CSI_STRIP_RE, '')
  const parts: ReactNode[] = []
  let last = 0
  let style: Style = {}
  let key = 0
  let m: RegExpExecArray | null
  ANSI_RE.lastIndex = 0
  while ((m = ANSI_RE.exec(cleaned)) !== null) {
    if (m.index > last) {
      parts.push(run(cleaned.slice(last, m.index), style, key++))
    }
    const codes = m[1]
      .split(';')
      .filter(Boolean)
      .map((x) => Number(x))
    style = applySgr(style, codes)
    last = m.index + m[0].length
  }
  if (last < cleaned.length) {
    parts.push(run(cleaned.slice(last), style, key++))
  }
  return <span className={className}>{parts.length === 0 ? text : parts}</span>
}

function run(text: string, style: Style, key: number): ReactNode {
  if (!text) return null
  const css = styleToCss(style)
  const hasStyle = Object.keys(css).length > 0
  if (!hasStyle) {
    // 保留换行（pre 容器已处理空白，但 Fragment 更轻量）
    return <Fragment key={key}>{text}</Fragment>
  }
  return (
    <span key={key} style={css}>
      {text}
    </span>
  )
}
