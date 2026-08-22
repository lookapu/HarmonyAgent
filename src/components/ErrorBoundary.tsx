// ============================================================
// React ErrorBoundary：兜底渲染错误，避免整页空白
// ============================================================
//
// 设计目标：
// - 默认 React 组件 throw 时会让整个组件树卸载 → 用户看到白屏
// - ErrorBoundary 捕获 throw → 显示错误详情 + 重置按钮
// - 避免"白屏"陷阱：哪怕子组件崩了，至少能看到错误信息和重试入口
// - 简单实现：useState 持有 error，componentDidCatch 设置
import { Component, type ReactNode } from 'react'

interface Props {
  children: ReactNode
  /** 自定义错误展示（不传则用默认的紧凑面板） */
  fallback?: (error: Error, reset: () => void) => ReactNode
}

interface State {
  error: Error | null
}

/** 通用 ErrorBoundary：把白屏变成可读错误 + 重置入口 */
export class ErrorBoundary extends Component<Props, State> {
  state: State = { error: null }

  static getDerivedStateFromError(error: Error): State {
    return { error }
  }

  componentDidCatch(error: Error, info: { componentStack?: string }) {
    // 简单 console 即可；后续可接 Sentry 等
    console.error('[ErrorBoundary] Caught error:', error, info.componentStack)
    this.componentStack = info.componentStack
  }

  componentStack: string | undefined

  getDetail = () => `${this.componentStack ?? ''}\n${this.state.error?.stack ?? ''}`

  reset = () => this.setState({ error: null })

  render() {
    const { error } = this.state
    const { children, fallback } = this.props
    if (error) {
      if (fallback) return fallback(error, this.reset)
      return (
        <div className="h-full w-full flex items-center justify-center p-6 bg-[var(--bg-window)]">
          <div className="w-[560px] max-w-[92vw] rounded-2xl glass-card p-5 animate-modal-in">
            <div className="flex items-start gap-3 mb-3">
              <div className="w-9 h-9 rounded-lg bg-[var(--danger)]/15 text-[var(--danger)] flex items-center justify-center shrink-0">
                <span className="text-[18px]">⚠️</span>
              </div>
              <div className="flex-1 min-w-0">
                <h2 className="text-[15px] font-semibold leading-tight">页面出现错误</h2>
                <p className="text-[12px] text-[var(--text-muted)] mt-1 leading-relaxed">
                  请复制下方错误信息并反馈。点击"重试"通常能恢复。
                </p>
              </div>
            </div>
            <div className="rounded-lg bg-[var(--bg-primary)] border border-[var(--border)] p-3 mb-3 max-h-[260px] overflow-y-auto">
              <p className="text-[12.5px] font-mono text-[var(--danger)] break-all whitespace-pre-wrap leading-relaxed">
                {error.name}: {error.message}
              </p>
            </div>
            <div className="flex items-center justify-end gap-2">
              <button
                onClick={() => {
                  // 复制到剪贴板
                  try {
                    void navigator.clipboard.writeText(`${error.name}: ${error.message}\n${this.getDetail()}`)
                  } catch {
                    // 剪贴板不可用 → 静默
                  }
                }}
                className="h-8 px-3 rounded-lg border border-[var(--border)] text-[12px] hover:bg-[var(--bg-hover)] transition-colors"
              >
                复制错误
              </button>
              <button
                onClick={this.reset}
                className="h-8 px-4 rounded-lg btn-primary text-[12px] font-medium active:scale-[0.98] transition-all"
              >
                重试
              </button>
            </div>
          </div>
        </div>
      )
    }
    return children
  }
}
