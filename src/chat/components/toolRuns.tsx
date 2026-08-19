import { memo, useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import type { ToolRun, AgentRun } from '../../stores/projectStore'
import Icon from '../../icons/Icon'
import { AnsiText, hasAnsi } from '../../components/AnsiText'
import { fmtElapsed } from '../chatUtils'
import { getItem, setItem } from '../../utils/storage'
import { STORAGE_KEYS } from '../../constants'

/* ============ 工具调用折叠组：一行展示（最后一次调用），点击展开全部 ============ */
export const ToolRunGroup = memo(function ToolRunGroup({ runs, onRetry, onCancel }: { runs: ToolRun[]; onRetry?: (run: ToolRun) => void; onCancel?: (run: ToolRun) => void }) {
  const { t } = useTranslation()
  const [open, setOpen] = useState(false)
  const last = runs[runs.length - 1]
  const running = runs.some((r) => r.status === 'running')
  const errCount = runs.filter((r) => r.status === 'error').length
  const doneCount = runs.filter((r) => r.status === 'done').length

  return (
    <div className="animate-fade-in-up tool-group">
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        className="w-full flex items-center gap-2 py-1.5 text-left transition-colors hover:opacity-80"
        title={t('home.toggleToolCalls')}
      >
        <Icon
          name="bolt"
          size={11}
          className={running ? 'text-[var(--warning)]' : errCount > 0 ? 'text-[var(--danger)]' : 'text-[var(--success)]'}
        />
        <span className="text-[12px] text-[var(--text-secondary)] truncate">
          {t('home.toolCalls', { count: runs.length })}
          {last && <span className="text-[var(--text-muted)]"> · {last.tool}</span>}
        </span>
        <span className="text-[11px] shrink-0 tabular-nums ml-auto">
          {errCount > 0 ? (
            <span className="text-[var(--danger)]">{t('home.toolFailed')} ×{errCount}</span>
          ) : running ? (
            <span className="text-[var(--warning)]">
              <span className="inline-block w-2.5 h-2.5 rounded-full border border-[var(--warning)] border-t-transparent animate-spin align-middle mr-1" />
              {t('home.toolRunning')}
            </span>
          ) : (
            <span className="text-[var(--text-muted)]">{t('home.toolDone')} ×{doneCount}</span>
          )}
        </span>
        <Icon name="chevron-right" size={11} className={`text-[var(--text-muted)] transition-transform ${open ? 'rotate-90' : ''}`} />
      </button>
      {open && (
        <div className="border-t border-[var(--border)]/60 divide-y divide-[var(--border)]/50 max-h-80 overflow-y-auto">
          {runs.map((r) => (
            <ToolRunRow key={r.id} run={r} onRetry={onRetry} onCancel={onCancel} />
          ))}
        </div>
      )}
    </div>
  )
})

/** 展开区单行：工具名 + 参数 + 状态，点击展开执行输出（终端样式）；失败时提供一键重试，运行中提供取消 */
export const ToolRunRow = memo(function ToolRunRow({ run, onRetry, onCancel }: { run: ToolRun; onRetry?: (run: ToolRun) => void; onCancel?: (run: ToolRun) => void }) {
  const { t } = useTranslation()
  // 默认展开规则（用户可手动覆盖，状态写到 localStorage）：
  // 1) 命令类（run_command 等）：用户大概率想看输出
  // 2) 失败状态：需要立即看到失败原因
  // 3) 其他：默认折叠（长任务视觉清爽）
  const isCommandTool = ['run_command', 'run_script', 'exec_command', 'terminal', 'run_command_shell'].includes(run.tool)
  const isFailed = run.status === 'error'
  const memKey = STORAGE_KEYS.TOOL_OPEN_PREFIX + run.tool
  const [open, setOpen] = useState<boolean>(() => {
    const saved = getItem(memKey)
    // 失败状态每次都强制展开（不被用户历史记忆覆盖）
    if (isFailed && saved === null) return true
    if (saved === null) return isCommandTool
    return saved === '1'
  })
  const toggleOpen = () => {
    setOpen((v) => {
      const next = !v
      setItem(memKey, next ? '1' : '0')
      return next
    })
  }
  const [copied, setCopied] = useState(false)
  // running 态计时：每秒刷新已运行时长（静默执行的工具也能看到进度）
  const [elapsed, setElapsed] = useState(0)
  useEffect(() => {
    if (run.status !== 'running' || !run.startedAt) {
      setElapsed(0)
      return
    }
    setElapsed(Math.floor((Date.now() - run.startedAt) / 1000))
    const timer = setInterval(() => setElapsed(Math.floor((Date.now() - (run.startedAt ?? Date.now())) / 1000)), 1000)
    return () => clearInterval(timer)
  }, [run.status, run.startedAt])

  const isErr = run.status === 'error'
  const running = run.status === 'running'
  const done = run.status === 'done' || run.status === 'error'
  // 展开内容：完成=最终输出（无输出时回退流式记录）；运行中=实时流式输出
  const displayOutput = done ? run.output || run.liveOutput || '' : run.liveOutput ?? ''
  // 运行中实时输出自动跟随（每次新行滚到底部）
  const liveRef = useRef<HTMLPreElement>(null)
  useEffect(() => {
    if (running && open && liveRef.current) {
      liveRef.current.scrollTop = liveRef.current.scrollHeight
    }
  }, [run.liveOutput, running, open])
  // 风险等级徽标配色：L0 只读=绿 / L1 写入=橙 / L2 危险=红
  const levelColor =
    run.level === 'L2'
      ? 'text-[var(--danger)] bg-[var(--danger)]/10'
      : run.level === 'L1'
        ? 'text-[var(--warning)] bg-[var(--warning)]/10'
        : 'text-[var(--success)] bg-[var(--success)]/10'
  const statusColor = run.status === 'running' ? 'text-[var(--accent)]' : isErr ? 'text-[var(--danger)]' : 'text-[var(--success)]'
  // 耗时标签：运行中=已运行 xx；完成=总耗时（带状态词）
  const durationLabel =
    run.status === 'running'
      ? run.startedAt
        ? `${t('home.toolRunning')} ${fmtElapsed(elapsed)}`
        : t('home.toolRunning')
      : run.durationMs != null
        ? `${isErr ? t('home.toolFailed') : t('home.toolDone')} · ${fmtElapsed(run.durationMs / 1000)}`
        : isErr
          ? t('home.toolFailed')
          : t('home.toolDone')

  const copyOutput = async () => {
    try {
      await navigator.clipboard.writeText(run.output)
      setCopied(true)
      setTimeout(() => setCopied(false), 1200)
    } catch {
      // 剪贴板不可用时静默失败
    }
  }

  return (
    <div>
      <button
        type="button"
        onClick={() => (done || running) && toggleOpen()}
        className={`w-full flex items-center gap-2 py-1.5 text-left transition-colors ${done || running ? 'hover:opacity-80' : ''}`}
      >
        <Icon
          name={isErr ? 'close' : 'bolt'}
          size={10}
          className={isErr ? 'text-[var(--danger)]' : running ? 'text-[var(--accent)]' : 'text-[var(--text-muted)]'}
        />
        <div className="flex-1 min-w-0">
          <div className="flex items-center gap-1.5 min-w-0">
            <div className="text-[12px] truncate text-[var(--text-secondary)]">{run.tool}</div>
            {run.level && (
              <span
                className={`text-[9px] font-semibold shrink-0 ${levelColor}`}
                title={run.level === 'L2' ? t('home.toolLevelL2') : run.level === 'L1' ? t('home.toolLevelL1') : t('home.toolLevelL0')}
              >
                {run.level}
              </span>
            )}
          </div>
          {run.args && (
            <div className="text-[10px] text-[var(--text-muted)] truncate font-mono mt-px" title={run.desc || run.args}>
              {run.args}
            </div>
          )}
        </div>
        <span className={`text-[11px] shrink-0 tabular-nums ${statusColor}`}>
          {running && (
            <span className="inline-block w-2.5 h-2.5 rounded-full border border-[var(--accent)] border-t-transparent animate-spin align-middle mr-1" />
          )}
          {durationLabel}
        </span>
        {(done || running) && <Icon name="chevron-right" size={11} className={`text-[var(--text-muted)] transition-transform ${open ? 'rotate-90' : ''}`} />}
      </button>
      {(done || running) && open && (
        <div className="bg-[#0d1117] border-t border-[var(--border)]">
          {/* 终端标题栏：mac 圆点 + 工具名 + 状态 + 复制输出 */}
          <div className="flex items-center gap-1.5 px-3 py-1.5 border-b border-white/10">
            <span className="w-2 h-2 rounded-full bg-[#ff5f56] shrink-0" />
            <span className="w-2 h-2 rounded-full bg-[#ffbd2e] shrink-0" />
            <span className="w-2 h-2 rounded-full bg-[#27c93f] shrink-0" />
            <span className="ml-1.5 text-[10px] font-mono text-[#8b949e] truncate">{run.tool}</span>
            {run.level && (
              <span className={`text-[9px] px-1 py-px rounded shrink-0 font-semibold ${levelColor}`}>{run.level}</span>
            )}
            <span className={`ml-auto text-[10px] shrink-0 ${statusColor}`}>{durationLabel}</span>
            <button
              type="button"
              onClick={copyOutput}
              title={t('home.copyOutput')}
              className="p-1 rounded text-[#8b949e] hover:text-[#e6edf3] hover:bg-white/10 transition-colors"
            >
              <Icon name="copy" size={11} className={copied ? 'text-[#3fb950]' : ''} />
            </button>
            {/* [59] 单工具取消：运行中显示 abort 按钮（后端 stop_tool 会话级中断） */}
            {running && onCancel && (
              <button
                type="button"
                onClick={() => onCancel(run)}
                title={t('home.stopTool')}
                className="flex items-center gap-1 px-1.5 py-0.5 rounded text-[#8b949e] hover:text-[#f85149] hover:bg-white/10 transition-colors text-[10px] shrink-0"
              >
                <Icon name="close" size={10} />
                {t('home.stopTool')}
              </button>
            )}
            {/* 失败工具一键重试：注入指令让 Agent 重新执行（失败输出截断后附给模型） */}
            {isErr && onRetry && (
              <button
                type="button"
                onClick={() => onRetry(run)}
                title={t('home.toolRetryHint')}
                className="flex items-center gap-1 px-1.5 py-0.5 rounded text-[#8b949e] hover:text-[#e6edf3] hover:bg-white/10 transition-colors text-[10px] shrink-0"
              >
                <Icon name="refresh" size={10} />
                {t('home.retry')}
              </button>
            )}
          </div>
          <pre
            ref={liveRef}
            className="px-3.5 py-2 text-[11px] font-mono whitespace-pre-wrap break-all leading-relaxed text-[#c9d1d9] max-h-64 overflow-y-auto"
          >
            {displayOutput ? (
              hasAnsi(displayOutput) ? (
                <AnsiText text={displayOutput} />
              ) : (
                displayOutput
              )
            ) : running ? (
              t('home.toolWaitingOutput')
            ) : (
              t('home.toolNoOutput')
            )}
          </pre>
        </div>
      )}
    </div>
  )
})

/* ============ 子 Agent 卡片（并行委派，Claude Code subagent 式） ============ */
export const AgentRunCard = memo(function AgentRunCard({ run }: { run: AgentRun }) {
  const { t } = useTranslation()
  const [open, setOpen] = useState(false)
  const isErr = run.status === 'error'
  const done = run.status === 'done' || run.status === 'error'
  const statusLabel =
    run.status === 'running' ? t('home.agentRunning') : isErr ? t('home.agentFailed') : t('home.agentDone')
  const statusColor = run.status === 'running' ? 'text-[var(--accent)]' : isErr ? 'text-[var(--danger)]' : 'text-[var(--success)]'

  return (
    <div className="animate-fade-in-up">
      <button
        onClick={() => done && setOpen((v) => !v)}
        className="w-full flex items-center gap-2 py-1.5 text-left transition-colors hover:opacity-80"
      >
        <Icon
          name="spark"
          size={11}
          className={isErr ? 'text-[var(--danger)]' : run.status === 'done' ? 'text-[var(--success)]' : 'text-[var(--warning)]'}
        />
        <div className="flex-1 min-w-0">
          <div className="text-[12px] truncate text-[var(--text-secondary)]">{run.name}</div>
        </div>
        <span className={`text-[11px] shrink-0 tabular-nums ${statusColor}`}>
          {run.status === 'running' && (
            <span className="inline-block w-2.5 h-2.5 rounded-full border border-[var(--accent)] border-t-transparent animate-spin align-middle mr-1" />
          )}
          {statusLabel}
        </span>
        {done && <Icon name="chevron-right" size={11} className={`text-[var(--text-muted)] transition-transform ${open ? 'rotate-90' : ''}`} />}
      </button>
      {done && open && (
        <pre className="text-[11px] font-mono whitespace-pre-wrap break-all leading-relaxed text-[var(--text-muted)] max-h-64 overflow-y-auto pb-2 animate-fade-in-up">
          {run.output}
        </pre>
      )}
    </div>
  )
})
