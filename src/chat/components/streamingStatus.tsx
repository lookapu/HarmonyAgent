import { memo, useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useShallow } from 'zustand/react/shallow'
import { useProjectStore, type AgentRun, type ToolRun } from '../../stores/projectStore'
import Icon from '../../icons/Icon'
import { fmtElapsed } from '../chatUtils'
import { StreamingMessage } from './messageBlocks'
import { TaskOpsBadge } from './plan'

function useElapsedSeconds(startedAt: number | null): number {
  const [elapsed, setElapsed] = useState(() => (startedAt ? Math.max(0, Math.floor((Date.now() - startedAt) / 1000)) : 0))

  useEffect(() => {
    if (!startedAt) {
      setElapsed(0)
      return
    }
    const update = () => setElapsed(Math.max(0, Math.floor((Date.now() - startedAt) / 1000)))
    update()
    const timer = setInterval(update, 1000)
    return () => clearInterval(timer)
  }, [startedAt])

  return elapsed
}

/** 高频正文/思考增量只让本组件刷新，不再触发 Home 与侧栏整树重渲染。 */
export const StreamingOutput = memo(function StreamingOutput({
  conversationId,
  speed,
}: {
  conversationId: string | null
  speed: number
}) {
  const { present, running, content, reasoning } = useProjectStore(
    useShallow((state) => {
      const bucket = conversationId ? state.streamings[conversationId] : undefined
      return {
        present: Boolean(bucket),
        running: Boolean(bucket?.conversationId),
        content: bucket?.content ?? '',
        reasoning: bucket?.reasoning ?? '',
      }
    }),
  )

  if (!present || (!running && !content && !reasoning)) return null
  return <StreamingMessage content={content} reasoning={reasoning} speed={speed} active={running} />
})

/** 会话列表运行状态独立计时，避免每秒刷新整个首页。 */
export const ConversationRunStatus = memo(function ConversationRunStatus({
  conversationId,
  foreground,
}: {
  conversationId: string
  foreground: boolean
}) {
  const { t } = useTranslation()
  const startedAt = useProjectStore((state) => state.streamings[conversationId]?.startedAt ?? null)
  const elapsed = useElapsedSeconds(foreground ? startedAt : null)

  return (
    <span className="flex items-center gap-1.5 text-[11px] text-[var(--accent)] mt-0.5 tabular-nums">
      <span className="w-1.5 h-1.5 rounded-full bg-[var(--accent)] animate-pulse shrink-0" />
      {foreground ? t('home.taskElapsed', { time: fmtElapsed(elapsed) }) : t('home.bgStreaming')}
    </span>
  )
})

/** 运行中操作徽章自行维护秒表；工具事件变化才由 Home 更新其余属性。 */
export const RunningTaskOpsBadge = memo(function RunningTaskOpsBadge({
  conversationId,
  count,
  toolName,
  open,
  onToggle,
  runs,
  agents,
}: {
  conversationId: string
  count: number
  toolName?: string
  open: boolean
  onToggle: () => void
  runs: ToolRun[]
  agents: AgentRun[]
}) {
  const startedAt = useProjectStore((state) => state.streamings[conversationId]?.startedAt ?? null)
  const elapsed = useElapsedSeconds(startedAt)
  if (!startedAt) return null

  return (
    <TaskOpsBadge
      running
      count={count}
      time={fmtElapsed(elapsed)}
      toolName={toolName}
      open={open}
      onToggle={onToggle}
      runs={runs}
      agents={agents}
    />
  )
})

/** 静默检测只订阅当前流式桶的时间戳，不让 token/秒表变化波及主页面。 */
export const SilentStreamHint = memo(function SilentStreamHint({
  conversationId,
  active,
}: {
  conversationId: string | null
  active: boolean
}) {
  const { t } = useTranslation()
  const { startedAt, lastDeltaAt } = useProjectStore(
    useShallow((state) => {
      const bucket = conversationId ? state.streamings[conversationId] : undefined
      return {
        startedAt: bucket?.startedAt ?? null,
        lastDeltaAt: bucket?.lastDeltaAt ?? null,
      }
    }),
  )
  const reference = active ? (lastDeltaAt ?? startedAt) : null
  const [silentSeconds, setSilentSeconds] = useState(0)

  useEffect(() => {
    if (!reference) {
      setSilentSeconds(0)
      return
    }
    let interval: ReturnType<typeof setInterval> | null = null
    const update = () => setSilentSeconds(Math.max(0, Math.floor((Date.now() - reference) / 1000)))
    update()
    const timeout = setTimeout(() => {
      update()
      interval = setInterval(update, 1000)
    }, Math.max(0, 15_000 - (Date.now() - reference)))
    return () => {
      clearTimeout(timeout)
      if (interval) clearInterval(interval)
    }
  }, [reference])

  if (!active || silentSeconds < 15) return null
  return (
    <div className="flex items-center gap-2 text-[11.5px] text-[var(--text-muted)] animate-pulse">
      <Icon name="spark" size={12} />
      {t('home.silentHint', { time: fmtElapsed(silentSeconds) })}
    </div>
  )
})
