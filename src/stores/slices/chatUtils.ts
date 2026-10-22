import type { ChatMessage, ExecutionStep } from '../../api/project'
import type { TaskPlan, ToolRun } from '../projectStoreTypes'
import { unified } from 'unified'
import remarkParse from 'remark-parse'
import remarkGfm from 'remark-gfm'
import remarkRehype from 'remark-rehype'
import rehypeSanitize, { defaultSchema } from 'rehype-sanitize'
import rehypeStringify from 'rehype-stringify'

/** 恢复与二次对账共用持久工具投影，绝不从 UI id 反推后端调用身份。 */
export function toolRunFromExecutionStep(step: ExecutionStep): ToolRun {
  return {
    id: `tool-call-${step.external_id || step.step_id}`,
    callId: step.external_id || undefined,
    tool: step.tool_name || step.title,
    args: '',
    status: ['prepared', 'running'].includes(step.state) ? 'running' : step.state === 'completed' ? 'done' : 'error',
    output: step.result_summary || '',
    startedAt: step.started_at ?? step.updated_at,
    durationMs: step.started_at != null && step.finished_at != null
      ? Math.max(0, step.finished_at - step.started_at) : undefined,
  }
}

/** invoke 失败后刷新到的最新一页里，是否已包含刚发送的用户消息。
 * 乐观消息没有数据库 ID，只能按「角色 + 内容 + 发送时间」判断；
 * 排队消息不算——它还没执行，本轮请求并未作为已发送消息入库。
 * 带图片的请求入库时后端会追加「（本次请求附带 N 张图片…）」，只在确实发过图片时
 * 才接受这个后缀，避免普通消息被宽松匹配。 */
export function containsPersistedUserMessage(
  messages: ChatMessage[],
  content: string,
  sentAtSec: number,
  imagesSent = false,
): boolean {
  const target = content.trim()
  return messages.some((m) => {
    if (m.role !== 'user' || m.queued === 1 || (m.created_at ?? 0) < sentAtSec - 2) return false
    const stored = m.content.trim()
    if (stored === target) return true
    return imagesSent && stored.startsWith(`${target}\n\n（本次请求附带 `)
  })
}

/** 运行代次匹配：无事件 ID 兼容旧后端；有 ID 时必须与当前任务一致。 */
export function acceptsRunEvent(activeRunId: string | null | undefined, eventRunId?: string): boolean {
  // 纯函数的判定口径保持不变：桶未握手（activeRunId 为空）时拒绝带代次的事件。
  // 真正需要放行「握手前首包」的是 classifyRunEvent —— 它额外知道该代次是否
  // 已经终态（墓碑），能在放行首包的同时拦住迟到的旧代次事件。
  return !eventRunId || activeRunId === eventRunId
}

/**
 * 流式事件与桶代次的匹配结论。
 * - `accept`：事件属于当前代次，或事件不带代次（旧后端/LAN 兼容）；
 * - `bind`：桶尚未握手（发送时创建的乐观桶，`run-started` 还没到），
 *   事件可作为首包接受，并由调用方就地把代次补绑到桶上；
 * - `reject`：该代次已经终态，或桶已绑定其它代次。
 */
export type RunAcceptance = 'accept' | 'bind' | 'reject'

export function classifyRunEvent(
  activeRunId: string | null | undefined,
  eventRunId: string | undefined,
  terminalRunId: string | undefined,
): RunAcceptance {
  // 终态代次的迟到事件优先拦截：此时桶可能已被下一次发送重置成 runId=null，
  // 单靠代次比较会把旧任务的尾包错误地当成本次首包。
  if (eventRunId && terminalRunId === eventRunId) return 'reject'
  if (acceptsRunEvent(activeRunId, eventRunId)) return 'accept'
  if (activeRunId || !eventRunId) return 'reject'
  return 'bind'
}

/** 可收口的工具运行记录（ToolRun / TerminalEntry 的最小公共形状）。 */
interface SettleableRun {
  status: 'running' | 'done' | 'error'
  output?: string
}

/**
 * 任务终止（停止 / 出错 / 看门狗判死）时把仍标记为 `running` 的工具卡收口。
 *
 * 停止按钮与「停止工具」入口都随 `isStreaming=false` 一起消失，若卡片继续显示
 * 「运行中」，用户既拿不到结果、也没有任何入口去结束它，只能等下一次任务开始时
 * 被整体清空——期间一直表现为「卡住」。已完成/已失败的记录原样保留，便于回看
 * 这一步到底做了什么。
 */
export function settleRunningRuns<T extends SettleableRun>(runs: T[], note: string): T[] {
  let changed = false
  const next = runs.map((run) => {
    if (run.status !== 'running') return run
    changed = true
    return {
      ...run,
      status: 'error' as T['status'],
      output: run.output?.trim() ? run.output : note,
    }
  })
  return changed ? next : runs
}

/** 把一次运行对应的用户消息收敛到已入库/已执行状态。
 * 普通发送用后端 ID 替换 local 占位；排队自动续跑已有真实 ID，只需清 queued。 */
export function reconcileRunUserMessage(
  messages: ChatMessage[],
  userMessageId: string,
  preferredLocalId?: string,
): ChatMessage[] {
  const persistedIdx = messages.findIndex((m) => m.id === userMessageId)
  if (persistedIdx >= 0) {
    return messages.map((m, i) => (i === persistedIdx ? { ...m, queued: 0 } : m))
  }
  const preferredIdx = preferredLocalId ? messages.findIndex((m) => m.id === preferredLocalId) : -1
  const localIdx =
    preferredIdx >= 0
      ? preferredIdx
      : [...messages]
          .map((m, i) => ({ m, i }))
          .reverse()
          .find(({ m }) => m.role === 'user' && typeof m.id === 'string' && m.id.startsWith('local-'))?.i
  if (localIdx === undefined) return messages
  return messages.map((m, i) => (i === localIdx ? { ...m, id: userMessageId } : m))
}

/** 最终消息按数据库 ID 幂等收敛。WebView 重载后历史里可能已经含占位消息，
 * chat-done 必须替换该项而非再次追加，否则界面会出现两条相同回复。 */
export function upsertMessageById(messages: ChatMessage[], message: ChatMessage): ChatMessage[] {
  const index = messages.findIndex((item) => item.id === message.id)
  if (index < 0) return [...messages, message]
  return messages.map((item, i) => (i === index ? message : item))
}

/** 排队消息被执行：消息可能已在时间线（单条排队的乐观行）也可能只在
 * 排队条（批量发送）。两种情况都要落成一条普通用户消息并清掉排队条目，
 * 否则任务在跑而提问气泡要等重新打开会话才出现。 */
export function applyConsumedQueuedMessage<T extends { id: string }>(
  messages: ChatMessage[],
  queuedList: T[],
  message: ChatMessage,
): { messages: ChatMessage[]; queuedList: T[] } {
  const shown = messages.some((m) => m.id === message.id)
  return {
    messages: shown
      ? messages.map((m) => (m.id === message.id ? { ...message, queued: 0 } : m))
      : [...messages, { ...message, queued: 0 }],
    queuedList: queuedList.filter((q) => q.id !== message.id),
  }
}

/** 一次完成事件只收敛一个运行项；同名工具并发时不能批量结束全部卡片。 */
export function firstRunningIndex<T extends { status: string }>(entries: T[], matches: (entry: T) => boolean): number {
  return entries.findIndex((entry) => entry.status === 'running' && matches(entry))
}

/** 从 Agent 正文解析"计划列表"（Markdown 有序列表，2~10 项，含工具标记/代码块的列表块不采用）。
 * 模型常以有序列表给出任务步骤；无匹配返回 null（不显示进度卡，工具卡已足够）。 */
export function parsePlanSteps(text: string): string[] | null {
  const steps: string[] = []
  let inList = false
  for (const raw of text.split('\n')) {
    const m = raw.match(/^\s*\d+[.)]\s+(.+)/)
    if (m) {
      const step = m[1].trim()
      if (!step || step.startsWith('<')) continue
      inList = true
      steps.push(step)
      if (steps.length > 10) return null
    } else if (inList && raw.trim()) {
      break // 列表块结束（连续行才算计划，避免正文散落列表误判）
    }
  }
  return steps.length >= 2 ? steps : null
}

/** 最近一条 assistant 消息正文（计划解析兜底来源） */
export function lastAssistantText(messages: ChatMessage[]): string {
  for (let i = messages.length - 1; i >= 0; i--) {
    const m = messages[i]
    if (m.role === 'assistant') return m.content
  }
  return ''
}

/** 任务进度状态机推进（工具事件驱动）：
 * start=true 表示新工具开始（首个工具时解析计划；后续工具在无进行中步骤时推进下一个待办）；
 * ok 表示工具执行结果（成功 done / 失败 error，作用于第一个进行中步骤）。 */
export function advancePlan(
  plan: TaskPlan | null,
  streamingText: string,
  messages: ChatMessage[],
  evt: { start?: boolean; ok?: boolean },
): TaskPlan | null {
  if (evt.start) {
    if (!plan || plan.phase !== 'running') {
      // 首个工具调用：从流式正文（或最近一条 assistant 消息）解析计划列表
      const steps = parsePlanSteps(streamingText) || parsePlanSteps(lastAssistantText(messages))
      if (!steps) return plan // 模型未输出计划列表：不显示进度卡（工具卡已足够）
      return { steps: steps.map((text, i) => ({ text, status: i === 0 ? 'running' : 'pending' })), phase: 'running' }
    }
    if (plan.steps.some((s) => s.status === 'running')) return plan
    const steps = plan.steps.map((s) => ({ ...s }))
    const i = steps.findIndex((s) => s.status === 'pending')
    if (i >= 0) steps[i].status = 'running'
    return { ...plan, steps }
  }
  // 工具完成：推进第一步（进行中优先；并行工具连发时回退到待办步骤，保证每个成功工具推进一步）
  if (!plan || plan.phase !== 'running') return plan
  const steps = plan.steps.map((s) => ({ ...s }))
  const i = steps.findIndex((s) => s.status === 'running')
  const target = i >= 0 ? i : steps.findIndex((s) => s.status === 'pending')
  if (target >= 0) steps[target].status = evt.ok ? 'done' : 'error'
  return { ...plan, steps }
}

/* ============ 导出辅助（Markdown 简单转 HTML） ============ */

export function escapeHtml(s: string): string {
  return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;')
}

/** rehype-sanitize 白名单：与 Markdown.tsx 保持一致（kbd / sup / blockquote 类名放行） */
const exportSanitizeSchema = {
  ...defaultSchema,
  tagNames: [...(defaultSchema.tagNames ?? []), 'kbd'],
  attributes: {
    ...(defaultSchema.attributes ?? {}),
    sup: [...(defaultSchema.attributes?.sup ?? []), 'className'],
    kbd: ['className'],
    blockquote: [...(defaultSchema.attributes?.blockquote ?? []), 'className'],
  },
}

/** 导出用的统一 Markdown 解析管线：与 UI Markdown 渲染走同一套 remark/rehype 插件，
 *  保证导出 HTML 与界面渲染一致（代码块、表格、任务列表、删除线、链接等全支持）。
 *  同步实现（全部 plugin 同步），可直接用于非异步上下文。 */
const exportProcessor = unified()
  .use(remarkParse)
  .use(remarkGfm)
  .use(remarkRehype, { allowDangerousHtml: false })
  .use(rehypeSanitize, exportSanitizeSchema)
  .use(rehypeStringify)

/** Markdown → HTML（导出用）。
 *  之前是手写 replace 串，存在代码块嵌套错乱、列表无 ul 包裹等问题；
 *  改用 unified pipeline 后结构正确，浏览器渲染自然恢复正常。 */
export function toHtml(md: string): string {
  if (!md) return ''
  // 极大文档（>50k）退回原始手写：unified 同步处理大文档会阻塞主线程；导出仅本地存档用
  if (md.length > 50_000) {
    return legacyToHtml(md)
  }
  try {
    const file = exportProcessor.processSync(md)
    return String(file)
  } catch {
    // unified 解析异常时回退到手写版本（避免导出整体失败）
    return legacyToHtml(md)
  }
}

/** 旧手写实现：保留作为超大文档和解析异常的兜底，行为与之前保持一致。 */
function legacyToHtml(md: string): string {
  let html = escapeHtml(md)
  // 代码块
  html = html.replace(/```([\s\S]*?)```/g, (_, code: string) => `<pre>${code.replace(/\n$/, '')}</pre>`)
  // 行内代码
  html = html.replace(/`([^`\n]+)`/g, '<code>$1</code>')
  // 加粗 / 斜体 / 删除线
  html = html.replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>')
  html = html.replace(/\*([^*\n]+)\*/g, '<em>$1</em>')
  html = html.replace(/~~([^~]+)~~/g, '<del>$1</del>')
  // 链接
  html = html.replace(/\[([^\]]+)\]\(([^)\s]+)\)/g, '<a href="$2">$1</a>')
  // 引用
  html = html.replace(/^&gt; (.+)$/gm, '<blockquote>$1</blockquote>')
  // 标题
  html = html.replace(/^##### (.+)$/gm, '<h5>$1</h5>')
  html = html.replace(/^#### (.+)$/gm, '<h4>$1</h4>')
  html = html.replace(/^### (.+)$/gm, '<h3>$1</h3>')
  html = html.replace(/^## (.+)$/gm, '<h2>$1</h2>')
  html = html.replace(/^# (.+)$/gm, '<h1>$1</h1>')
  // 列表项
  html = html.replace(/^[-*] (.+)$/gm, '<li>$1</li>')
  html = html.replace(/^\d+\. (.+)$/gm, '<li>$1</li>')
  // 段落
  html = html.replace(/^(?!<[hplb]|<\/)(.+)$/gm, '<p>$1</p>')
  // 空行
  html = html.replace(/\n{2,}/g, '\n')
  return html
}
