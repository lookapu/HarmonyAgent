import { invokeWithError } from './invoke'

export interface SandboxCapabilities {
  backend: string
  available: boolean
  os_level_isolation: boolean
  filesystem_read_only: boolean
  workspace_write: boolean
  network_none: boolean
  network_allowlist: boolean
  resource_limits: boolean
  reason: string | null
}

/** 只执行本地能力探测；不会下载镜像、安装运行时或执行用户命令。 */
export const probeSandboxBackends = () =>
  invokeWithError<SandboxCapabilities[]>('probe_sandbox_backends')
