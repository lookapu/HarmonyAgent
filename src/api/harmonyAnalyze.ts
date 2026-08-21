import { invokeWithError } from './invoke'

/** 单个构建错误（结构化） */
export interface AnalyzedBuildError {
  kind: string
  /** 根因分类：type / dependency / signing / sdk / api_level / resource / ohpm / syntax / other */
  category: string
  error_code?: string | null
  /** environment / dependency / configuration / compile / package / signing / build */
  stage: string
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

export interface HarmonyAbility {
  name: string
  src_entry?: string | null
  exported?: boolean | null
}

export interface HarmonyExtensionAbility extends HarmonyAbility {
  extension_type?: string | null
}

export interface HarmonyTarget {
  name: string
  products: string[]
}

export interface HarmonyModuleModel {
  name: string
  rel_path: string
  src_path: string
  kind: string
  api_type?: string | null
  build_modes: string[]
  artifact_kind: 'hap' | 'hsp' | 'har' | 'unknown'
  package_name?: string | null
  device_types: string[]
  main_element?: string | null
  targets: HarmonyTarget[]
  abilities: HarmonyAbility[]
  extension_abilities: HarmonyExtensionAbility[]
  permissions: Array<{
    name: string
    reason?: string | null
    abilities: string[]
    when?: string | null
  }>
}

export interface HarmonyProductModel {
  name: string
  compile_sdk_version?: string | null
  compatible_sdk_version?: string | null
  target_sdk_version?: string | null
  compile_api_level?: number | null
  compatible_api_level?: number | null
  target_api_level?: number | null
  runtime_os?: string | null
  signing_config?: string | null
  modules: string[]
}

export interface HarmonyDependencyModel {
  from_module: string
  name: string
  requirement: string
  scope: string
  target_module?: string | null
  locked_version?: string | null
  lockfile?: string | null
}

export interface HarmonyLockfileModel {
  path: string
  owner_module: string
  lockfile_version?: number | null
  specifiers: Array<{ declared: string; locked: string }>
  packages: Array<{
    key: string
    name?: string | null
    version?: string | null
    resolved?: string | null
    integrity?: string | null
    registry_type?: string | null
    dependencies: Record<string, string>
  }>
}

export interface HarmonyManifestSource {
  kind: string
  path: string
  owner_module: string
  status: 'parsed' | 'invalid'
  error?: string | null
}

export interface HarmonyProjectGraph {
  pages: Array<{
    module: string
    path: string
    source_kind: 'main_pages' | 'router_map' | 'decorator'
    source_file: string
    route_name?: string | null
  }>
  system_capabilities: Array<{
    module: string
    capability: string
    source_file: string
    line: number
  }>
  cross_module_refs: Array<{
    from_module: string
    to_module: string
    specifier: string
    source_file: string
    line: number
  }>
  edges: Array<{
    from: string
    to: string
    kind: string
    source: string
  }>
}

export interface HarmonySemanticModel {
  schema_version: number
  app: {
    bundle_name?: string | null
    version_code?: number | null
    version_name?: string | null
    label?: string | null
  }
  signing_configs: Array<{
    name: string
    material_configured: boolean
    certificate_configured: boolean
    profile_configured: boolean
    keystore_configured: boolean
    key_alias_configured: boolean
    sign_alg?: string | null
  }>
  build_modes: string[]
  products: HarmonyProductModel[]
  product_differences: Array<{
    baseline: string
    product: string
    fields: string[]
  }>
  modules: HarmonyModuleModel[]
  dependencies: HarmonyDependencyModel[]
  lockfiles: HarmonyLockfileModel[]
  manifests: HarmonyManifestSource[]
  graph: HarmonyProjectGraph
}

export interface HarmonyModelUpdate {
  mode: 'incremental' | 'full'
  changed_files: string[]
  affected_modules: string[]
  verification: {
    modules: string[]
    products: string[]
    checks: string[]
  }
  model: HarmonySemanticModel
}

export interface HarmonyImpactAnalysis {
  mode: 'incremental' | 'full'
  changed_files: string[]
  direct_modules: string[]
  affected_modules: string[]
  verification: {
    modules: string[]
    products: string[]
    checks: string[]
  }
  traces: Array<{
    module: string
    kind: 'direct' | 'dependency' | 'import' | 'project_structure'
    source: string
    depends_on?: string | null
  }>
}

/** 工程能力分析结果 */
export interface ProjectCapability {
  project: ProjectBasic
  semantic_model: HarmonySemanticModel
  modules: ModuleCapability[]
  kit_usage: KitStat[]
  permissions: PermissionInfo[]
  deps: OhpmDep[]
  build_errors: AnalyzedBuildError[]
}

/** 解析最近一次构建日志的结构化错误列表 */
export const analyzeBuildErrors = (projectPath: string) =>
  invokeWithError<AnalyzedBuildError[]>('analyze_build_errors', { projectPath })

/** 盘点工程能力（模块 / Kit 使用 / 权限 / 依赖 / 最近构建错误） */
export const analyzeHarmonyProject = (projectPath: string) =>
  invokeWithError<ProjectCapability>('analyze_harmony_project', { projectPath })

/** 预览文件变化对模块、产品与验证项的传播范围，不修改工程文件。 */
export const analyzeHarmonyImpact = (projectPath: string, changedFiles: string[]) =>
  invokeWithError<HarmonyImpactAnalysis>('analyze_harmony_impact', { projectPath, changedFiles })

/** 通用工程概览：识别非鸿蒙工程类型（Node/Go/Rust/Python/Java/C/C++/Flutter/.NET 等） */
export const analyzeGenericProject = (projectPath: string) =>
  invokeWithError<string>('analyze_generic_project', { projectPath })

/** ohpm 依赖版本核对：声明约束 vs 实际安装版本 */
export interface OhpmDepCheck {
  name: string
  declared: string
  installed: string
  dev: boolean
  module: string
}
export const checkOhpmDeps = (projectPath: string) =>
  invokeWithError<OhpmDepCheck[]>('check_ohpm_deps', { projectPath })

/** 在工程目录执行 ohpm install，返回过程日志 */
export const runOhpmInstall = (projectPath: string) =>
  invokeWithError<string>('run_ohpm_install', { projectPath })
