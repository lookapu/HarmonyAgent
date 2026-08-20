import { describe, it, expect } from 'vitest'
import { fmtElapsed, interruptedTailMessage, sanitizeToolMarkers, shouldSubmitComposerKey } from './chatUtils'
import type { ChatMessage } from '../api/project'

const chatMsg = (role: ChatMessage['role'], id: string, extra: Partial<ChatMessage> = {}): ChatMessage =>
  ({ id, role, content: id, ...extra }) as ChatMessage

describe('fmtElapsed', () => {
  it('格式化秒数为 mm:ss', () => {
    expect(fmtElapsed(0)).toBe('0:00')
    expect(fmtElapsed(59)).toBe('0:59')
    expect(fmtElapsed(61)).toBe('1:01')
    expect(fmtElapsed(599)).toBe('9:59')
  })

  it('超过 1 小时格式化为 h:mm:ss', () => {
    expect(fmtElapsed(3600)).toBe('1:00:00')
    expect(fmtElapsed(3725)).toBe('1:02:05')
  })

  it('负值与小数容错（取整、钳制到 0）', () => {
    expect(fmtElapsed(-5)).toBe('0:00')
    expect(fmtElapsed(1.9)).toBe('0:01')
  })
})

describe('interruptedTailMessage', () => {
  it('识别已有历史回复之后新出现的孤立 user 消息', () => {
    const tail = chatMsg('user', 'u2', { queued: 0 })
    expect(interruptedTailMessage([chatMsg('user', 'u1'), chatMsg('assistant', 'a1'), tail], false)).toBe(tail)
  })

  it('流式中和排队消息不提示中断恢复', () => {
    const queued = chatMsg('user', 'q1', { queued: 1 })
    expect(interruptedTailMessage([queued], false)).toBeNull()
    expect(interruptedTailMessage([chatMsg('user', 'u1')], true)).toBeNull()
  })

  it('识别未完成的 assistant 占位消息', () => {
    const partial = chatMsg('assistant', 'a1', { duration_ms: null })
    expect(interruptedTailMessage([partial], false)).toBe(partial)
  })
})

describe('shouldSubmitComposerKey', () => {
  it('仅普通 Enter 发送，Shift+Enter 与输入法选词 Enter 均不发送', () => {
    expect(shouldSubmitComposerKey('Enter', false, false)).toBe(true)
    expect(shouldSubmitComposerKey('Enter', true, false)).toBe(false)
    expect(shouldSubmitComposerKey('Enter', false, true)).toBe(false)
    expect(shouldSubmitComposerKey('a', false, false)).toBe(false)
  })
})

describe('sanitizeToolMarkers', () => {
  it('清理完整工具标记（】结束）', () => {
    expect(sanitizeToolMarkers('前置【TOOL|read_file|{"path":"a"}|结果】后置')).toBe('前置后置')
  })

  it('清理 }] 结束的残缺标记', () => {
    expect(sanitizeToolMarkers('【TOOL|run_cmd|{"cmd":"ls"}|]}正文')).toBe('正文')
  })

  it('标记独立成行时删除标记内容（换行保留为分隔）', () => {
    expect(sanitizeToolMarkers('行一\n【TOOL|open_file|{"path":"b"}\n行二')).toBe('行一\n\n行二')
  })

  it('清理漏写结束符但 JSON 完整的标记（JSON 完整时保留其后同行正文）', () => {
    expect(sanitizeToolMarkers('开头【TOOL|edit_file|{"path":"x"}正文泄漏')).toBe('开头正文泄漏')
  })

  it('清理 JSON 后多余 |尾段】 的标记', () => {
    expect(sanitizeToolMarkers('【TOOL|read_file|{"path":"a"}|结果】后置')).toBe('后置')
  })

  it('普通文本原样保留（JSON 参数内 ] 不误截断）', () => {
    const text = '示例：arr[1] = 2，无工具标记'
    expect(sanitizeToolMarkers(text)).toBe(text)
  })

  it('跨行 JSON 参数完整清理（多行 edit_file 不泄漏后续行碎片）', () => {
    const text =
      '我来修改文件。\n【TOOL|edit_file|{"path":"a.txt","old":"x","new":"y"}\n】\n修改完成。'
    expect(sanitizeToolMarkers(text)).toBe('我来修改文件。\n\n修改完成。')
  })

  it('参数中 | 后换行再写 JSON（多行参数起始跨行）', () => {
    const text = '前置\n【TOOL|edit_file|\n{"path":"a.txt"}\n】\n后置'
    expect(sanitizeToolMarkers(text)).toBe('前置\n\n后置')
  })

  it('JSON 完整但漏写结束符，保留其后同行正文', () => {
    const text = '【TOOL|read_file|{"path":"a"}请看结果'
    expect(sanitizeToolMarkers(text)).toBe('请看结果')
  })

  it('多个连续工具标记均清理（含多行）', () => {
    const text =
      '开始\n【TOOL|read_file|{"p":"a"}\n】\n中间\n【TOOL|edit_file|{"p":"b","v":"line1\nline2"}\n】\n结束'
    expect(sanitizeToolMarkers(text)).toBe('开始\n\n中间\n\n结束')
  })

  it('流式截断的残缺标记（无结束符、JSON 未闭合）丢弃其后全部内容', () => {
    const text = '正文开始\n【TOOL|edit_file|{"path":"x","old":"abc'
    expect(sanitizeToolMarkers(text)).toBe('正文开始\n')
  })

  it('不含工具标记的普通花括号/方括号原样保留', () => {
    const text = '配置项 { key: value } 和数组 [1, 2, 3] 正常显示'
    expect(sanitizeToolMarkers(text)).toBe(text)
  })
})
