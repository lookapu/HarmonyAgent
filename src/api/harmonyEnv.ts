import { invoke } from '@tauri-apps/api/core'

/** SDK 单个组件（ets/native/js/toolchains/previewer） */
export interface SdkComponent {
  name: string
  api_version: string
  version: string | null
  path: string
  api_dir: string | null
}

/** SDK 变体（openharmony / hms） */
export interface SdkVariant {
  variant: string
  path: string
  components: SdkComponent[]
  api_version: string | null
  is_default: boolean
}

/** command-line-tools 信息 */
export interface CommandLineTools {
  root: string
  bin: string
  has_hdc: boolean
  has_ohpm: boolean
  has_hvigorw: boolean
}

/** 统一鸿蒙环境快照 */
export interface HarmonyEnv {
  sdk_root: string | null
  default_api: string | null
  sdk_variants: SdkVariant[]
  sdk_versions: string[]
  cli: CommandLineTools | null
  hdc_path: string | null
  ohpm_path: string | null
  hvigorw_path: string | null
  studio_dir: string | null
  source: 'auto' | 'manual' | string
  suggestions: string[]
}

/** SDK API 模块元数据 */
export interface ApiModule {
  module: string
  kit: string | null
  syscap: string | null
  since_min: number | null
  since_max: number | null
  declarations: string[]
  deprecated: boolean
  path: string
}

/** SDK API 索引 */
export interface ApiIndex {
  api_dir: string
  modules: ApiModule[]
  by_kit: Record<string, string[]>
}

/** 工程 SDK 版本对齐结果 */
export interface ProjectSdkAlignment {
  project_compatible: string | null
  project_api: number | null
  installed_api: string | null
  status: 'ok' | 'behind' | 'ahead' | 'unknown' | string
  message: string
}

/** 获取当前环境（持久化配置 + 自动探测，带缓存） */
export function getHarmonyEnv(): Promise<HarmonyEnv> {
  return invoke<HarmonyEnv>('get_harmony_env')
}

/** 仅自动探测（忽略手动配置），用于展示"自动发现结果" */
export function detectHarmonyEnv(): Promise<HarmonyEnv> {
  return invoke<HarmonyEnv>('detect_harmony_env')
}

/** 保存手动指定的 SDK / command-line-tools 路径，返回最新环境 */
export function saveHarmonyEnv(sdkRoot: string | null, cliRoot: string | null): Promise<HarmonyEnv> {
  return invoke<HarmonyEnv>('save_harmony_env', {
    sdkRoot: sdkRoot || null,
    cliRoot: cliRoot || null,
  })
}

/** 列出 SDK API 模块（可按 kit 过滤） */
export function listSdkApiModules(kit?: string): Promise<ApiIndex> {
  return invoke<ApiIndex>('list_sdk_api_modules', { kit: kit || null })
}

/** 按关键字检索 SDK API 模块 */
export function searchSdkApi(query: string, limit?: number): Promise<ApiModule[]> {
  return invoke<ApiModule[]>('search_sdk_api', { query, limit: limit ?? null })
}

/** 读取某个 API 模块的完整 .d.ts 声明 */
export function readSdkApiModule(module: string): Promise<string> {
  return invoke<string>('read_sdk_api_module', { module })
}

/** 检查工程 compatibleSdkVersion 与已装 SDK 是否对齐 */
export function checkProjectSdkAlignment(projectPath: string): Promise<ProjectSdkAlignment> {
  return invoke<ProjectSdkAlignment>('check_project_sdk_alignment', { projectPath })
}

// ---------- OpenHarmony 文档本地镜像（替代需登录的华为文档站） ----------

export interface HarmonyDocsStatus {
  downloaded: boolean
  doc_count: number
  root: string
}

export interface DocEntry {
  rel_path: string
  title: string
  kit: string
  preview: string
  has_example: boolean
}

/** 查询本地文档库状态 */
export function getHarmonyDocsStatus(): Promise<HarmonyDocsStatus> {
  return invoke<HarmonyDocsStatus>('get_harmony_docs_status')
}

/** 下载/更新 OpenHarmony 文档（耗时较长）；useProxy=true 时 git 走系统代理 */
export function updateHarmonyDocs(preferGitee = true, useProxy = false): Promise<HarmonyDocsStatus> {
  return invoke<HarmonyDocsStatus>('update_harmony_docs', { preferGitee, useProxy })
}

/** 检索本地 OpenHarmony 文档 */
export function searchHarmonyDocs(query: string, limit?: number): Promise<DocEntry[]> {
  return invoke<DocEntry[]>('search_harmony_docs', { query, limit: limit ?? null })
}

/** 读取某篇文档完整 Markdown 原文 */
export function readHarmonyDoc(relPath: string): Promise<string> {
  return invoke<string>('read_harmony_doc', { relPath })
}
