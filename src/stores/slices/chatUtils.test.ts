import { describe, it, expect } from 'vitest'
import {
  parsePlanSteps,
  lastAssistantText,
  advancePlan,
  escapeHtml,
  toHtml,
} from './chatUtils'
import type { ChatMessage } from '../../api/project'

const msg = (role: string, content: string): ChatMessage => ({ role, content } as ChatMessage)

describe('parsePlanSteps', () => {
  it('解析连续有序列表为步骤', () => {
    const steps = parsePlanSteps('1. 第一步\n2. 第二步\n3. 第三步')
    expect(steps).toEqual(['第一步', '第二步', '第三步'])
  })

  it('少于 2 项不采用', () => {
    expect(parsePlanSteps('1. 只有一步')).toBeNull()
  })

  it('列表中断后不再收集后续列表项', () => {
    // break 语义：遇到非空非列表行即结束列表块，已收集步骤保留，后面的列表项忽略
    expect(parsePlanSteps('1. 第一步\n2. 第二步\n散落正文\n3. 第三步')).toEqual(['第一步', '第二步'])
  })

  it('超过 10 项不采用', () => {
    const many = Array.from({ length: 11 }, (_, i) => `${i + 1}. 步骤 ${i + 1}`).join('\n')
    expect(parsePlanSteps(many)).toBeNull()
  })

  it('忽略尖括号开头的伪列表项', () => {
    expect(parsePlanSteps('1. 正常步骤\n2. <tool_call>')).toBeNull() // 剔除后不足 2 项
    expect(parsePlanSteps('1. 正常步骤\n2. <tool_call>\n3. 正常三')).toEqual(['正常步骤', '正常三'])
  })
})

describe('lastAssistantText', () => {
  it('返回最近一条 assistant 消息正文', () => {
    const msgs = [msg('user', 'hi'), msg('assistant', '回答一'), msg('tool', 'out'), msg('assistant', '回答二')]
    expect(lastAssistantText(msgs)).toBe('回答二')
  })

  it('无 assistant 消息返回空串', () => {
    expect(lastAssistantText([msg('user', 'hi')])).toBe('')
  })
})

describe('advancePlan', () => {
  it('首个工具调用时从正文解析计划并标记第一步 running', () => {
    const plan = advancePlan(null, '步骤：\n1. 甲\n2. 乙', [], { start: true })
    expect(plan?.phase).toBe('running')
    expect(plan?.steps.map((s) => [s.text, s.status])).toEqual([
      ['甲', 'running'],
      ['乙', 'pending'],
    ])
  })

  it('无计划列表时保持 null', () => {
    expect(advancePlan(null, '没有列表', [], { start: true })).toBeNull()
  })

  it('工具成功推进 running 步骤为 done', () => {
    const plan = { steps: [{ text: '甲', status: 'running' as const }, { text: '乙', status: 'pending' as const }], phase: 'running' as const }
    const next = advancePlan(plan, '', [], { ok: true })
    expect(next?.steps.map((s) => s.status)).toEqual(['done', 'pending'])
  })

  it('工具失败标记 error 且后续工具仍可推进', () => {
    const plan = { steps: [{ text: '甲', status: 'running' as const }, { text: '乙', status: 'pending' as const }], phase: 'running' as const }
    const failed = advancePlan(plan, '', [], { ok: false })
    expect(failed?.steps.map((s) => s.status)).toEqual(['error', 'pending'])
    const next = advancePlan(failed, '', [], { start: true })
    expect(next?.steps.map((s) => s.status)).toEqual(['error', 'running'])
  })

  it('全部完成后不再推进', () => {
    const plan = { steps: [{ text: '甲', status: 'done' as const }], phase: 'running' as const }
    // 无待推进步骤：状态不变（实现返回等值新对象，故用 toStrictEqual 而非 toBe）
    expect(advancePlan(plan, '', [], { start: true })).toStrictEqual(plan)
    expect(advancePlan(plan, '', [], { ok: true })).toStrictEqual(plan)
  })
})

describe('escapeHtml / toHtml', () => {
  it('转义特殊字符', () => {
    expect(escapeHtml('<b>&"')).toBe('&lt;b&gt;&amp;&quot;')
  })

  it('代码块转为 pre', () => {
    expect(toHtml('```\ncode\n```')).toContain('<pre>')
  })

  it('行内代码与加粗', () => {
    const html = toHtml('`x` 和 **b**')
    expect(html).toContain('<code>x</code>')
    expect(html).toContain('<strong>b</strong>')
  })

  it('标题与列表', () => {
    const html = toHtml('# 标题\n- 项')
    expect(html).toContain('<h1>标题</h1>')
    expect(html).toContain('<li>项</li>')
  })
})
