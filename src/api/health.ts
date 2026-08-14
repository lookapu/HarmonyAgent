import { invoke } from '@tauri-apps/api/core'

export interface HealthResult {
  provider_id: string
  provider_name: string
  status: string
  latency_ms: number | null
  error: string | null
}

/** 鸿蒙工具链单项检查结果 */
export interface ToolchainCheck {
  name: string
  found: boolean
  detail: string
  suggestion: string | null
}

export const checkAllHealth = () => invoke<HealthResult[]>('check_all_health')
/** 鸿蒙工具链检查（hvigorw / hdc / ohpm / 工程结构）；projectId 可选，customPaths 为自定义工具链目录 */
export const checkHarmonyToolchain = (projectId?: string, customPaths?: string[]) =>
  invoke<ToolchainCheck[]>('check_harmony_toolchain', {
    projectId: projectId ?? '',
    customPaths: customPaths ?? [],
  })
