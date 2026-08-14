import type { ChatMessage } from '../../api/project'
import type { TaskPlan } from '../projectStoreTypes'

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

/** 轻量 Markdown → HTML（导出用；覆盖常用元素即可，不做完整解析） */
export function toHtml(md: string): string {
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
