import { invokeWithError } from './invoke'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'

export interface DeviceInfo {
  id: string
  state: string
  model: string
  os_version: string
  connection?: 'online' | 'offline' | 'unauthorized' | 'unknown'
  authorized?: boolean
  api_level?: number | null
  architecture?: string
  resolution?: string
  capabilities?: string[]
  observed_at?: number
  is_default: boolean
}

export const listDevices = () => invokeWithError<DeviceInfo[]>('list_devices')
export const setDefaultDevice = (deviceId: string) =>
  invokeWithError<void>('set_default_device', { deviceId })
export interface DeviceDetail {
  brand: string
  manufacturer: string
  model: string
  os_version: string
  resolution: string
  battery: string
  ram: string
  /** 存储用量文本（如 "28.2GB / 220.3GB（13%）"） */
  storage: string
  /** CPU 当前频率（如 "1.62GHz"） */
  cpu_freq: string
  /** 电池状态文本（充电状态 + 电压 + 电流 + 电芯技术） */
  battery_status: string
  /** 电池温度 ℃（如 "30.0℃"） */
  battery_temp: string
}
export const getDeviceDetail = (deviceId: string) =>
  invokeWithError<DeviceDetail>('get_device_detail', { deviceId })
export const hdcAvailable = () => invokeWithError<boolean>('hdc_available')
export const startHdcService = () => invokeWithError<string>('start_hdc_service')
export const stopHdcService = () => invokeWithError<string>('stop_hdc_service')
export const captureDeviceScreenshot = (projectId: string, deviceId?: string, projectName?: string) =>
  invokeWithError<string>('capture_device_screenshot', { projectId, deviceId: deviceId ?? null, projectName: projectName ?? null })

/** 项目截图文件条目 */
export interface ShotFile {
  name: string
  path: string
  size: number
  mtime: number
}
export const listDeviceScreenshots = (projectId: string) =>
  invokeWithError<ShotFile[]>('list_device_screenshots', { projectId })
export const deleteDeviceScreenshot = (projectId: string, name: string) =>
  invokeWithError<void>('delete_device_screenshot', { projectId, name })

export interface InstalledApp {
  package: string
  launcher: boolean
}
export const listInstalledApps = (deviceId: string) =>
  invokeWithError<InstalledApp[]>('list_installed_apps', { deviceId })
export const launchApp = (deviceId: string, pkg: string) =>
  invokeWithError<string>('launch_app', { deviceId, package: pkg })
export const stopApp = (deviceId: string, pkg: string) =>
  invokeWithError<string>('stop_app', { deviceId, package: pkg })

export interface DeviceProcess {
  pid: string
  name: string
}
export const listDeviceProcesses = (deviceId: string) =>
  invokeWithError<DeviceProcess[]>('list_device_processes', { deviceId })

export interface HilogStreamOptions {
  package?: string
  tag?: string
  level?: 'D' | 'I' | 'W' | 'E' | 'F'
}
export const startHilogStream = (deviceId: string, opts: HilogStreamOptions = {}) =>
  invokeWithError<void>('start_hilog_stream', {
    deviceId,
    package: opts.package ?? null,
    tag: opts.tag ?? null,
    level: opts.level ?? null,
  })
export const stopHilogStream = (deviceId: string) =>
  invokeWithError<void>('stop_hilog_stream', { deviceId })

export interface DevicePerf {
  /** CPU 总占用率 %（0-100，读取失败为 -1） */
  cpu: number
  /** 内存占用率 %（0-100，读取失败为 -1） */
  mem: number
  /** 电池电量 %（-1 表示无法读取） */
  battery: number
  /** 温度 ℃（-1 表示无法读取） */
  temp: number
  /** 时间戳（ms） */
  ts: number
}
export const getDevicePerf = (deviceId: string) =>
  invokeWithError<DevicePerf>('get_device_perf', { deviceId })

export interface HilogLinePayload {
  device_id: string
  line: string
}
export const onHilogLine = (cb: (payload: HilogLinePayload) => void): Promise<UnlistenFn> =>
  listen<HilogLinePayload>('device-hilog-line', (e) => cb(e.payload))
export const onHilogEnded = (cb: (deviceId: string) => void): Promise<UnlistenFn> =>
  listen<{ device_id: string }>('device-hilog-ended', (e) => cb(e.payload.device_id))
