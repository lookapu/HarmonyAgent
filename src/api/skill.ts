import { invoke } from '@tauri-apps/api/core'

export interface Skill {
  id: string
  name: string
  description: string | null
  directory: string | null
  repo_owner: string | null
  repo_name: string | null
  repo_branch: string
  subdir: string | null
  enabled: boolean
  content_hash: string | null
  installed_at: number
  updated_at: number | null
  /** 作用域：null=用户级(全局，对所有项目生效)；非空=仅该项目生效 */
  project_id: string | null
}

export const listSkills = (projectId?: string | null) =>
  invoke<Skill[]>('list_skills', { projectId: projectId ?? null })
export const importSkillFromGithub = (
  repo: string,
  branch?: string,
  useProxy = false,
  subdir?: string,
  projectId?: string | null,
) =>
  invoke<Skill>('import_skill_from_github', {
    input: { repo, branch, use_proxy: useProxy, subdir, project_id: projectId ?? null },
  })
export const toggleSkill = (id: string, enabled: boolean) => invoke<void>('toggle_skill', { id, enabled })
export const removeSkill = (id: string) => invoke<void>('remove_skill', { id })
/** 把技能复制到另一作用域：targetProjectId 传 null=全局，传项目 id=该项目 */
export const cloneSkill = (id: string, targetProjectId: string | null) =>
  invoke<Skill>('clone_skill', { id, targetProjectId: targetProjectId ?? null })
