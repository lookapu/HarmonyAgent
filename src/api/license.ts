import { invokeWithError } from './invoke'

/** 档位。序列化值即后端 `licensing::Tier` 的值，改它等于换了一种历史数据格式。 */
export type Tier = 'free' | 'pro' | 'team'

/**
 * 能力键。与后端 `licensing::Capability` 一一对应。
 *
 * **不要在这里另抄一份能力清单**——档位与能力的对应关系由后端
 * `TIER_ENTITLEMENTS` 决定，前端只负责把 key 翻译成文案。
 * 抄一份的必然结果是两处漂移，而漂移的表现是「Pro 页面显示某个能力不可用，
 * 实际点进去能用」，或者反过来。
 */
export type Capability =
  | 'multi_provider_pool'
  | 'team_sharing'
  | 'eval_reports'
  | 'lan_collaboration'
  | 'audit_trail'

export interface TierInfo {
  tier: Tier
  capabilities: Capability[]
  /** null = 不限。 */
  monthly_budget_cny: number | null
  monthly_runs: number | null
  monthly_tool_calls: number | null
}

export interface QuotaSnapshot {
  period: string
  runs: number
  tool_calls: number
  cost_cny: number
}

export interface QuotaBreach {
  limit: 'monthly_budget_cny' | 'monthly_runs' | 'monthly_tool_calls'
  used: number
  cap: number
}

export interface LicenseState {
  tier: Tier
  /**
   * 档位来源。`local` = 用户自己在本地切的，**不是买来的**。
   * UI 必须据此提示，否则用户会以为「Pro」代表已付费。
   */
  issued_by: string
  quota: QuotaSnapshot
  /** 非空时前端必须在顶部显示，不能只藏在角落。 */
  exceeded: QuotaBreach[]
  updated_at: number
}

export const getLicenseState = () => invokeWithError<LicenseState>('get_license_state')

export const listTiers = () => invokeWithError<TierInfo[]>('list_tiers')

/**
 * 本地切换档位。**无支付校验**——项目红线是支付暂缓，
 * 这里搭的是分级模型的骨架，收款那一步留在服务端（见后端同款注释）。
 */
export const setLicenseTier = (tier: Tier) =>
  invokeWithError<LicenseState>('set_license_tier', { tier })
