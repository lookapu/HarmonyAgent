import { invokeWithError } from './invoke'

/** Git 运行时状态（来源：system=系统已装 / upgraded=内置升级版 / bundled=出厂捆绑版 / none=不可用） */
export interface GitRuntimeInfo {
  git_version: string
  source: string
  dir: string | null
  upgraded_dir: string | null
  bundled_dir: string | null
  /** git --version 执行失败原因（git_version 为空时展示） */
  git_error: string | null
}

/** 查询 Git 运行时状态（版本、来源、目录） */
export const getGitRuntime = () => invokeWithError<GitRuntimeInfo>('get_git_runtime')

/** 查询 Git for Windows 最新发布 tag（GitHub API） */
export const fetchGitLatestVersion = () => invokeWithError<string>('fetch_git_latest_version')

/** 升级 Git 运行时到最新版（下载 PortableGit 自解压包，静默解压生效）；useProxy: true=走系统代理 / false=直连 */
export const upgradeGitRuntime = (useProxy?: boolean) =>
  invokeWithError<GitRuntimeInfo>('upgrade_git_runtime', { useProxy: useProxy ?? null })

/** 删除升级版，回到出厂捆绑版本 */
export const resetGitRuntime = () => invokeWithError<GitRuntimeInfo>('reset_git_runtime')
