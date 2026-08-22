import { invokeWithError } from './invoke'
import type { Project } from './project'

/** 分支条目（本地优先，同名远端分支不重复） */
export interface GitBranch {
  name: string
  is_current: boolean
  is_remote: boolean
}

/** 分支信息 + 工作区状态摘要（非 git 仓库 is_repo=false，不报错） */
export interface GitBranchInfo {
  is_repo: boolean
  current: string
  branches: GitBranch[]
  /** 已跟踪改动条数（M/D/A/R 等） */
  changed: number
  /** 未跟踪文件条数（??） */
  untracked: number
  /** git status --short 原文 */
  status_text: string
}

/** worktree 条目（第一个是主仓库） */
export interface WorktreeInfo {
  path: string
  branch: string
  is_main: boolean
}

/** 发现项目下的 git 仓库：根目录是仓库时返回 [projectPath]；否则返回一级子目录中自身是仓库根的目录列表 */
export const gitDiscoverRepos = (projectPath: string) =>
  invokeWithError<string[]>('git_discover_repos', { projectPath })

/** 分支信息 + 工作区摘要（项目目录；非 git 仓库 is_repo=false） */
export const gitBranchInfo = (projectPath: string) =>
  invokeWithError<GitBranchInfo>('git_branch_info', { projectPath })

/** 切换分支（未提交改动冲突时返回友好错误） */
export const gitSwitchBranch = (projectPath: string, branch: string) =>
  invokeWithError<string>('git_switch_branch', { projectPath, branch })

/** worktree 列表（第一个是主仓库） */
export const gitWorktreeList = (projectPath: string) =>
  invokeWithError<WorktreeInfo[]>('git_worktree_list', { projectPath })

/**
 * 创建 worktree：目录放在项目同级 `<项目名>-<分支名>`。
 * newBranch 提供时从 branch 新建分支；返回新 worktree 绝对路径。
 */
export const gitWorktreeCreate = (projectPath: string, branch: string, newBranch?: string) =>
  invokeWithError<string>('git_worktree_create', { projectPath, branch, newBranch })

/** 删除 worktree（目录内有未提交改动时后端返回提示） */
export const gitWorktreeRemove = (projectPath: string, wtPath: string) =>
  invokeWithError<string>('git_worktree_remove', { projectPath, wtPath })

/** 项目绑定 worktree（worktreePath=null 解除绑定）；返回更新后的项目 */
export const setProjectWorktree = (projectId: string, worktreePath: string | null) =>
  invokeWithError<Project>('set_project_worktree', { projectId, worktreePath })

/** 将 worktree 分支合并回主仓库当前分支（要求 worktree 内改动已提交） */
export const gitWorktreeMerge = (projectPath: string, wtPath: string) =>
  invokeWithError<string>('git_worktree_merge', { projectPath, wtPath })

/** 单文件工作区 diff（变更审查；file 为相对项目根路径，未跟踪新文件返回内容预览） */
export const gitFileDiff = (projectPath: string, file: string) =>
  invokeWithError<string>('git_file_diff', { projectPath, file })

/** 接受变更：git add 指定文件（相对路径列表）；返回成功条数 */
export const gitAcceptChanges = (projectPath: string, files: string[]) =>
  invokeWithError<number>('git_accept_changes', { projectPath, files })

/** 还原变更：已跟踪文件丢弃改动；未跟踪新文件拒绝并提示 */
export const gitRevertFile = (projectPath: string, file: string) =>
  invokeWithError<string>('git_revert_file', { projectPath, file })

/** 文件变更统计（+N/-M）：对文件列表累加增删行数（未跟踪文件只计数量） */
export interface DiffStat {
  files: number
  insertions: number
  deletions: number
}

export const gitDiffStat = (projectPath: string, files: string[]) =>
  invokeWithError<DiffStat>('git_diff_stat', { projectPath, files })

/** 初始化 Git 仓库（git init）：当前目录非仓库时执行，已是仓库返回提示 */
export const gitInitRepo = (projectPath: string) =>
  invokeWithError<string>('git_init_repo', { projectPath })

/** 单文件/目录的 Git 状态（文件树右键菜单；非仓库 is_repo=false 不报错） */
export interface GitFileStatus {
  is_repo: boolean
  /** 是否被 .gitignore 忽略（含父目录规则） */
  ignored: boolean
  /** 是否已被 git 跟踪 */
  tracked: boolean
  /** none | clean | ignored | untracked | modified | staged | deleted */
  status: string
}

export const gitFileStatus = (projectPath: string, path: string) =>
  invokeWithError<GitFileStatus>('git_file_status', { projectPath, path })

/** 文件树 Git 状态批量查询（懒加载树着色）：变更集（含目录聚合）+ 已加载节点忽略判定 */
export interface GitTreeEntry {
  path: string
  /** untracked | modified | staged | deleted */
  status: string
}

export interface GitTreeStatusBundle {
  is_repo: boolean
  entries: GitTreeEntry[]
  /** paths 参数中命中 .gitignore 的路径 */
  ignored: string[]
}

export const getFileTreeGitStatus = (projectPath: string, root: string | null, paths: string[]) =>
  invokeWithError<GitTreeStatusBundle>('get_file_tree_git_status', {
    projectPath,
    root: root ?? null,
    paths,
  })

/** 把路径追加到项目根 .gitignore（目录自动补尾部 /；已存在规则时跳过） */
export const gitIgnoreAdd = (projectPath: string, path: string) =>
  invokeWithError<string>('git_ignore_add', { projectPath, path })

/** 从 Git 历史排除（git rm --cached，工作区文件保留） */
export const gitUntrack = (projectPath: string, path: string) =>
  invokeWithError<string>('git_untrack', { projectPath, path })

/** 暂存改动（git add；被忽略文件返回提示） */
export const gitStage = (projectPath: string, path: string) =>
  invokeWithError<string>('git_stage', { projectPath, path })

/** 单文件/目录的提交历史（最近 15 条） */
export const gitFileLog = (projectPath: string, path: string) =>
  invokeWithError<string>('git_file_log', { projectPath, path })
