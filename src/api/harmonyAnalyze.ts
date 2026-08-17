import { invoke } from '@tauri-apps/api/core'

/** 单个构建错误（结构化） */
export interface AnalyzedBuildError {
  kind: string
  /** 根因分类：type / dependency / signing / sdk / api_level / resource / ohpm / syntax / other */
  category: string
  file?: string | null
  line?: number | null
  column?: number | null
  message: string
  suggestion: string
}

/** 权限信息 */
export interface PermissionInfo {
  name: string
  reason?: string | null
}

/** ohpm 依赖项 */
export interface OhpmDep {
  name: string
  version: string
  dev: boolean
  module: string
}

/** 模块级能力摘要 */
export interface ModuleCapability {
  rel_path: string
  kind: string
  device_types: string[]
  main_element?: string | null
  kits: string[]
  permissions: PermissionInfo[]
  deps: OhpmDep[]
}

/** Kit 使用统计 */
export interface KitStat {
  kit: string
  count: number
}

/** 工程级精简信息（与 harmony.rs HarmonyProject 对应） */
export interface ProjectBasic {
  bundle_name?: string | null
  version_code?: number | null
  version_name?: string | null
  app_label?: string | null
  main_element?: string | null
  entry_module?: string | null
  api_version?: number | null
  /** compatibleSdkVersion 原文（如 "6.1.1(24)"） */
  sdk_version?: string | null
  signing_configured: boolean
}

/** 工程能力分析结果 */
export interface ProjectCapability {
  project: ProjectBasic
  modules: ModuleCapability[]
  kit_usage: KitStat[]
  permissions: PermissionInfo[]
  deps: OhpmDep[]
  build_errors: AnalyzedBuildError[]
}

/** 解析最近一次构建日志的结构化错误列表 */
export const analyzeBuildErrors = (projectPath: string) =>
  invoke<AnalyzedBuildError[]>('analyze_build_errors', { projectPath })

/** 盘点工程能力（模块 / Kit 使用 / 权限 / 依赖 / 最近构建错误） */
export const analyzeHarmonyProject = (projectPath: string) =>
  invoke<ProjectCapability>('analyze_harmony_project', { projectPath })

/** 通用工程概览：识别非鸿蒙工程类型（Node/Go/Rust/Python/Java/C/C++/Flutter/.NET 等） */
export const analyzeGenericProject = (projectPath: string) =>
  invoke<string>('analyze_generic_project', { projectPath })

/** ohpm 依赖版本核对：声明约束 vs 实际安装版本 */
export interface OhpmDepCheck {
  name: string
  declared: string
  installed: string
  dev: boolean
  module: string
}
export const checkOhpmDeps = (projectPath: string) =>
  invoke<OhpmDepCheck[]>('check_ohpm_deps', { projectPath })

/** 在工程目录执行 ohpm install，返回过程日志 */
export const runOhpmInstall = (projectPath: string) =>
  invoke<string>('run_ohpm_install', { projectPath })
