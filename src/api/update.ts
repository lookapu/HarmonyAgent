import { invokeWithError } from './invoke'

/** 基座更新代理支持：检测到系统代理时临时注入环境变量（updater 读环境变量），返回待恢复快照 */
export const beginUpdateProxy = () => invokeWithError<[string | null, string | null][]>('begin_update_proxy')

/** 恢复注入前的环境变量 */
export const endUpdateProxy = (saved: [string | null, string | null][]) =>
  invokeWithError<void>('end_update_proxy', { saved })

/** 读取当前系统代理地址（无代理时返回 null） */
export const getSystemProxy = () => invokeWithError<string | null>('get_system_proxy')
