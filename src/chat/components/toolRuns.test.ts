import { describe, expect, it } from 'vitest'
import { mutationGuardKind, mutationRelocationApplied } from './toolRuns'

const failedRun = (tool: string, output: string) => ({
  tool,
  output,
  status: 'error' as const,
})

describe('mutationGuardKind', () => {
  it('never labels incomplete restoration as successful rollback', () => {
    expect(mutationGuardKind(failedRun('multi_edit', 'multi_edit 回滚未完成，部分文件已回滚'))).toBe('rollbackIncomplete')
    expect(mutationGuardKind(failedRun('edit_file', '写入文件失败且恢复原内容失败'))).toBe('rollbackIncomplete')
  })
  it('distinguishes unsafe node boundaries from stale handles', () => {
    expect(mutationGuardKind(failedRun('edit_file', '结构编辑句柄边界不安全：包含相邻节点'))).toBe('boundary')
  })
  it('classifies syntax gate rejections', () => {
    expect(mutationGuardKind(failedRun('edit_file', '代码修改事务被语法门禁拒绝：错误节点由 0 增至 1'))).toBe('syntax')
    expect(mutationGuardKind(failedRun('edit_file', '代码修改事务被 Java 声明门禁拒绝：游离注解由 0 增至 1'))).toBe('syntax')
    expect(mutationGuardKind(failedRun('lsp_rename', '代码修改事务被语法门禁拒绝：错误节点由 0 增至 1'))).toBe('syntax')
  })

  it('separates compiler type gate rejections from syntax gate rejections', () => {
    expect(mutationGuardKind(failedRun('edit_file', '代码修改事务被 Java 编译器门禁拒绝：A.java 的 javac 诊断由 0 条增至 1 条'))).toBe('typeCheck')
    expect(mutationGuardKind(failedRun('write_file', 'mutation rejected by java compiler gate'))).toBe('typeCheck')
  })

  it('classifies atomic rollback and stale structure failures', () => {
    expect(mutationGuardKind(failedRun('multi_edit', 'multi_edit 原子提交失败，已回滚此前 2 个文件'))).toBe('rollback')
    expect(mutationGuardKind(failedRun('write_file', '结构定位已过期：文件在定位后再次发生变化'))).toBe('stale')
    expect(mutationGuardKind(failedRun('edit_file', '结构重定位被拒绝：目标节点内容已经变化'))).toBe('stale')
  })

  it('does not relabel unrelated tool failures', () => {
    expect(mutationGuardKind(failedRun('run_command', 'syntax gate failed'))).toBeNull()
    expect(mutationGuardKind({ tool: 'edit_file', output: 'ok', status: 'done' })).toBeNull()
  })

  it('surfaces successful controlled relocation only for mutation tools', () => {
    expect(mutationRelocationApplied({ tool: 'edit_file', output: '受控重定位：目标节点唯一匹配', status: 'done' })).toBe(true)
    expect(mutationRelocationApplied({ tool: 'run_command', output: 'controlled relocation', status: 'done' })).toBe(false)
    expect(mutationRelocationApplied(failedRun('edit_file', '受控重定位失败'))).toBe(false)
  })
})
