import { invoke } from '@tauri-apps/api/core'

/** Node 运行时状态（来源：system=系统已装 / upgraded=内置升级版 / bundled=出厂捆绑版 / none=不可用） */
export interface NodeRuntimeInfo {
  node_version: string
  npx_version: string
  source: string
  dir: string | null
  upgraded_dir: string | null
  bundled_dir: string | null
  /** node --version 执行失败原因（node_version 为空时展示） */
  node_error: string | null
  /** npx --version 执行失败原因（npx_version 为空时展示） */
  npx_error: string | null
}

/** 查询 Node 运行时状态（版本、来源、目录） */
export const getNodeRuntime = () => invoke<NodeRuntimeInfo>('get_node_runtime')

/** 升级 Node 运行时；version 缺省时自动取最新 LTS；useProxy: true=走系统代理 / false=直连 */
export const upgradeNodeRuntime = (version?: string, useProxy?: boolean) =>
  invoke<NodeRuntimeInfo>('upgrade_node_runtime', { version: version ?? null, useProxy: useProxy ?? null })

/** 删除升级版，回到出厂捆绑版本 */
export const resetNodeRuntime = () => invoke<NodeRuntimeInfo>('reset_node_runtime')
