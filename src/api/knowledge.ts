import { invokeWithError } from './invoke'

export interface KnowledgeEntry {
  id: string
  keywords: string
  title: string
  cause: string
  fix: string
  enabled: boolean
  builtin: boolean
  project_id: string | null
  hit_count: number
  created_at: number
  updated_at: number | null
}

export interface KnowledgeInput {
  keywords: string
  title: string
  cause?: string
  fix?: string
  enabled?: boolean
}

export const listKnowledge = (projectId: string | null) =>
  invokeWithError<KnowledgeEntry[]>('list_knowledge', { projectId: projectId ?? null })
export const addKnowledge = (input: KnowledgeInput, projectId: string | null) =>
  invokeWithError<KnowledgeEntry>('add_knowledge', { input, projectId: projectId ?? null })
export const updateKnowledge = (id: string, input: KnowledgeInput, projectId: string | null) =>
  invokeWithError<void>('update_knowledge', { id, input, projectId: projectId ?? null })
export const toggleKnowledge = (id: string, enabled: boolean, projectId: string | null) =>
  invokeWithError<void>('toggle_knowledge', { id, enabled, projectId: projectId ?? null })
export const deleteKnowledge = (id: string) =>
  invokeWithError<void>('delete_knowledge', { id })
export const cloneKnowledge = (id: string, targetProjectId: string | null) =>
  invokeWithError<KnowledgeEntry>('clone_knowledge', { id, targetProjectId: targetProjectId ?? null })

export interface SaveFromTextInput {
  title?: string
  error_text: string
  fix?: string
  cause?: string
}
export const saveKnowledgeFromText = (input: SaveFromTextInput, projectId: string | null) =>
  invokeWithError<KnowledgeEntry>('save_knowledge_from_text', { input, projectId: projectId ?? null })
