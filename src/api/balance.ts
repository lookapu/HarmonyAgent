import { invoke } from '@tauri-apps/api/core'

export interface ProviderBalance {
  provider_id: string
  provider_name: string
  ok: boolean
  currency: string | null
  total: number | null
  used: number | null
  remaining: number | null
  exhausted: boolean
  error: string | null
}

/** 查询所有服务商（或指定 provider）的余额/额度；useProxy: true=走系统代理 / false=直连 */
export const queryBalances = (providerId?: string, useProxy?: boolean) =>
  invoke<ProviderBalance[]>('query_balances', { providerId: providerId ?? null, useProxy: useProxy ?? null })
