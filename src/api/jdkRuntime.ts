import { invokeWithError } from './invoke'
import type { RuntimeProgress } from './runtimeProgress'

/** JDK 安装/更新进度（事件 `jdk-install-progress`，与 Node/Git 共用 RuntimeProgress 形状） */
export type JdkProgress = RuntimeProgress

/** 单个 JDK 版本（来源：bundled=出厂捆绑版 / upgraded=在线安装版） */
export interface JdkVersionInfo {
  feature: string
  full_version: string
  path: string
  source: string
  is_default: boolean
}

/** JDK 运行时状态（多版本 + 默认版本） */
export interface JdkRuntimeInfo {
  /** 已安装版本列表（feature 降序） */
  versions: JdkVersionInfo[]
  /** 当前生效（默认）目录；无任何 JDK 时为 null */
  active_dir: string | null
  /** 生效 JDK 版本（如 17.0.20） */
  active_version: string | null
  /** 系统环境变量 JAVA_HOME（存在时优先于内置） */
  system_java_home: string | null
}

/** 已装 JDK 的更新检查结果 */
export interface JdkUpdateInfo {
  feature: string
  installed: string
  latest: string
  updatable: boolean
}

/** 查询 JDK 运行时状态（版本列表、默认版本、系统 JAVA_HOME） */
export const getJdkRuntime = () => invokeWithError<JdkRuntimeInfo>('get_jdk_runtime')

/** 查询可安装的 feature 版本（Adoptium LTS 列表，如 8/11/17/21/25）；
 * useProxy: true=强制走系统代理 / false=直连；缺省=自动（优先系统代理，无则直连） */
export const fetchJdkReleases = (useProxy?: boolean) =>
  invokeWithError<string[]>('fetch_jdk_releases', { useProxy: useProxy ?? null })

/** 在线安装/更新指定 feature 版本的 JDK（同 feature 已装时为覆盖更新）；
 * 下载进度通过 `jdk-install-progress` 事件推送；useProxy 缺省=自动（优先系统代理，无则直连） */
export const installJdk = (feature: string, useProxy?: boolean) =>
  invokeWithError<JdkRuntimeInfo>('install_jdk', { feature, useProxy: useProxy ?? null })

/** 检查已装 JDK 是否有可用的补丁更新（网络不可达时 reject，前端静默降级） */
export const checkJdkUpdates = () => invokeWithError<JdkUpdateInfo[]>('check_jdk_updates')

/** 设置默认 JDK 版本（多版本并存时切换构建/命令使用的 JDK） */
export const setDefaultJdk = (feature: string) =>
  invokeWithError<JdkRuntimeInfo>('set_default_jdk', { feature })

/** 卸载升级版 JDK（捆绑版不可卸载）；卸载默认版本时自动回落其他版本 */
export const uninstallJdk = (feature: string) =>
  invokeWithError<JdkRuntimeInfo>('uninstall_jdk', { feature })
