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
export const listAvailableVersions = () => invoke<VersionInfo[]>('list_available_versions')
export const installVersion = (version: string) => invoke<string>('install_version', { version })
export const checkBaseUpdate = () => invoke<BaseUpdateInfo>('check_base_update')
