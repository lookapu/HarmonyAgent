import { describe, it, expect } from 'vitest'
import { fmtElapsed, sanitizeToolMarkers } from './chatUtils'

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

  it('清理漏写结束符的残缺标记（从【TOOL| 删到行尾）', () => {
    expect(sanitizeToolMarkers('开头【TOOL|edit_file|{"path":"x"}正文泄漏')).toBe('开头')
  })

  it('普通文本原样保留（JSON 参数内 ] 不误截断）', () => {
    const text = '示例：arr[1] = 2，无工具标记'
    expect(sanitizeToolMarkers(text)).toBe(text)
  })
})
