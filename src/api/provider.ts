import { invoke } from '@tauri-apps/api/core'

/** 多协议端点（同一厂商可提供 OpenAI / Anthropic / Gemini 多套端点，如 DeepSeek） */
export interface EndpointDef {
  protocol: string // openai | anthropic | gemini
  base_url: string
}

export interface Provider {
  id: string
  name: string
  provider_type: string
  protocol: string // openai | anthropic | gemini
  base_url: string
  /** 多协议端点（可选：对话按所选协议匹配端点） */
  endpoints: EndpointDef[]
  api_key: string | null
  npm_package: string | null
  is_active: boolean
  in_failover_queue: boolean
  priority: number
  cost_multiplier: number
  limit_daily_cny: number | null
  limit_monthly_cny: number | null
  settings_json: string
  notes: string | null
  icon: string | null
  created_at: number
  updated_at: number
}

export interface CreateProviderInput {
  name: string
  provider_type: string
  protocol?: string // openai | anthropic | gemini
  base_url: string
  /** 多协议端点（可选：如 DeepSeek 同时提供 OpenAI 与 Anthropic 端点） */
  endpoints?: EndpointDef[]
  api_key?: string
  npm_package?: string
  models?: CreateModelInput[]
  notes?: string
}

export interface CreateModelInput {
  model_id: string
  display_name?: string
  tool_call?: boolean
  context_limit?: number
  output_limit?: number
  input_modalities?: string[]
  output_modalities?: string[]
  use_proxy?: boolean // 是否走系统代理
}

export interface ProviderModel {
  id: string
  provider_id: string
  model_id: string
  display_name: string | null
  tool_call: boolean
  context_limit: number
  output_limit: number
  input_modalities: string
  output_modalities: string
  is_default: boolean
  use_proxy: boolean
  enabled: boolean
  created_at: number
  /** 手动排序序号（默认模型强制置顶后，其余按此升序排列） */
  sort_order: number
}

export interface UpdateModelInput {
  use_proxy?: boolean
  is_default?: boolean
  display_name?: string
  context_limit?: number
  output_limit?: number
  enabled?: boolean
  /** 输入模态（text/image/audio/video） */
  input_modalities?: string[]
  /** 输出模态（text/image/audio/video） */
  output_modalities?: string[]
}

export interface UpdateProviderInput {
  name?: string
  base_url?: string
  api_key?: string
  npm_package?: string
  notes?: string
  priority?: number
  protocol?: string // openai | anthropic | gemini
  endpoints?: EndpointDef[]
  limit_daily_cny?: number | null // 日预算（元），0/null 表示不限制
  limit_monthly_cny?: number | null // 月预算（元），0/null 表示不限制
}

export interface SyncModelsResult {
  provider_id: string
  /** 平台当前返回的模型 ID 列表 */
  remote_models: string[]
  /** 本地已配置但平台当前不可用的模型 ID（默认模型等旧配置） */
  missing: string[]
  /** 平台当前有、但本地未配置的模型 ID（新增候选） */
  new_models: string[]
  /** 拉取远端模型列表失败时的原因（null=成功） */
  error: string | null
}

export const listProviders = () => invoke<Provider[]>('list_providers')
/** 手动排序 Provider：orderedIds 为全部 Provider 的新顺序（含当前激活的），返回排序后的列表 */
export const reorderProviders = (orderedIds: string[]) => invoke<Provider[]>('reorder_providers', { orderedIds })
export const createProvider = (input: CreateProviderInput) => invoke<Provider>('create_provider', { input })
export const updateProvider = (id: string, input: UpdateProviderInput) => invoke<Provider>('update_provider', { id, input })
export const deleteProvider = (id: string) => invoke<void>('delete_provider', { id })
export const switchProvider = (id: string) => invoke<void>('switch_provider', { id })
export const testProvider = (id: string) => invoke<string>('test_provider', { id })
export const listProviderModels = (providerId: string) => invoke<ProviderModel[]>('list_provider_models', { providerId })
export const updateModel = (id: string, input: UpdateModelInput) => invoke<ProviderModel>('update_model', { id, input })
export const addModel = (providerId: string, input: CreateModelInput) =>
  invoke<ProviderModel>('add_model', { providerId, input })
export const removeModel = (id: string) => invoke<void>('remove_model', { id })
/** 手动排序模型：orderedIds 为该 Provider 下模型的新顺序（需包含全部模型 ID），返回排序后的模型列表 */
export const reorderProviderModels = (providerId: string, orderedIds: string[]) =>
  invoke<ProviderModel[]>('reorder_provider_models', { providerId, orderedIds })
export const syncProviderModels = (providerId: string) =>
  invoke<SyncModelsResult>('sync_provider_models', { id: providerId })
