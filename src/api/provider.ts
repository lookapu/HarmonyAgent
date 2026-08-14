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
}

export interface UpdateModelInput {
  use_proxy?: boolean
  is_default?: boolean
  display_name?: string
  context_limit?: number
  output_limit?: number
  enabled?: boolean
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

export const listProviders = () => invoke<Provider[]>('list_providers')
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
