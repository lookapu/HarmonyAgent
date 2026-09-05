//! eval 工作树准备（docs/AGENT_EVAL_HARNESS.md §6「prepare immutable base + task worktree」）。
//!
//! v1 只接受本地已准备仓库：`prepare_worktree` 用 `git worktree add --detach` 从 base commit
//! 检出干净、隔离的 run 专用工作树，Agent 的改动与原始仓库互不影响；grader 侧再独立建一棵。

use std::path::Path;
use std::process::Command;

pub fn prepare_worktree(source_repo: &Path, target: &Path, base_commit: &str) -> Result<(), String> {
    if target.exists() {
        return Err(format!("目标工作树已存在：{}", target.display()));
    }
    let output = Command::new("git")
        .args(["worktree", "add", "--detach"])
        .arg(target)
        .arg(base_commit)
        .current_dir(source_repo)
        .output()
        .map_err(|error| format!("运行 git worktree add 失败：{error}"))?;
    if !output.status.success() {
        return Err(format!(
            "git worktree add 失败：{}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn git(dir: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .expect("run git");
        assert!(
            output.status.success(),
            "git {args:?} 失败：{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("deveco-eval-ws-{tag}-{}", uuid::Uuid::new_v4()))
    }

    #[test]
    fn prepare_worktree_checks_out_clean_base() {
        let source = temp_dir("source");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("a.txt"), "base\n").unwrap();
        git(&source, &["init", "-q"]);
        git(&source, &["add", "a.txt"]);
        git(&source, &["-c", "user.email=e@x", "-c", "user.name=t", "commit", "-q", "-m", "base"]);
        let base = git(&source, &["rev-parse", "HEAD"]);

        let target = temp_dir("target");
        prepare_worktree(&source, &target, &base).unwrap();
        assert_eq!(fs::read_to_string(target.join("a.txt")).unwrap(), "base\n");

        // 工作树隔离：改动 target 不影响 source。
        fs::write(target.join("a.txt"), "changed\n").unwrap();
        assert_eq!(fs::read_to_string(source.join("a.txt")).unwrap(), "base\n");

        fs::remove_dir_all(source).ok();
        fs::remove_dir_all(target).ok();
    }

    #[test]
    fn prepare_worktree_rejects_existing_target() {
        let source = temp_dir("src2");
        fs::create_dir_all(&source).unwrap();
        git(&source, &["init", "-q"]);

        let target = temp_dir("target2");
        fs::create_dir_all(&target).unwrap();
        assert!(prepare_worktree(&source, &target, "HEAD").is_err());

        fs::remove_dir_all(source).ok();
        fs::remove_dir_all(target).ok();
    }
}
