//! command grader（docs/AGENT_EVAL_HARNESS.md §3/§6）。
//!
//! 在独立工作树中运行已通过 `eval_task` 安全校验的命令，以退出码判定通过与否。
//! 命令以 argv 直接执行、不经过 shell；超时由本模块用线程 + `recv_timeout` 兜底，
//! 但真实隔离与资源上限由外层 sandbox 承担。

use crate::agent::eval_task::EvalGrader;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraderOutcome {
    pub passed: bool,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
    pub duration_ms: u64,
}

pub fn run_command_grader(grader: &EvalGrader, workspace: &Path) -> Result<GraderOutcome, String> {
    // 校验已在 eval_task::validate_eval_task 完成；这里仅做防御性兜底，避免越权直接跑任意命令。
    crate::agent::eval_task::validate_eval_task(&crate::agent::eval_task::EvalTask {
        schema_version: crate::agent::eval_task::EVAL_TASK_SCHEMA_VERSION,
        task_id: "grader".into(),
        suite: "grader".into(),
        problem_statement: "grader".into(),
        repo: crate::agent::eval_task::EvalRepo {
            url: "file:///".into(),
            base_commit: "0000000".into(),
            subdir: None,
        },
        limits: crate::agent::eval_task::EvalLimits {
            wall_time_seconds: 60,
            max_steps: 1,
            max_cost_cny: 0.0,
            network: "none".into(),
        },
        grader: grader.clone(),
        artifacts: vec![],
    })?;

    let program = grader.command[0].clone();
    let args: Vec<String> = grader.command[1..].to_vec();
    let workspace = workspace.to_path_buf();
    let timeout = Duration::from_secs(grader.timeout_seconds);

    let (tx, rx) = std::sync::mpsc::channel();
    let started = std::time::Instant::now();
    std::thread::spawn(move || {
        let result = Command::new(&program)
            .args(&args)
            .current_dir(&workspace)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output();
        let _ = tx.send(result);
    });

    match rx.recv_timeout(timeout) {
        Ok(Ok(output)) => Ok(GraderOutcome {
            passed: output.status.success(),
            exit_code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            timed_out: false,
            duration_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
        }),
        Ok(Err(error)) => Err(format!("grader 启动失败：{error}")),
        Err(_) => Ok(GraderOutcome {
            passed: false,
            exit_code: None,
            stdout: String::new(),
            stderr: String::new(),
            timed_out: true,
            duration_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grader(command: Vec<&str>) -> EvalGrader {
        EvalGrader {
            kind: "command".into(),
            command: command.into_iter().map(str::to_string).collect(),
            timeout_seconds: 10,
        }
    }

    #[test]
    fn passing_command_marks_passed() {
        let workspace = std::env::temp_dir();
        let outcome = run_command_grader(&grader(vec!["true"]), &workspace).unwrap();
        assert!(outcome.passed);
        assert_eq!(outcome.exit_code, Some(0));
        assert!(!outcome.timed_out);
    }

    #[test]
    fn failing_command_marks_failed_with_exit_code() {
        let workspace = std::env::temp_dir();
        let outcome = run_command_grader(&grader(vec!["false"]), &workspace).unwrap();
        assert!(!outcome.passed);
        assert_eq!(outcome.exit_code, Some(1));
    }

    #[test]
    fn rejects_unsafe_command_via_task_validation() {
        let workspace = std::env::temp_dir();
        assert!(run_command_grader(&grader(vec!["/usr/bin/rm", "-rf"]), &workspace).is_err());
        assert!(run_command_grader(&grader(vec!["sh", "-c", "rm -rf /"]), &workspace).is_err());
        assert!(run_command_grader(&grader(vec!["bash", "-c", "id"]), &workspace).is_err());
    }
}
