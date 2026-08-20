//! Git 域工具：状态 / diff / 提交 / 日志 / 分支 / 回滚 / 推送 / stash / 标签 / 变更评审。
//! 共享辅助函数在父模块 mod.rs，通过 `use super::*` 继承。

use super::*;

pub(super) async fn git_stash(args: &Value, roots: &[String]) -> Result<String, String> {
    let project_path = roots.first().map(String::as_str).unwrap_or("");
    if project_path.is_empty() {
        return Err("当前会话未绑定项目目录，无法执行 git stash".into());
    }
    // 并发护栏：与 git_commit/构建互斥（stash 会清理工作区，不能与提交/构建并发）
    let _gate = crate::services::tool_limits::acquire_workspace_gate(Path::new(project_path)).await;
    let action = args["action"].as_str().unwrap_or("push");
    let cmd_args: Vec<String> = match action {
        "list" => vec!["stash".to_string(), "list".to_string()],
        "pop" => vec!["stash".to_string(), "pop".to_string()],
        "push" => {
            let mut v = vec!["stash".to_string(), "push".to_string(), "-u".to_string()];
            if let Some(m) = args["message"].as_str() {
                v.push("-m".to_string());
                v.push(m.to_string());
            }
            v
        }
        other => return Err(format!("未知 stash action: {other}（支持 push/pop/list）")),
    };
    run_in_project(project_path, "git", &cmd_args, 60)
        .await
        .map_err(|e| with_advice("git_stash", e))
}

pub(super) async fn git_status(roots: &[String]) -> Result<String, String> {
    let project_path = roots.first().map(String::as_str).unwrap_or("");
    if project_path.is_empty() {
        return Err("当前会话未绑定项目目录".into());
    }
    let cwd = Path::new(project_path);
    let branch = match run_cmd("git", &["branch".into(), "--show-current".into()], Some(cwd), 15).await {
        Ok(b) => b.trim().to_string(),
        Err(e) => return Err(with_advice("git_status", e)),
    };
    let branch = if branch.is_empty() { "(新仓库，尚无提交)".to_string() } else { branch };
    let status = match run_cmd("git", &["status".into(), "--short".into()], Some(cwd), 15).await {
        Ok(s) => s,
        Err(e) => return Err(with_advice("git_status", e)),
    };
    let status = if status.trim().is_empty() {
        "（工作区干净，无改动）".to_string()
    } else {
        status
    };
    Ok(format!("分支: {branch}\n\n改动: \n{status}"))
}

pub(super) async fn git_diff(args: &Value, roots: &[String]) -> Result<String, String> {
    let project_path = roots.first().map(String::as_str).unwrap_or("");
    if project_path.is_empty() {
        return Err("当前会话未绑定项目目录".into());
    }
    let path = args["path"].as_str().unwrap_or("");
    let mut cargs = vec!["diff".to_string()];
    if path == "--staged" {
        cargs.push("--staged".into());
    } else if !path.is_empty() {
        cargs.push("--".into());
        cargs.push(path.to_string());
    }
    run_cmd("git", &cargs, Some(Path::new(project_path)), 30)
        .await
        .map_err(|e| with_advice("git_diff", e))
}

pub(super) async fn git_commit(args: &Value, roots: &[String]) -> Result<String, String> {
    let project_path = roots.first().map(String::as_str).unwrap_or("");
    if project_path.is_empty() {
        return Err("当前会话未绑定项目目录".into());
    }
    let message = args["message"].as_str().unwrap_or("").trim();
    if message.is_empty() {
        return Err("git_commit 需要参数 {\"message\":\"<提交信息>\"}".into());
    }
    let cwd = Path::new(project_path);
    // 并发护栏：提交与构建互斥（git add -A 会暂存 build 产物变化）
    let _gate = crate::services::tool_limits::acquire_workspace_gate(cwd).await;
    let add = run_cmd("git", &["add".into(), "-A".into()], Some(cwd), 30)
        .await
        .map_err(|e| with_advice("git_commit", e))?;
    let commit = run_cmd("git", &["commit".into(), "-m".into(), message.to_string()], Some(cwd), 30)
        .await
        .map_err(|e| with_advice("git_commit", e))?;
    Ok(format!("暂存全部改动：\n{add}\n\n提交：\n{commit}"))
}

pub(super) async fn git_log(args: &Value, roots: &[String]) -> Result<String, String> {
    let project_path = roots.first().map(String::as_str).unwrap_or("");
    if project_path.is_empty() {
        return Err("当前会话未绑定项目目录".into());
    }
    let n = args["n"].as_u64().unwrap_or(20).clamp(1, 100) as usize;
    let path = args["path"].as_str().unwrap_or("").trim();
    let grep = args["grep"].as_str().unwrap_or("").trim();
    let mut cargs = vec![
        "log".to_string(),
        format!("-{n}"),
        "--date=short".into(),
        "--pretty=format:%h | %ad | %an | %s".into(),
    ];
    if !grep.is_empty() {
        cargs.extend(["--grep".into(), grep.to_string()]);
    }
    if !path.is_empty() {
        cargs.push("--".into());
        cargs.push(path.to_string());
    }
    let out = run_cmd("git", &cargs, Some(Path::new(project_path)), 15)
        .await
        .map_err(|e| with_advice("git_log", e))?;
    if out.trim().is_empty() {
        return Ok("（无匹配的提交记录）".into());
    }
    Ok(out)
}

pub(super) async fn git_restore(args: &Value, roots: &[String]) -> Result<String, String> {
    let project_path = roots.first().map(String::as_str).unwrap_or("");
    if project_path.is_empty() {
        return Err("当前会话未绑定项目目录".into());
    }
    let path = args["path"].as_str().unwrap_or("").trim();
    let staged = args["staged"].as_bool().unwrap_or(false);
    let mut cargs = vec!["restore".to_string()];
    if staged {
        cargs.push("--staged".into());
        cargs.push("--worktree".into());
    }
    if path.is_empty() {
        cargs.push(".".into());
    } else {
        cargs.push(path.to_string());
    }
    let _gate = crate::services::tool_limits::acquire_workspace_gate(Path::new(project_path)).await;
    run_cmd("git", &cargs, Some(Path::new(project_path)), 30)
        .await
        .map_err(|e| with_advice("git_restore", e))
        .map(|out| {
            if out.trim().is_empty() {
                "已恢复：工作区与最近提交一致（无待恢复改动）".to_string()
            } else {
                out
            }
        })
}

pub(super) async fn git_branch(args: &Value, roots: &[String]) -> Result<String, String> {
    let project_path = roots.first().map(String::as_str).unwrap_or("");
    if project_path.is_empty() {
        return Err("当前会话未绑定项目目录".into());
    }
    let action = args["action"].as_str().unwrap_or("list");
    let name = args["name"].as_str().unwrap_or("").trim();
    let cwd = Path::new(project_path);
    match action {
        "create" => {
            if name.is_empty() {
                return Err("git_branch create 需要参数 {\"name\":\"<分支名>\"}".into());
            }
            let out = run_cmd("git", &["checkout".into(), "-b".into(), name.to_string()], Some(cwd), 30)
                .await
                .map_err(|e| with_advice("git_branch", e))?;
            Ok(format!("已创建并切换到分支 {name}\n{out}"))
        }
        "switch" => {
            if name.is_empty() {
                return Err("git_branch switch 需要参数 {\"name\":\"<分支名>\"}".into());
            }
            let out = run_cmd("git", &["checkout".into(), name.to_string()], Some(cwd), 30)
                .await
                .map_err(|e| with_advice("git_branch", e))?;
            Ok(format!("已切换到分支 {name}\n{out}"))
        }
        _ => {
            let out = run_cmd("git", &["branch".into(), "--list".into()], Some(cwd), 15)
                .await
                .map_err(|e| with_advice("git_branch", e))?;
            if out.trim().is_empty() {
                return Ok("（新仓库，尚无分支）".into());
            }
            Ok(out)
        }
    }
}

pub(super) async fn git_blame(args: &Value, roots: &[String]) -> Result<String, String> {
    let project_path = roots.first().map(String::as_str).unwrap_or("");
    if project_path.is_empty() {
        return Err("当前会话未绑定项目目录".into());
    }
    let path = args["path"].as_str().ok_or("git_blame 需要参数 {\"path\":\"<文件路径>\"}")?.trim();
    if path.is_empty() {
        return Err("git_blame 需要参数 {\"path\":\"<文件路径>\"}".into());
    }
    let start = args["start"].as_u64().unwrap_or(0).max(1);
    let lines = args["lines"].as_u64().unwrap_or(0);
    let cargs = vec![
        "blame".to_string(),
        "--date=short".into(),
        "-L".into(),
        if lines > 0 {
            format!("{start},{}", start + lines - 1)
        } else {
            format!("{start},")
        },
        path.to_string(),
    ];
    let out = run_cmd("git", &cargs, Some(Path::new(project_path)), 30)
        .await
        .map_err(|e| with_advice("git_blame", e))?;
    Ok(super::cmd_tools::cut_str(&out, 6000))
}

pub(super) async fn git_fetch(args: &Value, roots: &[String]) -> Result<String, String> {
    let project_path = roots.first().map(String::as_str).unwrap_or("");
    if project_path.is_empty() {
        return Err("当前会话未绑定项目目录".into());
    }
    let remote = args["remote"].as_str().unwrap_or("origin").trim().to_string();
    let branch = args["branch"].as_str().unwrap_or("").trim();
    let mut cargs = vec!["fetch".to_string(), "--prune".into(), remote.clone()];
    if !branch.is_empty() {
        cargs.push(branch.to_string());
    }
    let out = run_cmd("git", &cargs, Some(Path::new(project_path)), 60)
        .await
        .map_err(|e| with_advice("git_fetch", e))?;
    // fetch 输出含大量引用更新行，压缩为摘要
    let mut summary = String::new();
    for line in out.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if line.contains("->") || line.contains("* [new") || line.contains("deleted") {
            summary.push_str(&format!("  {line}\n"));
        }
    }
    if summary.trim().is_empty() {
        return Ok(format!("已同步远端 {remote} 的引用（无新变化）"));
    }
    Ok(format!("已同步远端 {remote}：\n{summary}"))
}

pub(super) async fn git_pull(args: &Value, roots: &[String]) -> Result<String, String> {
    let project_path = roots.first().map(String::as_str).unwrap_or("");
    if project_path.is_empty() {
        return Err("当前会话未绑定项目目录".into());
    }
    let cwd = Path::new(project_path);
    let remote = args["remote"].as_str().unwrap_or("origin").trim().to_string();
    let branch = args["branch"].as_str().unwrap_or("").trim().to_string();
    let autostash = args["autostash"].as_bool().unwrap_or(false);
    // 当前分支（branch 参数缺省时使用）
    let cur_branch = if branch.is_empty() {
        run_cmd("git", &["branch".into(), "--show-current".into()], Some(cwd), 15)
            .await
            .map_err(|e| with_advice("git_pull", e))?
            .trim()
            .to_string()
    } else {
        branch.clone()
    };
    if cur_branch.is_empty() {
        return Err("当前分支为空（新仓库或 detached HEAD），无法确定拉取目标；请用 git_branch switch 指定分支".into());
    }
    // 本地未提交改动检查：无 autostash 时警告（ff-only 在本地无提交时通常可成功）
    let dirty = run_cmd("git", &["status".into(), "--porcelain".into()], Some(cwd), 15)
        .await
        .map_err(|e| with_advice("git_pull", e))?;
    if !dirty.trim().is_empty() && !autostash {
        return Err(format!(
            "工作区有未提交改动，直接拉取可能失败：\n{}\n建议：先 git_commit 提交（或传 autostash=true 自动暂存后拉取）。",
            dirty.lines().take(10).collect::<Vec<_>>().join("\n")
        ));
    }
    let _gate = crate::services::tool_limits::acquire_workspace_gate(Path::new(project_path)).await;
    let mut cargs = vec!["pull".to_string(), "--ff-only".into()];
    if autostash {
        cargs.push("--autostash".into());
    }
    cargs.push(remote.clone());
    cargs.push(cur_branch.clone());
    match run_cmd("git", &cargs, Some(cwd), 120).await {
        Ok(out) => Ok(format!("已拉取 {remote}/{cur_branch}（快速前进）：\n{out}")),
        Err(e) => {
            // ff-only 失败：分叉或冲突。读取状态给出可执行建议
            let status = run_cmd("git", &["status".into()], Some(cwd), 15).await.unwrap_or_default();
            let msg = format!(
                "拉取失败（{e}）——本地与远端 {remote}/{cur_branch} 已分叉，无法快速前进。\n当前状态：\n{}\n\n可选处理：\n1. 本地有未推送提交且确实需要保留：git_commit 提交后，用 run_command 执行 `git pull --rebase {remote} {cur_branch}` 变基解决冲突（或 git merge {remote}/{cur_branch} 合并）；\n2. 不保留本地提交：git_branch switch 到干净分支或 run_command 执行 `git reset --hard {remote}/{cur_branch}`（慎用，会丢弃本地改动）。",
                status.lines().take(15).collect::<Vec<_>>().join("\n")
            );
            Err(msg)
        }
    }
}

pub(super) async fn git_push(args: &Value, roots: &[String]) -> Result<String, String> {
    let project_path = roots.first().map(String::as_str).unwrap_or("");
    if project_path.is_empty() {
        return Err("当前会话未绑定项目目录".into());
    }
    let cwd = Path::new(project_path);
    let remote = args["remote"].as_str().unwrap_or("origin").trim().to_string();
    let branch = args["branch"].as_str().unwrap_or("").trim().to_string();
    let set_upstream = args["set_upstream"].as_bool().unwrap_or(false);
    let cur_branch = if branch.is_empty() {
        run_cmd("git", &["branch".into(), "--show-current".into()], Some(cwd), 15)
            .await
            .map_err(|e| with_advice("git_push", e))?
            .trim()
            .to_string()
    } else {
        branch.clone()
    };
    if cur_branch.is_empty() {
        return Err("当前分支为空（新仓库或 detached HEAD），无法推送；请用 git_branch switch 指定分支".into());
    }
    // 前置检查 1：未提交改动
    let dirty = run_cmd("git", &["status".into(), "--porcelain".into()], Some(cwd), 15)
        .await
        .map_err(|e| with_advice("git_push", e))?;
    if !dirty.trim().is_empty() {
        return Err(format!(
            "工作区有未提交改动，请先 git_commit 提交（git_push 只推送已提交内容）：\n{}",
            dirty.lines().take(10).collect::<Vec<_>>().join("\n")
        ));
    }
    // 前置检查 2：落后远端（本地在远端之后，直接 push 会被拒）
    let sb = run_cmd("git", &["status".into(), "-sb".into()], Some(cwd), 15)
        .await
        .map_err(|e| with_advice("git_push", e))?;
    if let Some(behind) = sb.lines().next().and_then(|l| l.split(',').find(|p| p.contains("behind"))) {
        return Err(format!(
            "本地落后远端 {remote} {behind}（请先 git_pull 拉取远端更新再推送，避免推送被拒）。\n状态行：{}",
            sb.lines().next().unwrap_or("")
        ));
    }
    let _gate = crate::services::tool_limits::acquire_workspace_gate(Path::new(project_path)).await;
    let mut cargs = vec!["push".to_string()];
    if set_upstream {
        cargs.push("-u".into());
    }
    cargs.push(remote.clone());
    cargs.push(cur_branch.clone());
    let out = run_cmd("git", &cargs, Some(cwd), 120)
        .await
        .map_err(|e| with_advice("git_push", e))?;
    Ok(format!("已推送 {remote}/{cur_branch}：\n{}", out.trim_end()))
}

pub(super) async fn review_changes(args: &Value, roots: &[String]) -> Result<String, String> {
    let project_path = roots.first().map(String::as_str).unwrap_or("");
    if project_path.is_empty() {
        return Err("当前会话未绑定项目目录".into());
    }
    let cwd = Path::new(project_path);
    let scope = args["scope"].as_str().unwrap_or("all").trim();
    if !matches!(scope, "all" | "staged" | "unstaged") {
        return Err("scope 仅支持 all|staged|unstaged".into());
    }
    // 变更文件清单
    let status = run_cmd("git", &["status".into(), "--short".into()], Some(cwd), 15)
        .await
        .map_err(|e| with_advice("review_changes", e))?;
    let files: Vec<&str> = status
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect();
    if files.is_empty() {
        return Ok("工作区干净，没有待审查的改动（git_status 显示无变更）。".into());
    }
    // 按 scope 过滤：staged 只看已暂存，unstaged 只看未暂存（含未跟踪）
    let filtered: Vec<&str> = files
        .iter()
        .copied()
        .filter(|l| match scope {
            "staged" => !l.starts_with(" ") && !l.starts_with("??"),
            "unstaged" => l.starts_with(" ") || l.starts_with("??"),
            _ => true,
        })
        .collect();
    if filtered.is_empty() {
        return Ok(format!("scope={scope} 下没有待审查的改动（共 {} 个变更文件，全部在另一侧）。", files.len()));
    }
    // diff 统计与全文（staged 用 --cached，unstaged 用普通 diff，all 两者叠加）
    let mut diff_text = String::new();
    let mut stat_text = String::new();
    let mut pending: Vec<(&str, bool)> = Vec::new();
    if matches!(scope, "all" | "unstaged") {
        pending.push(("unstaged", false));
    }
    if matches!(scope, "all" | "staged") {
        pending.push(("staged", true));
    }
    let _gate = crate::services::tool_limits::acquire_workspace_gate(cwd).await;
    for (label, cached) in &pending {
        let mut cargs = vec!["diff".to_string()];
        if *cached {
            cargs.push("--cached".into());
        }
        let d = run_cmd("git", &cargs, Some(cwd), 30)
            .await
            .map_err(|e| with_advice("review_changes", e))?;
        if !d.trim().is_empty() {
            if pending.len() > 1 {
                stat_text.push_str(&format!("--- {label} ---\n"));
            }
            stat_text.push_str(&d);
            diff_text.push_str(&d);
            if pending.len() > 1 {
                diff_text.push('\n');
            }
        }
    }
    let (nf, ins, del) = summarize_diff_stats(&stat_text);
    let max_lines = args["max_lines"].as_u64().unwrap_or(400).clamp(20, 2000) as usize;
    let mut out = format!(
        "待审查改动（{} 个文件）：\n",
        filtered.len()
    );
    for f in &filtered {
        out.push_str(&format!("  {f}\n"));
    }
    out.push_str(&format!("\n统计：{nf} 个文件，+{ins} 行 / -{del} 行\n"));
    if diff_text.trim().is_empty() {
        out.push_str("（该 scope 下无文本 diff，可能是新增未跟踪文件或二进制变更）");
    } else {
        out.push_str(&format!("\nDiff：\n{}", super::cmd_tools::cut_str(&diff_text, max_lines * 80)));
    }
    Ok(out)
}

pub(super) fn summarize_diff_stats(diff: &str) -> (usize, usize, usize) {
    let mut files = 0usize;
    let mut ins = 0usize;
    let mut del = 0usize;
    for line in diff.lines() {
        if line.starts_with("diff --git ") {
            files += 1;
        } else if line.starts_with('+') && !line.starts_with("+++") {
            ins += 1;
        } else if line.starts_with('-') && !line.starts_with("---") {
            del += 1;
        }
    }
    (files, ins, del)
}

pub(super) async fn git_tag(args: &Value, roots: &[String]) -> Result<String, String> {
    let project_path = roots.first().map(String::as_str).unwrap_or("");
    if project_path.is_empty() {
        return Err("当前会话未绑定项目目录".into());
    }
    let action = args["action"].as_str().unwrap_or("list");
    let name = args["name"].as_str().unwrap_or("").trim();
    let cwd = Path::new(project_path);
    match action {
        "create" => {
            if name.is_empty() {
                return Err("git_tag create 需要参数 {\"name\":\"<标签名>\"}".into());
            }
            let out = run_cmd("git", &["tag".into(), name.to_string()], Some(cwd), 30)
                .await
                .map_err(|e| with_advice("git_tag", e))?;
            Ok(format!("已创建标签 {name}\n{out}"))
        }
        _ => {
            let out = run_cmd("git", &["tag".into(), "--list".into()], Some(cwd), 15)
                .await
                .map_err(|e| with_advice("git_tag", e))?;
            if out.trim().is_empty() {
                return Ok("（尚无标签）".into());
            }
            Ok(out)
        }
    }
}
