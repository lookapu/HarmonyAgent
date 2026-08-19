import { invokeWithError } from './invoke'

/** 应用基座信息（安装位置 / 数据目录 / 当前版本 / 内置 Node 目录） */
export interface AppInfo {
  version: string
  install_dir: string | null
  data_dir: string | null
  bundled_node_dir: string | null
  upgraded_node_dir: string | null
}

/** 环境总览（基座 + Node 运行时 + 最新 LTS + Git 运行时 + 工具链） */
export interface EnvironmentInfo {
  app: AppInfo
  node: import('./nodeRuntime').NodeRuntimeInfo
  node_latest_lts: string | null
  git: import('./gitRuntime').GitRuntimeInfo
  git_latest: string | null
  toolchain: import('./health').ToolchainCheck[]
}

/** 应用基座信息（当前版本、安装位置、数据目录） */
export const getAppInfo = () => invokeWithError<AppInfo>('get_app_info')

/** 环境总览一次性加载 */
export const getEnvironmentInfo = (customPaths?: string[]) =>
  invokeWithError<EnvironmentInfo>('get_environment_info', {
    customPaths: customPaths ?? [],
  })

/** 查询 Node 最新 LTS 版本号 */
export const fetchNodeLatestLts = () => invokeWithError<string>('fetch_node_latest_lts')

/** 手动升级工具包：下载 zip 解压到 toolkits/<name>，返回解压目录；useProxy: true=走系统代理 / false=直连 */
export const installToolkit = (name: string, url: string, useProxy?: boolean) =>
  invokeWithError<string>('install_toolkit', { name, url, useProxy: useProxy ?? null })

/** 某工具的一个可用环境目录（“选择环境”下拉展示） */
export interface ToolCandidate {
  path: string
  /** 来源：custom=自定义目录 / bundled=软件内置 / deveco=DevEco Studio / path=系统 PATH */
  source: string
}

/** 从本地 zip 安装工具包（官方 Command Line Tools 压缩包），解压到 toolkits/<name> */
export const installToolkitFromZip = (name: string, zipPath: string) =>
  invokeWithError<string>('install_toolkit_from_zip', { name, zipPath })


/** 枚举某工具的所有候选环境目录（自定义 > 软件内置 > DevEco Studio > 系统 PATH） */
export const getToolchainCandidates = (name: string, customPaths: string[]) =>
  invokeWithError<ToolCandidate[]>('get_toolchain_candidates', { name, customPaths: customPaths ?? [] })

/** 读取工具版本（执行 --version，15 秒超时） */
export const getToolVersion = (path: string) => invokeWithError<string>('get_tool_version', { path })
