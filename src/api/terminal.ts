import { invoke } from '@tauri-apps/api/core'

/** 在当前项目根目录打开系统终端（cmd 窗口），供用户手动执行命令 */
export const openTerminal = (projectPath: string) => invoke<void>('open_terminal', { projectPath })
