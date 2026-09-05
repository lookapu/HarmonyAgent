//! eval 补丁采集与应用（docs/AGENT_EVAL_HARNESS.md §6）。
//!
//! Agent 在 worktree 内改动后，`collect_patch` 用 `git diff <base_commit>` 产出 `model.patch`；
//! grader 侧在原始 base commit 的干净工作树中用 `git apply` 应用该 patch 后执行。两者都以
//! git 为事实来源，不信任 Agent 自述的“改了什么”。

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

/// 采集 worktree 相对 `base_commit` 的改动，返回 unified diff 文本（含二进制）。
pub fn collect_patch(worktree: &Path, base_commit: &str) -> Result<String, String> {
    let output = Command::new("git")
        .args(["diff", "--binary", base_commit])
        .current_dir(worktree)
        .output()
        .map_err(|error| format!("运行 git diff 失败：{error}"))?;
    if !output.status.success() {
        return Err(format!(
            "git diff 失败：{}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// 在 grader 干净工作树中应用 `model.patch`（stdin 传入，不经过 shell）。
pub fn apply_patch(worktree: &Path, patch: &str) -> Result<(), String> {
    let mut child = Command::new("git")
        .args(["apply", "--whitespace=nowarn"])
        .current_dir(worktree)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("启动 git apply 失败：{error}"))?;
    child
        .stdin
        .take()
        .ok_or_else(|| "无法写入 git apply 输入".to_string())?
        .write_all(patch.as_bytes())
        .map_err(|error| format!("写入 patch 失败：{error}"))?;
    let output = child
        .wait_with_output()
        .map_err(|error| format!("等待 git apply 失败：{error}"))?;
    if !output.status.success() {
        return Err(format!(
            "git apply 失败：{}",
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
        std::env::temp_dir().join(format!("deveco-eval-patch-{tag}-{}", uuid::Uuid::new_v4()))
    }

    fn init_repo(dir: &Path, content: &str) -> String {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join("a.txt"), content).unwrap();
        git(dir, &["init", "-q"]);
        git(dir, &["add", "a.txt"]);
        git(dir, &["-c", "user.email=e@x", "-c", "user.name=t", "commit", "-q", "-m", "base"]);
        git(dir, &["rev-parse", "HEAD"])
    }

    #[test]
    fn collect_patch_returns_diff_against_base() {
        let dir = temp_dir("collect");
        let base = init_repo(&dir, "base\n");
        fs::write(dir.join("a.txt"), "changed\n").unwrap();

        let patch = collect_patch(&dir, &base).unwrap();
        assert!(patch.contains("+changed"), "patch 应含新增行：{patch}");
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn apply_patch_reproduces_change_in_clean_worktree() {
        let author_dir = temp_dir("author");
        let base = init_repo(&author_dir, "base\n");
        fs::write(author_dir.join("a.txt"), "changed\n").unwrap();
        let patch = collect_patch(&author_dir, &base).unwrap();

        let grader_dir = temp_dir("grader");
        let _ = init_repo(&grader_dir, "base\n");
        apply_patch(&grader_dir, &patch).unwrap();
        let applied = fs::read_to_string(grader_dir.join("a.txt")).unwrap();
        assert_eq!(applied, "changed\n");

        fs::remove_dir_all(author_dir).ok();
        fs::remove_dir_all(grader_dir).ok();
    }

    #[test]
    fn apply_patch_rejects_invalid_patch() {
        let grader_dir = temp_dir("badpatch");
        let _ = init_repo(&grader_dir, "base\n");
        let result = apply_patch(&grader_dir, "not a valid patch\n");
        assert!(result.is_err());
        fs::remove_dir_all(grader_dir).ok();
    }
}
