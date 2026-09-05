//! eval run 编排器（docs/AGENT_EVAL_HARNESS.md §6 执行状态机）。
//!
//! `run_trial` 把已落地的各阶段串成一条可跑闭环：validate → prepare worktree →
//! drive agent → collect patch → grade in clean worktree → 组装结果。Agent 驱动是
//! 可注入的 [`AgentDriver`]（真实实现待从 `commands/chat.rs` 抽取 headless 驱动核心，
//! 见 §9「可以替换 Provider 配置来源」），编排逻辑本身不依赖 UI、可用桩驱动端到端验证。

use crate::agent::eval_task::{validate_eval_task, EvalTask};
use crate::agent::eval_grader::{run_command_grader, GraderOutcome};
use crate::agent::eval_patch::{apply_patch, collect_patch};
use crate::agent::eval_workspace::{collect_artifacts, prepare_worktree};
use std::path::{Path, PathBuf};
use std::time::Instant;

pub const OUTCOME_RESOLVED: &str = "resolved";
pub const OUTCOME_UNRESOLVED: &str = "unresolved";
pub const OUTCOME_HARNESS_ERROR: &str = "harness_error";

/// 在给定工作树中完成任务的执行核心。真实实现待从 UI 耦合的 `commands/chat.rs`
/// 抽取 headless 驱动；测试桩只需在工作树里做出改动即可驱动整条闭环。
pub trait AgentDriver: Send + Sync {
    fn run(&self, task: &EvalTask, workspace: &Path) -> Result<(), String>;
}

#[derive(Debug, Clone)]
pub struct EvalTrialOutcome {
    pub status: String,
    pub patch: String,
    pub grader: GraderOutcome,
    pub collected_artifacts: Vec<String>,
    pub duration_ms: u64,
}

/// 一次 trial 的完整编排。`source_repo` 是本地已准备仓库；`output_dir` 为 run 专用目录，
/// 内部产出 `agent/`、`grader/` 两棵隔离工作树与 `model.patch`。
pub fn run_trial(
    task: &EvalTask,
    source_repo: &Path,
    output_dir: &Path,
    driver: &dyn AgentDriver,
) -> Result<EvalTrialOutcome, String> {
    validate_eval_task(task)?;
    std::fs::create_dir_all(output_dir)
        .map_err(|error| format!("创建输出目录失败：{error}"))?;
    let started = Instant::now();
    let agent_ws = output_dir.join("agent");
    let grader_ws = output_dir.join("grader");

    prepare_worktree(source_repo, &agent_ws, &task.repo.base_commit)?;
    driver.run(task, &agent_ws)?;
    let patch = collect_patch(&agent_ws, &task.repo.base_commit)?;
    std::fs::write(output_dir.join("model.patch"), &patch)
        .map_err(|error| format!("写入 model.patch 失败：{error}"))?;

    prepare_worktree(source_repo, &grader_ws, &task.repo.base_commit)?;
    apply_patch(&grader_ws, &patch)?;
    let grader = run_command_grader(&task.grader, &grader_ws)?;
    // 采集任务声明的产物（如 test-results/**），从 grader 干净工作树收集，保留相对结构。
    let collected_artifacts = collect_artifacts(&grader_ws, &task.artifacts, output_dir)?;

    let status = if grader.passed {
        OUTCOME_RESOLVED
    } else {
        OUTCOME_UNRESOLVED
    };
    Ok(EvalTrialOutcome {
        status: status.to_string(),
        patch,
        grader,
        collected_artifacts,
        duration_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
    })
}

/// 测试桩：把 `a.txt` 改为 `fixed`，模拟 Agent 完成任务后的改动。
#[cfg(test)]
pub struct StubAgentDriver;

#[cfg(test)]
impl AgentDriver for StubAgentDriver {
    fn run(&self, _task: &EvalTask, workspace: &Path) -> Result<(), String> {
        std::fs::write(workspace.join("a.txt"), "fixed\n")
            .map_err(|error| format!("stub agent 写入失败：{error}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::eval_task::{EvalGrader, EvalLimits, EvalRepo};
    use std::fs;
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) -> String {
        let output = Command::new("git").args(args).current_dir(dir).output().unwrap();
        assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn task_with_grader(grader_command: Vec<&str>) -> EvalTask {
        EvalTask {
            schema_version: crate::agent::eval_task::EVAL_TASK_SCHEMA_VERSION,
            task_id: "smoke__fix-1".into(),
            suite: "smoke".into(),
            problem_statement: "make a.txt contain fixed".into(),
            repo: EvalRepo {
                url: "file:///repo".into(),
                base_commit: "0000000".into(),
                subdir: None,
            },
            limits: EvalLimits {
                wall_time_seconds: 60,
                max_steps: 10,
                max_cost_cny: 0.0,
                network: "none".into(),
            },
            grader: EvalGrader {
                kind: "command".into(),
                command: grader_command.into_iter().map(str::to_string).collect(),
                timeout_seconds: 30,
            },
            artifacts: vec![],
        }
    }

    fn source_repo_with_base() -> (PathBuf, String) {
        let dir = std::env::temp_dir().join(format!("deveco-eval-run-src-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("a.txt"), "base\n").unwrap();
        git(&dir, &["init", "-q"]);
        git(&dir, &["add", "a.txt"]);
        git(&dir, &["-c", "user.email=e@x", "-c", "user.name=t", "commit", "-q", "-m", "base"]);
        let base = git(&dir, &["rev-parse", "HEAD"]);
        (dir, base)
    }

    #[test]
    fn run_trial_resolves_when_stub_fix_passes_grader() {
        let (source, base) = source_repo_with_base();
        let mut task = task_with_grader(vec!["grep", "-q", "fixed", "a.txt"]);
        task.repo.base_commit = base.clone();
        let output_dir = std::env::temp_dir().join(format!("deveco-eval-run-out-{}", uuid::Uuid::new_v4()));

        let outcome = run_trial(&task, &source, &output_dir, &StubAgentDriver).unwrap();
        assert_eq!(outcome.status, OUTCOME_RESOLVED);
        assert!(outcome.patch.contains("+fixed"), "patch 应含 fixed：{}", outcome.patch);
        assert!(output_dir.join("model.patch").exists());

        fs::remove_dir_all(source).ok();
        fs::remove_dir_all(output_dir).ok();
    }

    #[test]
    fn run_trial_unresolved_when_grader_fails() {
        let (source, base) = source_repo_with_base();
        // grader 要求 a.txt 含 "other"，但 stub 写的是 "fixed"
        let mut task = task_with_grader(vec!["grep", "-q", "other", "a.txt"]);
        task.repo.base_commit = base.clone();
        let output_dir = std::env::temp_dir().join(format!("deveco-eval-run-out2-{}", uuid::Uuid::new_v4()));

        let outcome = run_trial(&task, &source, &output_dir, &StubAgentDriver).unwrap();
        assert_eq!(outcome.status, OUTCOME_UNRESOLVED);
        assert!(!outcome.grader.passed);

        fs::remove_dir_all(source).ok();
        fs::remove_dir_all(output_dir).ok();
    }

    #[test]
    fn run_trial_collects_declared_artifacts() {
        let (source, base) = source_repo_with_base();
        // 声明 a.txt 为产物：stub 把 a.txt 改为 fixed，patch 应用后 grader 工作树含 a.txt=fixed。
        let mut task = task_with_grader(vec!["grep", "-q", "fixed", "a.txt"]);
        task.repo.base_commit = base.clone();
        task.artifacts = vec!["a.txt".to_string()];
        let output_dir = std::env::temp_dir().join(format!("deveco-eval-run-out3-{}", uuid::Uuid::new_v4()));

        let outcome = run_trial(&task, &source, &output_dir, &StubAgentDriver).unwrap();
        assert_eq!(outcome.status, OUTCOME_RESOLVED);
        assert_eq!(outcome.collected_artifacts, vec!["a.txt".to_string()]);
        assert!(output_dir.join("artifacts/a.txt").exists());
        assert_eq!(fs::read_to_string(output_dir.join("artifacts/a.txt")).unwrap(), "fixed\n");

        fs::remove_dir_all(source).ok();
        fs::remove_dir_all(output_dir).ok();
    }
}
