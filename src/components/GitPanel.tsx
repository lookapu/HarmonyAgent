import { useCallback, useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import Icon from '../icons/Icon'
import {
  gitBranchInfo,
  gitDiscoverRepos,
  gitSwitchBranch,
  gitWorktreeList,
  gitWorktreeCreate,
  gitWorktreeRemove,
  gitWorktreeMerge,
  type GitBranchInfo,
  type WorktreeInfo,
} from '../api/git'
import type { Project } from '../api/project'
import { getItem, setItem } from '../utils/storage'
import { STORAGE_KEYS } from '../constants'

/** localStorage key 前缀：持久化每个项目在 Git 面板选中的仓库目录（一个根目录下多个 git 仓库时） */
function readGitRepo(projectId: string): string | null {
  return getItem(STORAGE_KEYS.GIT_REPO_PREFIX + projectId)
}

function writeGitRepo(projectId: string, repo: string) {
  setItem(STORAGE_KEYS.GIT_REPO_PREFIX + projectId, repo)
}

interface Props {
  project: Project
  /** 当前会话绑定的 worktree（只读指示，null=本地模式） */
  sessionWorktree?: { path: string; branch: string | null } | null
  /** 在指定 worktree 上新建会话（worktree 模式） */
  onNewWorktreeConversation: (wt: { path: string; branch: string }) => void
}

/** 右侧面板：Git（分支切换 + worktree 管理）。绑定 worktree 后 Agent 任务在 worktree 目录执行。 */
export default function GitPanel({ project, sessionWorktree, onNewWorktreeConversation }: Props) {
  const { t } = useTranslation()
  const [info, setInfo] = useState<GitBranchInfo | null>(null)
  const [worktrees, setWorktrees] = useState<WorktreeInfo[]>([])
  /** 发现到的仓库列表（根目录是仓库时为 [project.path]，否则为一级子目录中的仓库） */
  const [repos, setRepos] = useState<string[]>([])
  /** 当前选中仓库（多仓库时由下拉切换） */
  const [activeRepo, setActiveRepo] = useState<string | null>(null)
  const [loading, setLoading] = useState(true)
  /** 进行中的操作标识（防重连点） */
  const [busy, setBusy] = useState<string | null>(null)
  const [err, setErr] = useState<string | null>(null)
  const [msg, setMsg] = useState<string | null>(null)
  const [statusOpen, setStatusOpen] = useState(false)
  const [createBranch, setCreateBranch] = useState('')
  const [createNewBranch, setCreateNewBranch] = useState('')

  const refresh = useCallback(async () => {
    if (!project.path) return
    setLoading(true)
    try {
      // 仓库发现：根目录是仓库返回原路径；否则下沉一级子目录收集，供多仓库切换
      const discovered = await gitDiscoverRepos(project.path).catch(() => [] as string[])
      setRepos(discovered)
      // 优先恢复本项目上次选中的仓库；其次沿用当前 activeRepo；最后回退到第一个仓库
      const persisted = readGitRepo(project.id)
      const target = (persisted && discovered.includes(persisted))
        ? persisted
        : (activeRepo && discovered.includes(activeRepo))
          ? activeRepo
          : (discovered[0] ?? project.path)
      if (target !== activeRepo) setActiveRepo(target)
      const [b, w] = await Promise.all([
        gitBranchInfo(target).catch(() => null),
        gitWorktreeList(target).catch(() => [] as WorktreeInfo[]),
      ])
      setInfo(b)
      setWorktrees(w)
    } finally {
      setLoading(false)
    }
  }, [project.path, project.id, activeRepo])

  useEffect(() => {
    setErr(null)
    setMsg(null)
    setStatusOpen(false)
    refresh()
  }, [refresh])

  /** 包装操作：统一错误提示 / 成功后刷新 */
  const run = async (key: string, fn: () => Promise<unknown>, reload = true) => {
    if (busy) return
    setBusy(key)
    setErr(null)
    setMsg(null)
    try {
      const out = await fn()
      if (typeof out === 'string' && out.trim()) setMsg(out.trim())
      if (reload) await refresh()
    } catch (e) {
      setErr(String(e))
    } finally {
      setBusy(null)
    }
  }

  /** 当前生效的 git 仓库目录：多仓库时为用户选中的仓库，否则为项目根目录 */
  const gitRoot = activeRepo ?? project.path

  const switchTo = (branch: string) =>
    run(`switch:${branch}`, () => gitSwitchBranch(gitRoot, branch))

  const createWt = () => {
    if (!createBranch.trim()) return
    run('create', () =>
      gitWorktreeCreate(gitRoot, createBranch.trim(), createNewBranch.trim() || undefined),
    ).then(() => {
      setCreateNewBranch('')
    })
  }

  const removeWt = (wt: WorktreeInfo) =>
    run(`remove:${wt.path}`, () => gitWorktreeRemove(gitRoot, wt.path))

  const newConvWt = (wt: WorktreeInfo) => {
    onNewWorktreeConversation({ path: wt.path, branch: wt.branch })
  }

  const mergeWt = (wt: WorktreeInfo) =>
    run(`merge:${wt.path}`, () => gitWorktreeMerge(gitRoot, wt.path))

  const isRepo = !!info?.is_repo
  const branchOptions = info?.branches.filter((b) => !b.is_remote) ?? []

  /** 仓库显示名：项目目录内的相对路径，越界时用绝对路径 */
  const repoLabel = (repo: string) => {
    if (repo.startsWith(project.path)) {
      return repo.slice(project.path.length).replace(/^[\\/]+/, '') || repo
    }
    return repo
  }

  return (
    <div className="p-3 space-y-2.5">
      {/* 反馈条（可手动关闭） */}
      {err && (
        <div className="flex items-start gap-1.5 rounded-xl border border-[var(--danger)]/30 bg-[var(--danger)]/10 p-2.5 text-[11px] text-[var(--danger)]">
          <span className="flex-1 min-w-0 whitespace-pre-wrap break-all">{err}</span>
          <button
            onClick={() => setErr(null)}
            title={t('home.gitDismiss')}
            className="shrink-0 p-0.5 rounded-md text-[var(--danger)]/70 hover:text-[var(--danger)] hover:bg-[var(--bg-hover)] transition-colors"
          >
            <Icon name="close" size={11} />
          </button>
        </div>
      )}
      {msg && (
        <div className="flex items-start gap-1.5 rounded-xl border border-[var(--success)]/30 bg-[var(--success)]/10 p-2.5 text-[11px] text-[var(--success)]">
          <span className="flex-1 min-w-0 whitespace-pre-wrap break-all">{msg}</span>
          <button
            onClick={() => setMsg(null)}
            title={t('home.gitDismiss')}
            className="shrink-0 p-0.5 rounded-md text-[var(--success)]/70 hover:text-[var(--success)] hover:bg-[var(--bg-hover)] transition-colors"
          >
            <Icon name="close" size={11} />
          </button>
        </div>
      )}

      {/* ============ 当前会话 worktree 指示（只读） ============ */}
      {sessionWorktree && (
        <div className="rounded-xl border border-[var(--accent)]/25 bg-[var(--accent-soft)]/40 p-2.5">
          <div className="flex items-center gap-1.5">
            <Icon name="git-branch" size={12} className="text-[var(--accent)] shrink-0" />
            <span className="text-[10.5px] font-medium text-[var(--accent)]">{t('home.gitSessionWorktree')}</span>
            {sessionWorktree.branch && (
              <span className="ml-auto shrink-0 text-[10px] px-1.5 py-0.5 rounded-md bg-[var(--accent)]/10 text-[var(--accent)] font-mono">
                {sessionWorktree.branch}
              </span>
            )}
          </div>
          <div className="mt-1 text-[10px] font-mono text-[var(--text-muted)] truncate" title={sessionWorktree.path}>
            {sessionWorktree.path}
          </div>
        </div>
      )}

      {/* ============ 多仓库选择（根目录非仓库时下沉一级发现） ============ */}
      {repos.length > 1 && (
        <div className="rounded-xl modern-card p-2">
          <div className="flex items-center gap-1.5">
            <Icon name="git-branch" size={12} className="text-[var(--text-secondary)] shrink-0" />
            <select
              value={activeRepo ?? project.path}
              onChange={(e) => {
                const v = e.target.value
                setActiveRepo(v)
                writeGitRepo(project.id, v)
              }}
              title={t('home.gitSelectRepo')}
              className="flex-1 min-w-0 h-7 rounded-lg bg-[var(--bg-window)] border border-[var(--border)] px-2 text-[11px] text-[var(--text-secondary)] outline-none focus:border-[var(--accent)] transition-colors"
            >
              {repos.map((r) => (
                <option key={r} value={r}>
                  {repoLabel(r)}
                </option>
              ))}
            </select>
          </div>
        </div>
      )}

      {/* ============ 分支区 ============ */}
      <div className="rounded-xl modern-card p-3">
        <div className="flex items-center gap-1.5 mb-2.5">
          <Icon name="git-branch" size={13} className="text-[var(--text-secondary)]" />
          <span className="text-[12px] font-medium">{t('home.gitBranches')}</span>
          {isRepo && (
            <div className="ml-auto flex items-center gap-1">
              <span className="text-[10px] px-1.5 py-0.5 rounded-md bg-[var(--warning)]/10 text-[var(--warning)]">
                {t('home.gitChanged', { n: info!.changed })}
              </span>
              <span className="text-[10px] px-1.5 py-0.5 rounded-md bg-[var(--accent-soft)] text-[var(--accent)]">
                {t('home.gitUntracked', { n: info!.untracked })}
              </span>
            </div>
          )}
        </div>

        {loading ? (
          <div className="flex items-center justify-center py-6">
            <span className="w-4 h-4 rounded-full border border-[var(--accent)] border-t-transparent animate-spin" />
          </div>
        ) : !isRepo ? (
          <div className="py-3 text-[11.5px] text-[var(--text-muted)] text-center">{t('home.gitNoRepo')}</div>
        ) : (
          <>
            {/* 当前分支 + 状态详情 */}
            <div className="flex items-center gap-2 mb-2">
              <span className="flex-1 min-w-0 flex items-center gap-1.5 px-2 py-1 rounded-lg bg-[var(--bg-window)] border border-[var(--border)]">
                <span className="w-1.5 h-1.5 rounded-full bg-[var(--success)] shrink-0" />
                <span className="text-[11px] text-[var(--text-muted)] shrink-0">{t('home.gitBranchCurrent')}</span>
                <span className="text-[11.5px] font-mono font-medium truncate">{info!.current || '—'}</span>
              </span>
              <button
                onClick={() => setStatusOpen((v) => !v)}
                title={t('home.gitStatusDetail')}
                className={`p-1.5 rounded-lg transition-colors ${statusOpen ? 'text-[var(--accent)] bg-[var(--accent-soft)]' : 'text-[var(--text-muted)] hover:text-[var(--accent)] hover:bg-[var(--bg-hover)]'}`}
              >
                <Icon name="info" size={13} />
              </button>
            </div>
            {statusOpen && info!.status_text.trim() && (
              <pre className="mb-2 max-h-40 overflow-y-auto rounded-lg bg-[var(--bg-window)] border border-[var(--border)] p-2 text-[10.5px] font-mono text-[var(--text-secondary)] whitespace-pre-wrap">
                {info!.status_text}
              </pre>
            )}

            {/* 分支列表 */}
            <div className="space-y-0.5">
              {info!.branches.map((b) => (
                <div
                  key={`${b.is_remote ? 'r' : 'l'}:${b.name}`}
                  className={`flex items-center gap-1.5 px-2 py-1.5 rounded-lg text-[11.5px] ${
                    b.is_current ? 'bg-[var(--accent-soft)] text-[var(--accent)]' : 'hover:bg-[var(--bg-hover)]'
                  }`}
                >
                  <Icon
                    name="git-branch"
                    size={11}
                    className={b.is_current ? '' : 'opacity-40'}
                  />
                  <button
                    onClick={() => switchTo(b.name)}
                    disabled={b.is_current || b.is_remote || busy !== null}
                    title={
                      b.is_remote
                        ? t('home.gitRemoteTip')
                        : b.is_current
                          ? undefined
                          : `${t('home.gitSwitchTo')} ${b.name}`
                    }
                    className="flex-1 min-w-0 text-left font-mono truncate disabled:opacity-60 disabled:cursor-default"
                  >
                    {b.name}
                  </button>
                  {b.is_remote && (
                    <span className="text-[9px] px-1 py-0.5 rounded bg-[var(--bg-hover)] text-[var(--text-muted)] shrink-0">
                      {t('home.gitRemote')}
                    </span>
                  )}
                  {b.is_current && (
                    <span className="text-[9px] px-1 py-0.5 rounded bg-[var(--accent)]/10 shrink-0">
                      {t('home.branchCurrent')}
                    </span>
                  )}
                  {busy === `switch:${b.name}` && (
                    <span className="w-2.5 h-2.5 rounded-full border border-[var(--accent)] border-t-transparent animate-spin shrink-0" />
                  )}
                </div>
              ))}
              {info!.branches.length === 0 && (
                <div className="py-2 text-center text-[11px] text-[var(--text-muted)]">—</div>
              )}
            </div>
          </>
        )}
      </div>

      {/* ============ Worktree 区 ============ */}
      {isRepo && (
        <div className="rounded-xl modern-card p-3">
          <div className="flex items-center gap-1.5 mb-1">
            <Icon name="folder" size={13} className="text-[var(--text-secondary)]" />
            <span className="text-[12px] font-medium">{t('home.gitWorktrees')}</span>
          </div>
          <p className="text-[10.5px] text-[var(--text-muted)] leading-relaxed mb-2.5">{t('home.gitWorktreeTip')}</p>

          {/* worktree 列表 */}
          <div className="space-y-1.5">
            {worktrees.map((wt) => (
                <div key={wt.path} className="rounded-lg border p-2 border-[var(--border)] bg-[var(--bg-window)]/50">
                  <div className="flex items-center gap-1.5 min-w-0">
                    <Icon name={wt.is_main ? 'bolt' : 'folder'} size={11} className="shrink-0 opacity-50" />
                    <span className="text-[11px] font-mono font-medium truncate" title={wt.path}>
                      {wt.is_main ? wt.path : wt.path.split(/[\\/]/).pop()}
                    </span>
                    <span className="ml-auto shrink-0 text-[9px] px-1 py-0.5 rounded bg-[var(--bg-hover)] text-[var(--text-muted)]">
                      {wt.is_main ? t('home.gitMain') : wt.branch}
                    </span>
                  </div>
                  {!wt.is_main && (
                    <div className="mt-1.5 flex items-center gap-1">
                      <button
                        onClick={() => newConvWt(wt)}
                        disabled={busy !== null}
                        title={t('home.newConversation')}
                        className="flex-1 text-[10px] px-1.5 py-1 rounded-md text-[var(--text-secondary)] hover:text-[var(--accent)] hover:bg-[var(--bg-hover)] transition-colors disabled:opacity-40"
                      >
                        {t('home.newConversation')}
                      </button>
                      <button
                        onClick={() => mergeWt(wt)}
                        disabled={busy !== null}
                        title={t('home.gitMerge')}
                        className="flex-1 text-[10px] px-1.5 py-1 rounded-md text-[var(--text-secondary)] hover:text-[var(--success)] hover:bg-[var(--bg-hover)] transition-colors disabled:opacity-40"
                      >
                        {t('home.gitMerge')}
                      </button>
                      <button
                        onClick={() => removeWt(wt)}
                        disabled={busy !== null}
                        title={t('home.gitRemove')}
                        className="shrink-0 p-1 rounded-md text-[var(--text-muted)] hover:text-[var(--danger)] hover:bg-[var(--bg-hover)] transition-colors disabled:opacity-40"
                      >
                        <Icon name="delete" size={11} />
                      </button>
                    </div>
                  )}
                </div>
              ))}
            {worktrees.length === 0 && (
              <div className="py-3 text-center text-[11px] text-[var(--text-muted)]">{t('home.gitWorktreeEmpty')}</div>
            )}
          </div>

          {/* 创建表单 */}
          <div className="mt-2.5 pt-2.5 border-t border-[var(--border)]">
            <div className="flex items-center gap-1.5 mb-1.5">
              <span className="text-[11px] font-medium text-[var(--text-secondary)]">{t('home.gitCreateWorktree')}</span>
            </div>
            <div className="flex gap-1.5">
              <select
                value={createBranch}
                onChange={(e) => setCreateBranch(e.target.value)}
                title={t('home.gitSelectBranch')}
                className="flex-1 min-w-0 h-7 rounded-lg bg-[var(--bg-window)] border border-[var(--border)] px-2 text-[11px] text-[var(--text-secondary)] outline-none focus:border-[var(--accent)] transition-colors"
              >
                <option value="">{t('home.gitSelectBranch')}</option>
                {branchOptions.map((b) => (
                  <option key={b.name} value={b.name}>
                    {b.name}
                  </option>
                ))}
              </select>
              <button
                onClick={createWt}
                disabled={!createBranch.trim() || busy !== null}
                className="shrink-0 h-7 px-3 rounded-lg btn-primary text-[11px] font-medium hover:opacity-90 transition-opacity disabled:opacity-40"
              >
                {busy === 'create' ? '…' : t('home.gitCreate')}
              </button>
            </div>
            <input
              value={createNewBranch}
              onChange={(e) => setCreateNewBranch(e.target.value)}
              placeholder={t('home.gitNewBranchName')}
              className="mt-1.5 w-full h-7 rounded-lg bg-[var(--bg-window)] border border-[var(--border)] px-2 text-[11px] text-[var(--text-secondary)] outline-none placeholder:text-[var(--text-muted)] focus:border-[var(--accent)] transition-colors"
            />
          </div>
        </div>
      )}
    </div>
  )
}


