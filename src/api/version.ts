import { invoke } from '@tauri-apps/api/core'

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

export const getCurrentVersion = () => invoke<string>('get_current_version')
export const listAvailableVersions = (useProxy: boolean | null = null) =>
  invoke<VersionInfo[]>('list_available_versions', { useProxy })
export const installVersion = (version: string, useProxy: boolean | null = null) =>
  invoke<string>('install_version', { version, useProxy })
export const checkBaseUpdate = (useProxy: boolean | null = null) =>
  invoke<BaseUpdateInfo>('check_base_update', { useProxy })
