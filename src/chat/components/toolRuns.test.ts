import { describe, expect, it } from 'vitest'
import { mutationGuardKind } from './toolRuns'

const failedRun = (tool: string, output: string) => ({
  tool,
  output,
  status: 'error' as const,
})

describe('mutationGuardKind', () => {
  it('classifies syntax gate rejections', () => {
    expect(mutationGuardKind(failedRun('edit_file', '代码修改事务被语法门禁拒绝：错误节点由 0 增至 1'))).toBe('syntax')
    expect(mutationGuardKind(failedRun('edit_file', '代码修改事务被 Java 声明门禁拒绝：游离注解由 0 增至 1'))).toBe('syntax')
    expect(mutationGuardKind(failedRun('lsp_rename', '代码修改事务被语法门禁拒绝：错误节点由 0 增至 1'))).toBe('syntax')
  })

  it('classifies atomic rollback and stale structure failures', () => {
    expect(mutationGuardKind(failedRun('multi_edit', 'multi_edit 原子提交失败，已回滚此前 2 个文件'))).toBe('rollback')
    expect(mutationGuardKind(failedRun('write_file', '结构定位已过期：文件在定位后再次发生变化'))).toBe('stale')
  })

  it('does not relabel unrelated tool failures', () => {
    expect(mutationGuardKind(failedRun('run_command', 'syntax gate failed'))).toBeNull()
    expect(mutationGuardKind({ tool: 'edit_file', output: 'ok', status: 'done' })).toBeNull()
  })
})
