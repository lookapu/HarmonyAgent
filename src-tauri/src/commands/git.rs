//! Git 面板命令：分支信息/切换 + worktree 管理（创建/删除/绑定/合并）。
//! 供前端右侧 Git 面板调用；执行统一复用 agent::tools::run_cmd（隐藏窗口、超时、输出截断）。

use crate::db::models::Project;
use crate::db::DbState;
use rusqlite::{params, OptionalExtension};
use serde::Serialize;
use std::path::Path;
use tauri::State;

#[derive(Debug, Clone, Serialize)]
pub struct GitBranch {
    pub name: String,
    pub is_current: bool,
    pub is_remote: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct GitBranchInfo {
    pub is_repo: bool,
    pub current: String,
    pub branches: Vec<GitBranch>,
    /// 已跟踪的改动条数（M/D/A/R 等）
    pub changed: usize,
    /// 未跟踪文件条数（??）
    pub untracked: usize,
    /// git status --short 原文（面板可展开查看）
    pub status_text: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorktreeInfo {
    pub path: String,
    pub branch: String,
    pub is_main: bool,
}

/// 发现项目下的 git 仓库：根目录自身是仓库（或嵌套于某仓库内，git 向上解析成功）时
/// 返回 `[project_path]`，保持现状（后续命令仍以 project_path 为 cwd）；
/// 否则下沉一级子目录，收集自身是仓库根（含 .git 目录或 gitdir 文件）的目录，
/// 供 Git 面板多仓库切换。
#[tauri::command]
pub async fn git_discover_repos(project_path: String) -> Result<Vec<String>, String> {
    if project_path.trim().is_empty() {
        return Err("未指定项目目录".into());
    }
    let root = Path::new(&project_path);
    if !root.is_dir() {
        return Err(format!("项目目录不存在: {}", root.display()));
    }
    // 根目录（或其祖先）是仓库：视为单仓库，返回原路径
    if crate::agent::tools::run_cmd(
        "git",
        &["rev-parse".into(), "--show-toplevel".into()],
        Some(root),
        15,
    )
    .await
    .is_ok()
    {
        return Ok(vec![project_path]);
    }
    // 下沉一级：跳过隐藏/依赖/产物目录，收集直接子目录中是仓库根的
    let mut dirs: Vec<String> = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return Ok(dirs);
    };
    for e in entries.flatten() {
        let p = e.path();
        if !p.is_dir() {
            continue;
        }
        let name = e.file_name().to_string_lossy().to_string();
        if name.starts_with('.')
            || matches!(
                name.as_str(),
                "node_modules" | "oh_modules" | "build" | "target" | ".arkui-x"
            )
        {
            continue;
        }
        dirs.push(p.to_string_lossy().to_string());
    }
    dirs.sort();
    let repos: Vec<String> = dirs
        .into_iter()
        .filter(|d| Path::new(d).join(".git").exists())
        .collect();
    Ok(repos)
}

/// 分支信息 + 工作区摘要（非 git 仓库时 is_repo=false，不报错）
#[tauri::command]
pub async fn git_branch_info(project_path: String) -> Result<GitBranchInfo, String> {
    if project_path.trim().is_empty() {
        return Err("未指定项目目录".into());
    }
    let cwd = Path::new(&project_path);
    let current = match crate::agent::tools::run_cmd(
        "git",
        &["branch".into(), "--show-current".into()],
        Some(cwd),
        15,
    )
    .await
    {
        Ok(b) => b.trim().to_string(),
        Err(_) => {
            return Ok(GitBranchInfo {
                is_repo: false,
                current: String::new(),
                branches: vec![],
                changed: 0,
                untracked: 0,
                status_text: String::new(),
            })
        }
    };
    // 分支列表：本地优先，同名远程分支不重复展示（HEAD 跳过）
    let mut branches: Vec<GitBranch> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    if let Ok(out) = crate::agent::tools::run_cmd(
        "git",
        &["branch".into(), "-a".into(), "--no-color".into()],
        Some(cwd),
        15,
    )
    .await
    {
        for line in out.lines() {
            let line = line.trim();
            if line.is_empty() || line == "HEAD" {
                continue;
            }
            let (is_current, raw) = if let Some(rest) = line.strip_prefix("* ") {
                (true, rest.trim().to_string())
            } else if let Some(rest) = line.strip_prefix("+ ") {
                (true, rest.trim().to_string())
            } else {
                (false, line.to_string())
            };
            let is_remote = raw.starts_with("remotes/");
            let name = raw.trim_start_matches("remotes/origin/").to_string();
            if is_remote {
                if seen.contains(&name) {
                    continue;
                }
            } else {
                seen.insert(name.clone());
            }
            branches.push(GitBranch {
                name,
                is_current,
                is_remote,
            });
        }
    }
    // 状态摘要：统计已跟踪改动与未跟踪文件
    let mut changed = 0usize;
    let mut untracked = 0usize;
    let status_text = match crate::agent::tools::run_cmd(
        "git",
        &["status".into(), "--short".into()],
        Some(cwd),
        15,
    )
    .await
    {
        Ok(s) => {
            for line in s.lines() {
                if line.trim_start().starts_with("??") {
                    untracked += 1;
                } else {
                    changed += 1;
                }
            }
            s
        }
        Err(e) => e,
    };
    Ok(GitBranchInfo {
        is_repo: true,
        current,
        branches,
        changed,
        untracked,
        status_text,
    })
}

/// 切换分支（未提交改动冲突时返回友好错误，提示提交或改用 worktree）
#[tauri::command]
pub async fn git_switch_branch(
    project_path: String,
    branch: String,
    state: State<'_, DbState>,
) -> Result<String, String> {
    let branch = branch.trim().to_string();
    if branch.is_empty() {
        return Err("未指定分支".into());
    }
    let cwd = Path::new(&project_path);
    match crate::agent::tools::run_cmd(
        "git",
        &["checkout".into(), branch.clone()],
        Some(cwd),
        60,
    )
    .await
    {
        Ok(o) => {
            if let Ok(conn) = state.0.lock() {
                if let Ok(project_id) = conn.query_row(
                    "SELECT id FROM projects WHERE path=?1 OR worktree_path=?1 LIMIT 1",
                    [&project_path],
                    |row| row.get::<_, String>(0),
                ) {
                    let _ = crate::agent::context::invalidate_project_facts(
                        &conn,
                        &project_id,
                        "git_branch_changed",
                    );
                    let _ = crate::agent::context::invalidate_project_memories(
                        &conn,
                        &project_id,
                        "git_branch_changed",
                        &[],
                    );
                }
            }
            Ok(format!("✅ 已切换到分支 {branch}\n{o}"))
        }
        Err(e) => Err(format!(
            "切换失败：{e}\n提示：若有未提交改动导致切换被拒，请先提交/暂存改动（可让 Agent 执行 git_commit），或使用 worktree 在独立目录操作该分支。"
        )),
    }
}

/// worktree 列表（第一个是主仓库）
#[tauri::command]
pub async fn git_worktree_list(project_path: String) -> Result<Vec<WorktreeInfo>, String> {
    let cwd = Path::new(&project_path);
    let out = crate::agent::tools::run_cmd(
        "git",
        &["worktree".into(), "list".into(), "--porcelain".into()],
        Some(cwd),
        15,
    )
    .await?;
    let mut list: Vec<WorktreeInfo> = Vec::new();
    let mut cur: Option<(String, String)> = None;
    let mut first_done = false;
    for line in out.lines() {
        if let Some(p) = line.strip_prefix("worktree ") {
            if let Some((path, branch)) = cur.take() {
                list.push(WorktreeInfo {
                    path,
                    branch,
                    is_main: !first_done,
                });
                first_done = true;
            }
            cur = Some((p.trim().to_string(), String::new()));
        } else if let Some(b) = line.strip_prefix("branch refs/heads/") {
            if let Some(c) = cur.as_mut() {
                c.1 = b.trim().to_string();
            }
        } else if line.trim().is_empty() {
            if let Some((path, branch)) = cur.take() {
                list.push(WorktreeInfo {
                    path,
                    branch,
                    is_main: !first_done,
                });
                first_done = true;
            }
        }
    }
    if let Some((path, branch)) = cur.take() {
        list.push(WorktreeInfo {
            path,
            branch,
            is_main: !first_done,
        });
    }
    Ok(list)
}

/// 把分支名清洗成安全的目录段：路径分隔符与 Windows 非法字符统一替换为 '-'，
/// 避免 `feature/foo` 这类分支名被当成嵌套路径，导致 worktree 目录错乱。
fn sanitize_dir_segment(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        let ok = !matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
            && !c.is_control();
        out.push(if ok { c } else { '-' });
    }
    let trimmed = out.trim_matches(|c: char| c == '-' || c == ' ');
    if trimmed.is_empty() { "branch".to_string() } else { trimmed.to_string() }
}

/// 创建 worktree：目录放在项目同级 `<项目名>-<分支名>`（分支名经清洗，`/` 等替换为 `-`）。
/// branch 为已有分支名（或起始点）；new_branch 提供时用 -b 从 branch 新建分支。
/// 返回新 worktree 的绝对路径。
#[tauri::command]
pub async fn git_worktree_create(
    project_path: String,
    branch: String,
    new_branch: Option<String>,
) -> Result<String, String> {
    let branch = branch.trim().to_string();
    if branch.is_empty() {
        return Err("请选择或输入分支名".into());
    }
    let project = Path::new(&project_path);
    let name = project
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "project".into());
    let dir_branch = sanitize_dir_segment(&branch);
    let wt_dir = project
        .parent()
        .unwrap_or(project)
        .join(format!("{name}-{dir_branch}"));
    if wt_dir.exists() {
        return Err(format!("worktree 目录已存在：{}", wt_dir.display()));
    }
    let mut args = vec![
        "worktree".into(),
        "add".into(),
        wt_dir.to_string_lossy().to_string(),
    ];
    if let Some(nb) = new_branch.as_deref().map(|s| s.trim()).filter(|s| !s.is_empty()) {
        args.push("-b".into());
        args.push(nb.to_string());
        args.push(branch.clone());
    } else {
        args.push(branch.clone());
    }
    crate::agent::tools::run_cmd("git", &args, Some(project), 120).await?;
    Ok(wt_dir.to_string_lossy().to_string())
}

/// 删除 worktree（目录内有未提交改动时提示先处理）
#[tauri::command]
pub async fn git_worktree_remove(project_path: String, wt_path: String) -> Result<String, String> {
    if wt_path.trim().is_empty() {
        return Err("未指定 worktree 路径".into());
    }
    let cwd = Path::new(&project_path);
    match crate::agent::tools::run_cmd(
        "git",
        &["worktree".into(), "remove".into(), wt_path.clone()],
        Some(cwd),
        60,
    )
    .await
    {
        Ok(o) => Ok(format!("✅ 已删除 worktree：{wt_path}\n{o}")),
        Err(e) => Err(format!(
            "删除失败：{e}\n提示：若 worktree 内有未提交改动，请先在 worktree 中提交或丢弃改动后再删。"
        )),
    }
}

/// 项目绑定 worktree（绑定后 Agent 任务在该 worktree 目录中执行）；传 None 解除绑定
#[tauri::command]
pub fn set_project_worktree(
    project_id: String,
    worktree_path: Option<String>,
    state: State<DbState>,
) -> Result<Project, String> {
    let conn = state.0.lock().map_err(|e| e.to_string())?;
    let wp = worktree_path
        .as_deref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty());
    let previous: Option<String> = conn
        .query_row(
            "SELECT worktree_path FROM projects WHERE id=?1",
            [&project_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE projects SET worktree_path = ?1 WHERE id = ?2",
        params![wp, project_id],
    )
    .map_err(|e| e.to_string())?;
    if previous.as_deref() != wp {
        let _ = crate::agent::context::invalidate_project_facts(
            &conn,
            &project_id,
            "project_worktree_changed",
        );
        let refs = wp.map(str::to_string).into_iter().collect::<Vec<_>>();
        let _ = crate::agent::context::invalidate_project_memories(
            &conn,
            &project_id,
            "project_changed",
            &refs,
        );
    }
    drop(conn);
    crate::commands::project::get_project_by_id(&state, &project_id)
}

/// 将 worktree 分支合并回主仓库当前分支（合并前要求 worktree 内改动已提交）
#[tauri::command]
pub async fn git_worktree_merge(project_path: String, wt_path: String) -> Result<String, String> {
    let cwd = Path::new(&project_path);
    // 1. 取 worktree 的分支名
    let out = crate::agent::tools::run_cmd(
        "git",
        &["worktree".into(), "list".into(), "--porcelain".into()],
        Some(cwd),
        15,
    )
    .await?;
    let mut branch = String::new();
    let mut in_target = false;
    for line in out.lines() {
        if let Some(p) = line.strip_prefix("worktree ") {
            in_target = p.trim().trim_end_matches(['/', '\\']) == wt_path.trim().trim_end_matches(['/', '\\']);
        } else if in_target {
            if let Some(x) = line.strip_prefix("branch refs/heads/") {
                branch = x.trim().to_string();
            }
        }
    }
    if branch.is_empty() {
        return Err("未找到该 worktree 的分支信息".into());
    }
    // 2. 要求 worktree 内改动已提交（合并只包含分支历史）
    let dirty = crate::agent::tools::run_cmd(
        "git",
        &["status".into(), "--porcelain".into()],
        Some(Path::new(&wt_path)),
        15,
    )
    .await?;
    if !dirty.trim().is_empty() {
        return Err(format!(
            "worktree 内还有未提交的改动（{} 项）：\n{dirty}\n请先在 worktree 中提交改动（可让 Agent 执行 git_commit），再执行合并。",
            dirty.lines().count()
        ));
    }
    // 3. 主仓库合并该分支（--no-edit 避免编辑器挂起；冲突时 git 会返回非零退出码）
    let merged = crate::agent::tools::run_cmd(
        "git",
        &["merge".into(), "--no-edit".into(), branch.clone()],
        Some(cwd),
    120,
    )
    .await?;
    Ok(format!(
        "✅ 已将 worktree 分支 {branch} 合并到当前分支：\n{merged}\n\n合并完成后可在 Git 面板删除该 worktree。"
    ))
}

// ---------- 任务回滚（回到任务前的工作区状态） ----------

/// 回滚目标信息（dry_run 预览 / 执行后结果）
#[derive(Debug, Clone, Serialize)]
pub struct RollbackInfo {
    /// 回滚目标提交哈希（无提交可回滚时为空）
    pub commit: String,
    /// 目标提交时间（格式 YYYY-MM-DD HH:MM）
    pub commit_date: String,
    /// 已跟踪改动条数（回滚将被丢弃）
    pub changed: usize,
    /// 未跟踪文件条数（回滚不会删除，保留）
    pub untracked: usize,
    /// 项目是否为 git 仓库（false = 不支持回滚）
    pub is_repo: bool,
}

/// 计算会话任务起点并回滚到该点：起点 = 会话第一条 user 消息时间之前最后一次提交。
/// dry_run=true 仅返回预览（不修改工作区）；false 执行 `git reset --hard` 回滚。
/// 未跟踪文件保留（不执行 clean，防误删）；worktree 绑定项目在绑定目录内执行。
#[tauri::command]
pub async fn rollback_conversation(
    conversation_id: String,
    dry_run: bool,
    state: State<'_, DbState>,
) -> Result<RollbackInfo, String> {
    // 1. 会话 → 项目路径（worktree 优先）
    let project_path = {
        let conn = state.0.lock().map_err(|e| e.to_string())?;
        let row = conn
            .query_row(
                "SELECT p.path, c.worktree_path FROM conversations c JOIN projects p ON p.id = c.project_id
                 WHERE c.id = ?1",
                [&conversation_id],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)),
            )
            .map_err(|e| e.to_string())?;
        match row.1 {
            Some(w) if !w.trim().is_empty() && Path::new(&w).is_dir() => w.trim().to_string(),
            _ => row.0,
        }
    };
    if project_path.trim().is_empty() {
        return Err("项目未绑定目录，无法回滚".into());
    }
    let cwd = Path::new(&project_path);
    // 2. 任务起点：会话第一条 user 消息时间戳
    let start_ts = {
        let conn = state.0.lock().map_err(|e| e.to_string())?;
        conn.query_row(
            "SELECT MIN(created_at) FROM messages WHERE conversation_id = ?1 AND role = 'user' AND queued = 0 AND hidden = 0",
            [&conversation_id],
            |r| r.get::<_, Option<i64>>(0),
        )
        .map_err(|e| e.to_string())?
        .ok_or("会话还没有用户消息，无法回滚")?
    };
    // 3. 非 git 仓库：不支持回滚
    if crate::agent::tools::run_cmd(
        "git",
        &["rev-parse".into(), "--is-inside-work-tree".into()],
        Some(cwd),
        15,
    )
    .await
    .is_err()
    {
        return Ok(RollbackInfo {
            commit: String::new(),
            commit_date: String::new(),
            changed: 0,
            untracked: 0,
            is_repo: false,
        });
    }
    // 4. 起点之前最后一次提交（无提交则不可回滚）；@ 前缀表示 unix 时间戳（裸数字会被 git 当作相对时间）
    let commit = crate::agent::tools::run_cmd(
        "git",
        &["log".into(), format!("--before=@{start_ts}"), "--format=%H".into(), "-1".into()],
        Some(cwd),
        15,
    )
    .await?
    .trim()
    .to_string();
    if commit.is_empty() {
        return Err("任务起点之前没有提交记录，无法回滚（任务开始前工作区处于未提交状态）".into());
    }
    let commit_date = crate::agent::tools::run_cmd(
        "git",
        &["log".into(), "-1".into(), "--format=%ad".into(), "--date=format:%Y-%m-%d %H:%M".into(), commit.clone()],
        Some(cwd),
        15,
    )
    .await
    .unwrap_or_default()
    .trim()
    .to_string();
    // 5. 当前改动统计（预览用）
    let status = crate::agent::tools::run_cmd(
        "git",
        &["status".into(), "--short".into()],
        Some(cwd),
        15,
    )
    .await
    .unwrap_or_default();
    let mut changed = 0usize;
    let mut untracked = 0usize;
    for line in status.lines() {
        if line.trim_start().starts_with("??") {
            untracked += 1;
        } else {
            changed += 1;
        }
    }
    if dry_run {
        return Ok(RollbackInfo {
            commit,
            commit_date,
            changed,
            untracked,
            is_repo: true,
        });
    }
    // 6. 执行回滚（硬重置到起点提交；未跟踪文件保留，不 clean 防误删）
    crate::agent::tools::run_cmd(
        "git",
        &["reset".into(), "--hard".into(), commit.clone()],
        Some(cwd),
        60,
    )
    .await
    .map_err(|e| format!("回滚失败: {e}"))?;
    Ok(RollbackInfo {
        commit,
        commit_date,
        changed,
        untracked,
        is_repo: true,
    })
}

/// 单文件工作区 diff（变更审查用；file 为相对项目根路径，禁止越权路径；绝对路径若位于项目内自动转相对）
#[tauri::command]
pub async fn git_file_diff(project_path: String, file: String) -> Result<String, String> {
    if project_path.trim().is_empty() {
        return Err("未指定项目目录".into());
    }
    let cwd = Path::new(&project_path);
    // 路径归一化：绝对路径若位于项目内则转相对；含 .. 的路径先 canonicalize 防止越权
    let file = resolve_repo_path(cwd, &file)?;
    // 工作区+暂存 vs HEAD；无提交时 HEAD 不可用则退回工作区 diff
    let diff = match crate::agent::tools::run_cmd(
        "git",
        &["diff".into(), "HEAD".into(), "--".into(), file.clone()],
        Some(cwd),
        15,
    )
    .await
    {
        Ok(d) if !d.trim().is_empty() => d,
        _ => crate::agent::tools::run_cmd(
            "git",
            &["diff".into(), "--".into(), file.clone()],
            Some(cwd),
            15,
        )
        .await
        .unwrap_or_default(),
    };
    if diff.trim().is_empty() {
        // 未跟踪新文件：读前 40 行作为内容预览
        let full = cwd.join(&file);
        if full.is_file() {
            let head = std::fs::read_to_string(&full)
                .map(|s| s.lines().take(40).collect::<Vec<_>>().join("\n"))
                .unwrap_or_default();
            return Ok(format!("(新文件，无 diff)\n\n{head}"));
        }
        return Ok("(无变更)".into());
    }
    Ok(diff)
}

/// 将传入的文件路径解析为相对项目根的安全路径：
/// - 绝对路径若位于项目内 → 转相对
/// - 含 `..` 的路径 → canonicalize 后再比较，越权（项目外）拒绝
/// - 普通相对路径直接使用（但拒绝绝对路径且不在项目内的情况）
fn resolve_repo_path(cwd: &Path, file: &str) -> Result<String, String> {
    if file.trim().is_empty() {
        return Err("文件路径为空".into());
    }
    let p = Path::new(file);
    if p.is_absolute() {
        let canon_cwd = std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
        let canon_p = std::fs::canonicalize(p).map_err(|_| "文件路径不合法".to_string())?;
        let rel = canon_p
            .strip_prefix(&canon_cwd)
            .map_err(|_| "文件路径不合法".to_string())?;
        return Ok(rel.to_string_lossy().replace('\\', "/"));
    }
    // 相对路径：含 .. 时 canonicalize 校验越权
    if file.contains("..") {
        let canon_cwd = std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
        let joined = cwd.join(p);
        let canon_p = std::fs::canonicalize(&joined).map_err(|_| "文件路径不合法".to_string())?;
        let rel = canon_p
            .strip_prefix(&canon_cwd)
            .map_err(|_| "文件路径不合法".to_string())?;
        return Ok(rel.to_string_lossy().replace('\\', "/"));
    }
    Ok(file.replace('\\', "/"))
}

/// 接受变更：git add 指定文件（相对路径列表）
#[tauri::command]
pub async fn git_accept_changes(project_path: String, files: Vec<String>) -> Result<usize, String> {
    if project_path.trim().is_empty() {
        return Err("未指定项目目录".into());
    }
    let files: Vec<String> = files
        .into_iter()
        .filter(|f| !f.trim().is_empty() && !f.contains(".."))
        .collect();
    if files.is_empty() {
        return Ok(0);
    }
    let mut args = vec!["add".to_string(), "--".to_string()];
    args.extend(files.iter().cloned());
    crate::agent::tools::run_cmd("git", &args, Some(Path::new(&project_path)), 30)
        .await
        .map_err(|e| format!("接受变更失败: {e}"))?;
    Ok(files.len())
}

/// 还原变更：已跟踪文件 git checkout 丢弃改动；未跟踪新文件不删除（防误删）
#[tauri::command]
pub async fn git_revert_file(project_path: String, file: String) -> Result<String, String> {
    if project_path.trim().is_empty() {
        return Err("未指定项目目录".into());
    }
    let rel = Path::new(&file);
    if rel.is_absolute() || file.contains("..") {
        return Err("文件路径不合法".into());
    }
    let cwd = Path::new(&project_path);
    if crate::agent::tools::run_cmd(
        "git",
        &["ls-files".into(), "--error-unmatch".into(), "--".into(), file.clone()],
        Some(cwd),
        15,
    )
    .await
    .is_err()
    {
        return Err("未跟踪的新文件不自动删除（可在文件树中手动删除）".into());
    }
    crate::agent::tools::run_cmd(
        "git",
        &["checkout".into(), "--".into(), file.clone()],
        Some(cwd),
        30,
    )
    .await
    .map_err(|e| format!("还原失败: {e}"))?;
    Ok(file)
}

/// 文件变更统计（ChatGPT 式 +N/-M）：对给定文件列表跑 git diff --numstat HEAD，
/// 累加增删行数；未跟踪文件（不在 diff 中）只计数量不计行数
#[derive(serde::Serialize)]
pub struct DiffStat {
    pub files: usize,
    pub insertions: usize,
    pub deletions: usize,
}

#[tauri::command]
pub async fn git_diff_stat(project_path: String, files: Vec<String>) -> Result<DiffStat, String> {
    if project_path.trim().is_empty() {
        return Err("未指定项目目录".into());
    }
    for f in &files {
        let rel = Path::new(f);
        if rel.is_absolute() || f.contains("..") {
            return Err("文件路径不合法".into());
        }
    }
    let cwd = Path::new(&project_path);
    let mut args = vec!["diff".to_string(), "--numstat".to_string(), "HEAD".to_string(), "--".to_string()];
    args.extend(files.iter().cloned());
    // HEAD 缺失（无提交）/失败时回退到工作区 diff
    let out = match crate::agent::tools::run_cmd("git", &args, Some(cwd), 15).await {
        Ok(d) if !d.trim().is_empty() => d,
        _ => {
            let mut ws = vec!["diff".to_string(), "--numstat".to_string(), "--".to_string()];
            ws.extend(files.iter().cloned());
            crate::agent::tools::run_cmd("git", &ws, Some(cwd), 15)
                .await
                .unwrap_or_default()
        }
    };
    let mut insertions = 0usize;
    let mut deletions = 0usize;
    for line in out.lines() {
        let mut parts = line.split_whitespace();
        if let (Some(a), Some(d)) = (parts.next(), parts.next()) {
            if let (Ok(a), Ok(d)) = (a.parse::<usize>(), d.parse::<usize>()) {
                insertions += a;
                deletions += d;
            }
        }
    }
    Ok(DiffStat {
        files: files.len(),
        insertions,
        deletions,
    })
}

// ---------- 仓库初始化 / 文件级 Git 操作（文件树右键菜单） ----------

/// 初始化 Git 仓库（git init）：当前目录已在仓库内时直接返回提示，不重复初始化。
/// 无 .gitignore 时生成最小默认规则，避免 node_modules 等依赖/产物目录被误跟踪。
#[tauri::command]
pub async fn git_init_repo(project_path: String) -> Result<String, String> {
    if project_path.trim().is_empty() {
        return Err("未指定项目目录".into());
    }
    let cwd = Path::new(&project_path);
    if crate::agent::tools::run_cmd(
        "git",
        &["rev-parse".into(), "--is-inside-work-tree".into()],
        Some(cwd),
        15,
    )
    .await
    .is_ok()
    {
        return Ok("该项目已经是 Git 仓库".into());
    }
    let out = crate::agent::tools::run_cmd("git", &["init".into()], Some(cwd), 30)
        .await
        .map_err(|e| format!("初始化 Git 仓库失败：{e}"))?;
    // 常见工程补充默认 .gitignore（仅当不存在时生成，不覆盖已有规则）
    let gitignore = cwd.join(".gitignore");
    if !gitignore.exists() {
        let default = "# 依赖与构建产物\nnode_modules/\noh_modules/\ndist/\nbuild/\n*.log\n";
        if std::fs::write(&gitignore, default).is_ok() {
            return Ok(format!("{out}\n已生成默认 .gitignore"));
        }
    }
    Ok(out)
}

/// 单文件/目录的 Git 状态（文件树右键菜单用；非仓库 is_repo=false，不报错）
#[derive(Debug, Clone, Serialize)]
pub struct GitFileStatus {
    pub is_repo: bool,
    /// 是否被 .gitignore 忽略（check-ignore 命中，含父目录规则）
    pub ignored: bool,
    /// 是否已被 git 跟踪（ls-files 命中；目录含任一已跟踪文件即 true）
    pub tracked: bool,
    /// 状态摘要：none | clean | ignored | untracked | modified | staged | deleted
    pub status: String,
}

#[tauri::command]
pub async fn git_file_status(project_path: String, path: String) -> Result<GitFileStatus, String> {
    if project_path.trim().is_empty() {
        return Err("未指定项目目录".into());
    }
    let cwd = Path::new(&project_path);
    let rel = resolve_repo_path(cwd, &path)?;
    // 非仓库：返回 none，不报错（前端据此隐藏 git 操作分组）
    if crate::agent::tools::run_cmd(
        "git",
        &["rev-parse".into(), "--is-inside-work-tree".into()],
        Some(cwd),
        15,
    )
    .await
    .is_err()
    {
        return Ok(GitFileStatus {
            is_repo: false,
            ignored: false,
            tracked: false,
            status: "none".into(),
        });
    }
    // 忽略判断：check-ignore 命中即忽略（含父目录规则，文件/目录均可用）
    let ignored = crate::agent::tools::run_cmd(
        "git",
        &["check-ignore".into(), "-q".into(), "--".into(), rel.clone()],
        Some(cwd),
        15,
    )
    .await
    .is_ok();
    // 跟踪判断：ls-files 列出匹配文件（目录会列出其下已跟踪文件），输出非空即已跟踪
    let tracked = crate::agent::tools::run_cmd(
        "git",
        &["ls-files".into(), "--".into(), rel.clone()],
        Some(cwd),
        15,
    )
    .await
    .map(|o| !o.trim().is_empty())
    .unwrap_or(false);
    // 状态判断：git status --short -- path（目录为聚合：全未跟踪→untracked，含暂存→staged，含删除→deleted，否则 modified）
    let mut status = "clean".to_string();
    if let Ok(s) = crate::agent::tools::run_cmd(
        "git",
        &["status".into(), "--short".into(), "--".into(), rel.clone()],
        Some(cwd),
        15,
    )
    .await
    {
        let lines: Vec<&str> = s.lines().filter(|l| !l.trim().is_empty()).collect();
        if !lines.is_empty() {
            let all_untracked = lines.iter().all(|l| l.trim_start().starts_with("??"));
            let any_staged = lines.iter().any(|l| {
                let b = l.as_bytes();
                b.len() > 1 && !matches!(b[0], b' ' | b'?' | b'!')
            });
            let any_deleted = lines.iter().any(|l| {
                let b = l.as_bytes();
                b.len() > 1 && (b[0] == b'D' || (b[0] == b' ' && b[1] == b'D'))
            });
            status = if all_untracked {
                "untracked".into()
            } else if any_staged {
                "staged".into()
            } else if any_deleted {
                "deleted".into()
            } else {
                "modified".into()
            };
        }
    }
    if ignored && status == "clean" {
        status = "ignored".into();
    }
    Ok(GitFileStatus {
        is_repo: true,
        ignored,
        tracked,
        status,
    })
}

/// 文件树 Git 状态批量查询（懒加载树着色用，一次调用返回全量变更集）：
/// - entries：git status --porcelain 全量变更（文件级），外加按目录前缀聚合的目录条目；
/// - ignored：paths 参数（已加载节点）中命中 .gitignore 的路径，供灰色标识；
/// - 路径统一相对 root（worktree 根，未传时相对项目根），与文件树节点路径对齐。
#[derive(Debug, Clone, Serialize)]
pub struct GitTreeEntry {
    pub path: String,
    /// untracked | modified | staged | deleted
    pub status: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct GitTreeStatusBundle {
    pub is_repo: bool,
    pub entries: Vec<GitTreeEntry>,
    pub ignored: Vec<String>,
}

/// 目录聚合严重度：未跟踪最醒目，其次已删除，再次已修改，最后已暂存
fn status_severity(s: &str) -> u32 {
    match s {
        "untracked" => 4,
        "deleted" => 3,
        "modified" => 2,
        _ => 1, // staged
    }
}

/// 把 git 输出路径（相对 cwd=项目根）转成相对 root 的路径（root 为项目根时不转换）
fn strip_root_prefix(path: &str, root_rel: &Option<String>) -> String {
    if let Some(rel) = root_rel {
        if let Some(rest) = path.strip_prefix(&format!("{rel}/")) {
            return rest.to_string();
        }
        if path == rel {
            return String::new();
        }
    }
    path.to_string()
}

/// 给 check-ignore 传参用的路径：相对 root → 相对项目根（root 为项目根时不转换）
fn add_root_prefix(path: &str, root_rel: &Option<String>) -> String {
    match root_rel {
        Some(rel) if !rel.is_empty() => format!("{rel}/{path}"),
        _ => path.to_string(),
    }
}

#[tauri::command]
pub async fn get_file_tree_git_status(
    project_path: String,
    root: Option<String>,
    paths: Vec<String>,
) -> Result<GitTreeStatusBundle, String> {
    if project_path.trim().is_empty() {
        return Err("未指定项目目录".into());
    }
    let cwd = Path::new(&project_path);
    // 非仓库：返回空 bundle，不报错（前端据此关闭着色）
    if crate::agent::tools::run_cmd(
        "git",
        &["rev-parse".into(), "--is-inside-work-tree".into()],
        Some(cwd),
        15,
    )
    .await
    .is_err()
    {
        return Ok(GitTreeStatusBundle {
            is_repo: false,
            entries: vec![],
            ignored: vec![],
        });
    }

    // root（worktree 根）相对项目根的路径；root 必须位于项目内（防御越界）
    let root_rel: Option<String> = root.filter(|r| !r.is_empty()).and_then(|r| {
        let abs = if Path::new(&r).is_absolute() {
            r.clone()
        } else {
            format!("{}/{}", project_path.trim_end_matches(['/', '\\']), r.trim_start_matches(['/', '\\']))
        };
        let canon_root = cwd.canonicalize().ok()?;
        let canon_target = Path::new(&abs).canonicalize().ok()?;
        if canon_target == canon_root {
            return None;
        }
        canon_target
            .strip_prefix(&canon_root)
            .ok()
            .map(|p| p.to_string_lossy().replace('\\', "/"))
    });

    // 全量变更集（-z 分隔防路径含空格/特殊字符；输出上限 8MB 覆盖大仓库）
    let mut status_args = vec![
        "status".into(),
        "--porcelain".into(),
        "-z".into(),
        "--untracked-files=all".into(),
    ];
    if let Some(rel) = &root_rel {
        status_args.push("--".into());
        status_args.push(rel.clone());
    }
    let status_out = crate::agent::tools::run_cmd_capped(
        "git",
        &status_args,
        Some(cwd),
        15,
        8_000_000,
    )
    .await
    .unwrap_or_default();

    // 文件级条目 + 目录前缀聚合（同一目录多个子项取最高严重度）
    let mut entries: Vec<GitTreeEntry> = Vec::new();
    let mut dir_agg: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
    let mut seen_files: std::collections::HashSet<String> = std::collections::HashSet::new();
    let fields: Vec<&str> = status_out.split('\0').collect();
    let mut i = 0usize;
    while i < fields.len() {
        let field = fields[i];
        i += 1;
        if field.is_empty() || field.len() < 4 {
            continue;
        }
        let b = field.as_bytes();
        let x = b[0] as char;
        let y = b[1] as char;
        let mut path = field[3..].to_string();
        // 重命名/复制：-z 格式下旧名与新名分属两个 \0 字段，取新名
        if (x == 'R' || x == 'C') && path.is_empty() && i < fields.len() {
            path = fields[i].to_string();
            i += 1;
        }
        let status = if x == '?' && y == '?' {
            "untracked"
        } else if x != ' ' && x != '?' && x != '!' {
            "staged" // 暂存区有改动（A/M/R/C/D）
        } else if y == 'M' || y == 'T' || y == 'U' {
            "modified"
        } else if y == 'D' {
            "deleted"
        } else {
            "modified"
        };
        let rel_path = strip_root_prefix(&path, &root_rel);
        if rel_path.is_empty() {
            continue;
        }
        if !seen_files.contains(&rel_path) {
            seen_files.insert(rel_path.clone());
            entries.push(GitTreeEntry {
                path: rel_path.clone(),
                status: status.to_string(),
            });
        }
        // 目录前缀聚合（文件自身最后一段不参与）
        let sev = status_severity(status);
        let mut prefix = String::new();
        for part in rel_path.split('/') {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(part);
            if prefix == rel_path {
                break;
            }
            let e = dir_agg.entry(prefix.clone()).or_insert(0);
            *e = (*e).max(sev);
        }
    }
    let sev_name = ["", "staged", "modified", "deleted", "untracked"];
    for (dir, sev) in dir_agg {
        if sev > 0 && !seen_files.contains(&dir) {
            entries.push(GitTreeEntry {
                path: dir,
                status: sev_name[sev as usize].to_string(),
            });
        }
    }

    // 忽略判定：对已加载节点中「不在变更集」的路径，先排除已跟踪（ls-files），
    // 剩余用 check-ignore 批量判定（已跟踪文件不受 gitignore 影响，不能标灰）
    let mut ignored: Vec<String> = Vec::new();
    let candidates: Vec<String> = paths
        .iter()
        .filter(|p| !p.is_empty() && !seen_files.contains(p.as_str()))
        .cloned()
        .collect();
    if !candidates.is_empty() {
        let mut ls_args = vec!["ls-files".into(), "-z".into()];
        if let Some(rel) = &root_rel {
            ls_args.push("--".into());
            ls_args.push(rel.clone());
        }
        let tracked: std::collections::HashSet<String> = crate::agent::tools::run_cmd_capped(
            "git",
            &ls_args,
            Some(cwd),
            15,
            8_000_000,
        )
        .await
        .unwrap_or_default()
        .split('\0')
        .filter(|s| !s.is_empty())
        .map(|s| strip_root_prefix(s, &root_rel))
        .collect();
        let probe: Vec<String> = candidates
            .iter()
            .filter(|p| !tracked.contains(p.as_str()))
            .map(|p| add_root_prefix(p, &root_rel))
            .collect();
        if !probe.is_empty() {
            let mut ci_args = vec!["check-ignore".into(), "-z".into(), "--".into()];
            ci_args.extend(probe.iter().cloned());
            if let Ok(out) = crate::agent::tools::run_cmd_capped(
                "git",
                &ci_args,
                Some(cwd),
                15,
                8_000_000,
            )
            .await
            {
                ignored = out
                    .split('\0')
                    .filter(|s| !s.is_empty())
                    .map(|s| strip_root_prefix(s, &root_rel))
                    .collect();
            }
        }
    }

    Ok(GitTreeStatusBundle {
        is_repo: true,
        entries,
        ignored,
    })
}

/// 把路径追加到项目根 .gitignore（目录自动补尾部 /；已存在相同规则时跳过）
#[tauri::command]
pub async fn git_ignore_add(project_path: String, path: String) -> Result<String, String> {
    if project_path.trim().is_empty() {
        return Err("未指定项目目录".into());
    }
    let cwd = Path::new(&project_path);
    let rel = resolve_repo_path(cwd, &path)?;
    if rel.is_empty() {
        return Err("文件路径为空".into());
    }
    let is_dir = cwd.join(&rel).is_dir();
    let rule = if is_dir { format!("{rel}/") } else { rel.clone() };
    let gitignore = cwd.join(".gitignore");
    let existing = std::fs::read_to_string(&gitignore).unwrap_or_default();
    if existing.lines().any(|l| l.trim() == rule) {
        return Ok(format!("{rule} 已在 .gitignore 中"));
    }
    // 追加：文件不存在/结尾无换行时先补换行，保证规则各自独立成行
    let mut content = existing;
    if !content.is_empty() && !content.ends_with('\n') {
        content.push('\n');
    }
    content.push_str(&rule);
    content.push('\n');
    std::fs::write(&gitignore, content).map_err(|e| format!("写入 .gitignore 失败：{e}"))?;
    Ok(format!("已将 {rule} 加入 .gitignore"))
}

/// 从 Git 索引移除（git rm --cached）：工作区文件保留，配合 .gitignore 可彻底停止跟踪。
/// 需已跟踪；仓库尚无提交时 git 会拒绝，给出友好提示。
#[tauri::command]
pub async fn git_untrack(project_path: String, path: String) -> Result<String, String> {
    if project_path.trim().is_empty() {
        return Err("未指定项目目录".into());
    }
    let cwd = Path::new(&project_path);
    let rel = resolve_repo_path(cwd, &path)?;
    let tracked = crate::agent::tools::run_cmd(
        "git",
        &["ls-files".into(), "--".into(), rel.clone()],
        Some(cwd),
        15,
    )
    .await
    .map(|o| !o.trim().is_empty())
    .unwrap_or(false);
    if !tracked {
        return Err("该路径未被 Git 跟踪，无需排除".into());
    }
    let mut args = vec!["rm".into(), "--cached".into()];
    if cwd.join(&rel).is_dir() {
        args.push("-r".into());
    }
    args.push("--".into());
    args.push(rel.clone());
    crate::agent::tools::run_cmd("git", &args, Some(cwd), 30).await.map_err(|e| {
        format!("从 Git 历史排除失败：{e}\n提示：若仓库还没有任何提交，请先提交一次后再排除。")
    })?;
    Ok(format!("已从 Git 历史排除 {rel}（工作区文件保留；建议同时加入 .gitignore 防止再次跟踪）"))
}

/// 暂存改动（git add）：被 .gitignore 忽略的路径直接拒绝并提示，避免误暂存
#[tauri::command]
pub async fn git_stage(project_path: String, path: String) -> Result<String, String> {
    if project_path.trim().is_empty() {
        return Err("未指定项目目录".into());
    }
    let cwd = Path::new(&project_path);
    let rel = resolve_repo_path(cwd, &path)?;
    if !cwd.join(&rel).exists() {
        return Err("路径不存在，无法暂存".into());
    }
    if crate::agent::tools::run_cmd(
        "git",
        &["check-ignore".into(), "-q".into(), "--".into(), rel.clone()],
        Some(cwd),
        15,
    )
    .await
    .is_ok()
    {
        return Err("该路径已被 .gitignore 忽略，无法直接暂存（如需强制暂存可在终端执行 git add -f）".into());
    }
    crate::agent::tools::run_cmd(
        "git",
        &["add".into(), "--".into(), rel.clone()],
        Some(cwd),
        30,
    )
    .await
    .map_err(|e| format!("暂存失败：{e}"))?;
    Ok(format!("已暂存 {rel}"))
}

/// 单文件/目录的提交历史（最近 15 条，一行一条）
#[tauri::command]
pub async fn git_file_log(project_path: String, path: String) -> Result<String, String> {
    if project_path.trim().is_empty() {
        return Err("未指定项目目录".into());
    }
    let cwd = Path::new(&project_path);
    let rel = resolve_repo_path(cwd, &path)?;
    let out = crate::agent::tools::run_cmd(
        "git",
        &["log".into(), "--oneline".into(), "-n".into(), "15".into(), "--".into(), rel.clone()],
        Some(cwd),
        15,
    )
    .await
    .map_err(|e| format!("读取历史失败：{e}"))?;
    if out.trim().is_empty() {
        return Err("该路径还没有提交历史".into());
    }
    Ok(out)
}
