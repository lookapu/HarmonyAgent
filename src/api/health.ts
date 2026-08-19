import { invokeWithError } from './invoke'

export interface HealthResult {
  provider_id: string
  provider_name: string
  status: string
  latency_ms: number | null
  error: string | null
}

/** 工程结构检查详情（仅 name="project_structure" 的检查项有值；前端据此做 i18n 渲染） */
export interface ProjectStructure {
  /** single=标准单工程 / workspace=多项目工作区 / invalid=非完整工程 */
  kind: 'single' | 'workspace' | 'invalid'
  /** workspace 时的工程名列表（最多 8 个） */
  projects: string[]
  /** workspace 工程总数（可能大于 projects.length，超出部分未列出） */
  total: number
  /** invalid 时缺失的关键文件（如 build-profile.json5） */
  missing: string[]
  /** invalid 时目标目录是否存在 */
  dir_exists: boolean
}

/** 鸿蒙工具链单项检查结果 */
export interface ToolchainCheck {
  name: string
  found: boolean
  detail: string
  suggestion: string | null
  /** 工程结构检查详情（仅 name="project_structure" 时有值） */
  structure?: ProjectStructure | null
}

export const checkAllHealth = () => invokeWithError<HealthResult[]>('check_all_health')
/** 轻量工具链体检（[66]：只查 hvigorw/hdc/ohpm，不查工程结构；启动自动 ping 用） */
export const toolsHealth = () => invokeWithError<ToolchainCheck[]>('tools_health')
/** 鸿蒙工具链检查（hvigorw / hdc / ohpm / 工程结构）；projectId 可选，customPaths 为自定义工具链目录 */
export const checkHarmonyToolchain = (projectId?: string, customPaths?: string[]) =>
  invokeWithError<ToolchainCheck[]>('check_harmony_toolchain', {
    projectId: projectId ?? '',
    customPaths: customPaths ?? [],
  })
