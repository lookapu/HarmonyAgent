import { invoke } from '@tauri-apps/api/core'

export interface Skill {
  id: string
  name: string
  description: string | null
  directory: string | null
  repo_owner: string | null
  repo_name: string | null
  /** 仓库平台：github / gitee */
  repo_host: string | null
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
/** 从 GitHub/Gitee 仓库导入 Skill：repo 支持 https://github.com/owner/name、
 *  https://gitee.com/owner/name、git@gitee.com:owner/name.git 或 owner/name（缺省 github） */
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

/* ============ Skill 调用记录（use_skill 工具落库） ============ */

/** 技能调用统计（按技能聚合：次数 / 最近调用时间） */
export interface SkillUsageStat {
  skill_id: string
  skill_name: string
  call_count: number
  /** unix 秒 */
  last_called_at: number | null
}

/** 技能调用明细（时间线：一次 use_skill 调用） */
export interface SkillUsageEvent {
  id: string
  skill_id: string
  skill_name: string
  conversation_title: string
  project_id: string
  created_at: number
}

/** 技能调用统计；projectId 空 = 全部项目 */
export const listSkillUsage = (projectId?: string | null) =>
  invoke<SkillUsageStat[]>('list_skill_usage', { projectId: projectId ?? null })

/** 最近技能调用明细；projectId 空 = 全部项目 */
export const listSkillUsageEvents = (projectId?: string | null, limit?: number) =>
  invoke<SkillUsageEvent[]>('list_skill_usage_events', {
    projectId: projectId ?? null,
    limit: limit ?? 100,
  })
