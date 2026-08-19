import { invokeWithError } from './invoke'

/** 令牌元数据 */
export interface LanTokenInfo {
  id: number
  name: string
  /** 到期时间戳（unix 秒，0=永不过期） */
  expires_at: number
  created_at: number
  last_used_at: number
  /** 0=永久；>0=剩余秒数；<0=已过期 */
  remaining_secs: number
  /** 最近一次使用设备（来自会话记录） */
  last_device: string
  /** 最近一次使用时长（秒） */
  last_duration_secs: number
  /** 6 位数字明文（旧令牌可能为空：无法恢复二维码，需重建） */
  token_plain: string | null
}

/** LAN 服务状态 + 配置（token 明文不回传） */
export interface LanStatusInfo {
  running: boolean
  enabled: boolean
  listen_port: number
  read_only: boolean
  token_set: boolean
  /** 令牌列表（不含明文） */
  tokens: LanTokenInfo[]
  /** 本机局域网 IPv4 地址列表 */
  ips: string[]
}

export interface LanConfigInput {
  port?: number
  read_only?: boolean
}

export const startLanServer = () => invokeWithError<void>('start_lan_server')
export const stopLanServer = () => invokeWithError<void>('stop_lan_server')
export const getLanServerStatus = () => invokeWithError<LanStatusInfo>('get_lan_server_status')
export const updateLanServerConfig = (input: LanConfigInput) =>
  invokeWithError<void>('update_lan_server_config', { input })
/** 创建令牌：name 备注名，expires_at 到期时间戳（0=永久）。返回 6 位明文，仅此一次 */
export const createLanToken = (name: string, expiresAt: number) =>
  invokeWithError<string>('create_lan_token', { name, expiresAt })
export const listLanTokens = () => invokeWithError<LanTokenInfo[]>('list_lan_tokens')
/** 撤销令牌：立即失效并断开其全部连接 */
export const revokeLanToken = (id: number) => invokeWithError<void>('revoke_lan_token', { id })
export const getLanIps = () => invokeWithError<string[]>('get_lan_ips')
