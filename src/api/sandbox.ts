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

export interface SandboxBoundaryCheck {
  name: string
  passed: boolean
  detail: string
}

export interface SandboxBoundaryReport {
  scope: 'filesystem_smoke_v1' | string
  backend: string
  available: boolean
  passed: boolean
  checks: SandboxBoundaryCheck[]
}

/** 只执行本地能力探测；不会下载镜像、安装运行时或执行用户命令。 */
export const probeSandboxBackends = () =>
  invokeWithError<SandboxCapabilities[]>('probe_sandbox_backends')

/** 在内部临时目录运行文件边界 smoke，不读取或修改用户项目。 */
export const verifyNativeSandboxBoundary = () =>
  invokeWithError<SandboxBoundaryReport>('verify_native_sandbox_boundary')
