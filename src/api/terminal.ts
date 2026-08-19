import { invokeWithError } from './invoke'

/** 在当前项目根目录打开系统终端（cmd 窗口），供用户手动执行命令 */
export const openTerminal = (projectPath: string) => invokeWithError<void>('open_terminal', { projectPath })

/** 内置终端执行结果 */
export interface TermResult {
  /** 命令输出（stdout+stderr 合并，GBK 已解码） */
  output: string
  /** 执行后当前目录 */
  cwd: string
  /** 是否仍在运行 */
  running: boolean
  /** 退出码（超时被终止为 null） */
  exit_code: number | null
  /** 是否超时被终止 */
  timed_out: boolean
}
/** 在内置终端执行一条命令（cd 命令更新会话目录；上一条未结束时需先停止） */
export const terminalExec = (projectId: string, projectPath: string, command: string) =>
  invokeWithError<TermResult>('terminal_exec', { projectId, projectPath, command })
/** 停止当前正在运行的终端命令 */
export const terminalKill = (projectId: string) =>
  invokeWithError<void>('terminal_kill', { projectId })
/** 终端会话状态 */
export const terminalStatus = (projectId: string, projectPath: string) =>
  invokeWithError<{ cwd: string; running: boolean }>('terminal_status', { projectId, projectPath })
