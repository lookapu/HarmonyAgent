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

/// 最小 glob 匹配：`*` 匹配段内任意字符，`**` 匹配零个或多个路径段。
/// 覆盖 eval task `artifacts` 字段的常见形态（`dir/**`、`dir/*.log`、`file.txt`）。
fn glob_matches(pattern: &str, rel: &str) -> bool {
    fn seg(pat: &str, name: &str) -> bool {
        if !pat.contains('*') {
            return pat == name;
        }
        let parts: Vec<&str> = pat.split('*').collect();
        let mut pos = 0usize;
        for (i, part) in parts.iter().enumerate() {
            if part.is_empty() {
                continue;
            }
            if i == 0 {
                if !name.starts_with(part) {
                    return false;
                }
                pos = part.len();
            } else if i == parts.len() - 1 {
                if !name[pos..].ends_with(part) {
                    return false;
                }
            } else {
                match name[pos..].find(part) {
                    Some(idx) => pos += idx + part.len(),
                    None => return false,
                }
            }
        }
        true
    }
    let pat: Vec<&str> = pattern.split('/').collect();
    let segs: Vec<&str> = rel.split('/').collect();
    fn walk(pat: &[&str], segs: &[&str]) -> bool {
        match (pat.first(), segs.first()) {
            (None, None) => true,
            (Some(&"**"), _) => {
                walk(&pat[1..], segs) || (!segs.is_empty() && walk(pat, &segs[1..]))
            }
            (Some(p), Some(s)) => seg(p, s) && walk(&pat[1..], &segs[1..]),
            _ => false,
        }
    }
    walk(&pat, &segs)
}

/// 采集任务声明的 artifacts（glob 匹配）到输出目录的 `artifacts/` 子目录，保留相对结构。
/// 返回收集到的相对路径列表。工作树内无匹配文件不视为错误。
pub fn collect_artifacts(
    worktree: &Path,
    artifacts: &[String],
    output_dir: &Path,
) -> Result<Vec<String>, String> {
    let dest = output_dir.join("artifacts");
    std::fs::create_dir_all(&dest).map_err(|error| format!("创建 artifacts 目录失败：{error}"))?;
    let mut collected = Vec::new();
    walk_and_collect(worktree, worktree, &dest, artifacts, &mut collected)?;
    Ok(collected)
}

fn walk_and_collect(
    dir: &Path,
    base: &Path,
    dest: &Path,
    patterns: &[String],
    out: &mut Vec<String>,
) -> Result<(), String> {
    for entry in std::fs::read_dir(dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        let rel = path
            .strip_prefix(base)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        if path.is_dir() {
            if entry.file_name() != ".git" {
                walk_and_collect(&path, base, dest, patterns, out)?;
            }
        } else if patterns.iter().any(|pattern| glob_matches(pattern, &rel)) {
            let target = dest.join(&rel);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            std::fs::copy(&path, &target).map_err(|e| e.to_string())?;
            out.push(rel);
        }
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

    #[test]
    fn glob_matches_supports_star_and_double_star() {
        assert!(glob_matches("test-results/**", "test-results/a.log"));
        assert!(glob_matches("test-results/**", "test-results/sub/b.log"));
        assert!(glob_matches("test-results/*.log", "test-results/a.log"));
        assert!(!glob_matches("test-results/*.log", "test-results/sub/a.log"));
        assert!(glob_matches("out/report.json", "out/report.json"));
        assert!(!glob_matches("out/report.json", "out/other.json"));
        assert!(glob_matches("**/*.hap", "entry/build/x.hap"));
    }

    #[test]
    fn collect_artifacts_copies_matches_preserving_structure() {
        let worktree = temp_dir("art-ws");
        fs::create_dir_all(worktree.join("test-results/sub")).unwrap();
        fs::create_dir_all(worktree.join("entry/build")).unwrap();
        fs::write(worktree.join("test-results/a.log"), "a").unwrap();
        fs::write(worktree.join("test-results/sub/b.log"), "b").unwrap();
        fs::write(worktree.join("entry/build/app.hap"), "hap").unwrap();
        fs::create_dir_all(worktree.join("src")).unwrap();
        fs::write(worktree.join("src/main.ets"), "code").unwrap();

        let output = temp_dir("art-out");
        let mut collected = collect_artifacts(
            &worktree,
            &["test-results/**".to_string(), "**/*.hap".to_string()],
            &output,
        )
        .unwrap();
        collected.sort();
        assert_eq!(
            collected,
            vec![
                "entry/build/app.hap".to_string(),
                "test-results/a.log".to_string(),
                "test-results/sub/b.log".to_string(),
            ]
        );
        assert!(output.join("artifacts/test-results/sub/b.log").exists());
        assert!(output.join("artifacts/entry/build/app.hap").exists());
        assert!(!output.join("artifacts/src/main.ets").exists());

        fs::remove_dir_all(worktree).ok();
        fs::remove_dir_all(output).ok();
    }
}
