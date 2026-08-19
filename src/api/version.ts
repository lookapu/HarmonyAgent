import { invokeWithError } from './invoke'

export interface VersionInfo {
  version: string
  tag: string | null
  is_current: boolean
}

export interface BaseUpdateInfo {
  current: string
  latest: string
  can_update: boolean
  package: string
}

export const getCurrentVersion = () => invokeWithError<string>('get_current_version')
export const listAvailableVersions = (useProxy: boolean | null = null) =>
  invokeWithError<VersionInfo[]>('list_available_versions', { useProxy })
export const installVersion = (version: string, useProxy: boolean | null = null) =>
  invokeWithError<string>('install_version', { version, useProxy })
export const checkBaseUpdate = (useProxy: boolean | null = null) =>
  invokeWithError<BaseUpdateInfo>('check_base_update', { useProxy })
