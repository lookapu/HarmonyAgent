import { invokeWithError } from './invoke'

export interface ProxyStatus {
  running: boolean
  listen_address: string
  listen_port: number
  total_requests: number
  active_provider: string | null
}

export interface ProxyConfigInput {
  listen_address?: string
  listen_port?: number
  auto_failover?: boolean
  max_retries?: number
  streaming_first_byte_timeout_s?: number
  non_streaming_timeout_s?: number
  /** 是否随应用启动自动开启代理 */
  enabled?: boolean
}

export interface ProxyConfigInfo {
  enabled: boolean
  listen_address: string
  listen_port: number
  auto_failover: boolean
  max_retries: number
  streaming_first_byte_timeout_s: number
  non_streaming_timeout_s: number
}

export const startProxy = () => invokeWithError<void>('start_proxy')
export const stopProxy = () => invokeWithError<void>('stop_proxy')
export const getProxyStatus = () => invokeWithError<ProxyStatus>('get_proxy_status')
export const getProxyConfig = () => invokeWithError<ProxyConfigInfo>('get_proxy_config')
export const updateProxyConfig = (config: ProxyConfigInput) => invokeWithError<void>('update_proxy_config', { input: config })
