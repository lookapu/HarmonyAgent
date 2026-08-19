import { invokeWithError } from './invoke'

export interface ProviderBalance {
  provider_id: string
  provider_name: string
  ok: boolean
  /** 该服务商未提供余额查询接口（前端据此直接不展示余额卡片） */
  unsupported: boolean
  currency: string | null
  total: number | null
  used: number | null
  remaining: number | null
  exhausted: boolean
  error: string | null
}

/** 查询所有服务商（或指定 provider）的余额/额度；useProxy: true=走系统代理 / false=直连 */
export const queryBalances = (providerId?: string, useProxy?: boolean) =>
  invokeWithError<ProviderBalance[]>('query_balances', { providerId: providerId ?? null, useProxy: useProxy ?? null })
