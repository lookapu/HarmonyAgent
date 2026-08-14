import { invoke } from '@tauri-apps/api/core'

export interface McpServer {
  id: string
  name: string
  server_type: string
  command: string
  args: string
  env: string
  enabled: boolean
  description: string | null
  homepage: string | null
  created_at: number
  /** 最近一次连接测试结果（null=尚未测试） */
  last_test_ok: boolean | null
  last_test_at: number | null
  last_test_error: string | null
  /** 作用域：null=用户级(全局，对所有项目生效)；非空=仅该项目生效 */
  project_id: string | null
}

export interface CreateMcpInput {
  name: string
  server_type?: string
  command: string[]
  env?: Record<string, string>
  description?: string
  homepage?: string
  project_id?: string | null
}

/** 编辑 MCP 服务器入参（全量替换，用于修改连接配置） */
export interface UpdateMcpInput {
  name: string
  server_type?: string
  command: string[]
  env?: Record<string, string>
  description?: string
  homepage?: string
}

/** 从 URL 获取到的 MCP 服务器草稿 */
export interface McpDraft {
  name: string
  command: string[]
  env: Record<string, string> | null
  description: string | null
}

export const listMcpServers = (projectId?: string | null) =>
  invoke<McpServer[]>('list_mcp_servers', { projectId: projectId ?? null })
export const addMcpServer = (input: CreateMcpInput) => invoke<McpServer>('add_mcp_server', { input })
export const updateMcpServer = (id: string, input: UpdateMcpInput) => invoke<McpServer>('update_mcp_server', { id, input })
export const testMcpServer = (id: string) => invoke<string>('test_mcp_server', { id })
export const toggleMcpServer = (id: string, enabled: boolean) => invoke<void>('toggle_mcp_server', { id, enabled })
export const removeMcpServer = (id: string) => invoke<void>('remove_mcp_server', { id })
/** 把 MCP 服务器复制到另一作用域：targetProjectId 传 null=全局，传项目 id=该项目 */
export const cloneMcpServer = (id: string, targetProjectId: string | null) =>
  invoke<McpServer>('clone_mcp_server', { id, targetProjectId: targetProjectId ?? null })
/** 导出指定作用域的 MCP 配置为 JSON 文本 */
export const exportMcpConfig = (projectId: string | null) =>
  invoke<string>('export_mcp_config', { projectId: projectId ?? null })
/** 从 JSON 文本导入 MCP 配置到目标作用域；overwrite=true 同名覆盖 */
export const importMcpConfig = (json: string, targetProjectId: string | null, overwrite = false) =>
  invoke<number>('import_mcp_config', { json, targetProjectId: targetProjectId ?? null, overwrite })
export const fetchMcpFromUrl = (url: string, useProxy = false) =>
  invoke<McpDraft[]>('fetch_mcp_from_url', { url, useProxy })
