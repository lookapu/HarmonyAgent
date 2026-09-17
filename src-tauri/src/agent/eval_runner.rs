//! eval run 编排器（docs/AGENT_EVAL_HARNESS.md §6 执行状态机）。
//!
//! `run_trial` 把已落地的各阶段串成一条可跑闭环：validate → prepare worktree →
//! drive agent → collect patch → grade in clean worktree → 组装结果。Agent 驱动是
//! 可注入的 [`AsyncAgentDriver`]（builtin 最小驱动已落地，最终仍需与
//! `commands/chat.rs` 共享统一 AgentKernel），编排逻辑本身不依赖 UI、可用桩驱动端到端验证。

use crate::agent::eval_grader::{run_command_grader, GraderOutcome};
use crate::agent::eval_patch::{apply_patch, collect_patch};
use crate::agent::eval_report::{
    EvalManifest, EvalReport, HarnessInfo, ModelInfo, OutcomeInfo, PromptInfo, RunInfo,
    SandboxInfo, TaskInfo, ToolRegistryInfo, EVAL_REPORT_SCHEMA_VERSION,
};
use crate::agent::eval_task::{validate_eval_task, EvalTask};
use crate::agent::eval_trajectory::{TrajectoryEvent, TrajectoryWriter};
use crate::agent::eval_workspace::{collect_artifacts, prepare_worktree};
use sha2::{Digest, Sha256};
use std::future::Future;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::{Command, Stdio};
use std::time::Duration;
use std::time::Instant;

pub const OUTCOME_RESOLVED: &str = "resolved";
pub const OUTCOME_UNRESOLVED: &str = "unresolved";
pub const OUTCOME_HARNESS_ERROR: &str = "harness_error";
pub const OUTCOME_CANCELLED: &str = "cancelled";

#[derive(Debug, Clone)]
pub enum AgentDriverError {
    Failed(String),
    Cancelled(String),
}

impl AgentDriverError {
    fn status(&self) -> &'static str {
        match self {
            Self::Failed(_) => OUTCOME_HARNESS_ERROR,
            Self::Cancelled(_) => OUTCOME_CANCELLED,
        }
    }
    fn message(&self) -> &str {
        match self {
            Self::Failed(message) | Self::Cancelled(message) => message,
        }
    }
}

impl From<String> for AgentDriverError {
    fn from(value: String) -> Self {
        Self::Failed(value)
    }
}

/// 旧同步 driver 边界，仅供外部进程 adapter 内部兼容。runner 主路径使用
/// [`AsyncAgentDriver`]，避免 Provider/工具异步执行被嵌套 runtime 包装。
pub trait AgentDriver: Send + Sync {
    fn run(
        &self,
        task: &EvalTask,
        workspace: &Path,
    ) -> Result<AgentDriverOutcome, AgentDriverError>;
}

/// 异步 driver 边界。保留旧同步 trait 以兼容外部 adapter；builtin 和未来 kernel 优先实现此接口。
pub trait AsyncAgentDriver: Send + Sync {
    fn run_async<'a>(
        &'a self,
        task: &'a EvalTask,
        workspace: &'a Path,
    ) -> Pin<Box<dyn Future<Output = Result<AgentDriverOutcome, AgentDriverError>> + Send + 'a>>;
}

/// 真实驱动必须返回可审计的资源计量与事件，不允许 runner 从最终文本猜测。
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct AgentDriverOutcome {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_tokens: u64,
    pub cost_cny: f64,
    pub steps: u64,
    pub tool_calls: u64,
    pub retries: u64,
    pub trajectory: Vec<TrajectoryEvent>,
    pub failure_taxonomy: Vec<String>,
    pub policy_violations: u64,
}

/// 通过受信任的本地 adapter 接入真实 Agent。任务 JSON 只经 stdin 传递，工作树通过
/// current_dir 约束；adapter stdout 必须只包含 `AgentDriverOutcome` JSON。
#[derive(Clone)]
pub struct ProcessAgentDriver {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub timeout: Duration,
}

impl AgentDriver for ProcessAgentDriver {
    fn run(
        &self,
        task: &EvalTask,
        workspace: &Path,
    ) -> Result<AgentDriverOutcome, AgentDriverError> {
        if !self.program.is_absolute() || !self.program.is_file() {
            return Err(AgentDriverError::Failed(
                "driver program 必须是存在的绝对文件路径".into(),
            ));
        }
        let mut child = Command::new(&self.program)
            .args(&self.args)
            .current_dir(workspace)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| AgentDriverError::Failed(format!("启动 driver 失败：{e}")))?;
        let request = serde_json::to_vec(task)
            .map_err(|e| AgentDriverError::Failed(format!("序列化 driver 请求失败：{e}")))?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| AgentDriverError::Failed("driver stdin 不可用".into()))?;
        stdin
            .write_all(&request)
            .map_err(|e| AgentDriverError::Failed(format!("写入 driver stdin 失败：{e}")))?;
        stdin
            .write_all(b"\n")
            .map_err(|e| AgentDriverError::Failed(format!("结束 driver stdin 请求失败：{e}")))?;
        drop(stdin);
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| AgentDriverError::Failed("driver stdout 不可用".into()))?;
        let mut stderr = child
            .stderr
            .take()
            .ok_or_else(|| AgentDriverError::Failed("driver stderr 不可用".into()))?;
        let stdout_reader = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            stdout.read_to_end(&mut bytes).map(|_| bytes)
        });
        let stderr_reader = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            stderr.read_to_end(&mut bytes).map(|_| bytes)
        });
        let started = Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if started.elapsed() < self.timeout => {
                    std::thread::sleep(Duration::from_millis(25))
                }
                Ok(None) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(AgentDriverError::Cancelled(
                        "driver 超过 wall time，已终止".into(),
                    ));
                }
                Err(e) => return Err(AgentDriverError::Failed(format!("等待 driver 失败：{e}"))),
            }
        };
        let stdout = stdout_reader
            .join()
            .map_err(|_| AgentDriverError::Failed("driver stdout reader panic".into()))?
            .map_err(|e| AgentDriverError::Failed(format!("读取 driver stdout 失败：{e}")))?;
        let stderr = stderr_reader
            .join()
            .map_err(|_| AgentDriverError::Failed("driver stderr reader panic".into()))?
            .map_err(|e| AgentDriverError::Failed(format!("读取 driver stderr 失败：{e}")))?;
        if !status.success() {
            let stderr = String::from_utf8_lossy(&stderr);
            return Err(AgentDriverError::Failed(format!(
                "driver 退出码 {:?}：{}",
                status.code(),
                stderr.trim()
            )));
        }
        serde_json::from_slice(&stdout).map_err(|e| {
            AgentDriverError::Failed(format!("driver stdout 不是 AgentDriverOutcome JSON：{e}"))
        })
    }
}

impl AsyncAgentDriver for ProcessAgentDriver {
    fn run_async<'a>(
        &'a self,
        task: &'a EvalTask,
        workspace: &'a Path,
    ) -> Pin<Box<dyn Future<Output = Result<AgentDriverOutcome, AgentDriverError>> + Send + 'a>>
    {
        let driver = self.clone();
        let task = task.clone();
        let workspace = workspace.to_path_buf();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || driver.run(&task, &workspace))
                .await
                .map_err(|error| {
                    AgentDriverError::Failed(format!("driver blocking worker 失败：{error}"))
                })?
        })
    }
}

/// 一次 eval 的不可变运行条件。调用方必须显式提供真实指纹，runner 不填伪造默认值。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EvalRunConfig {
    pub run_id: String,
    pub suite_version: String,
    pub grader_version: String,
    pub harness: HarnessInfo,
    pub model: ModelInfo,
    pub prompt: PromptInfo,
    pub tool_registry: ToolRegistryInfo,
    pub sandbox: SandboxInfo,
    /// 单次 Provider 请求的硬上限（秒）。缺省时 builtin driver 使用内置默认值；
    /// 实际生效值始终取它与剩余 wall time 的较小值。
    #[serde(default)]
    pub request_timeout_seconds: Option<u64>,
}

const MAX_REQUEST_TIMEOUT_SECONDS: u64 = 3600;

fn validate_run_config(config: &EvalRunConfig) -> Result<(), String> {
    let required = [
        ("run_id", config.run_id.as_str()),
        ("suite_version", config.suite_version.as_str()),
        ("grader_version", config.grader_version.as_str()),
        ("harness.commit", config.harness.commit.as_str()),
        ("model.provider", config.model.provider.as_str()),
        ("model.model_id", config.model.model_id.as_str()),
        (
            "prompt.profile_version",
            config.prompt.profile_version.as_str(),
        ),
        ("prompt.digest", config.prompt.digest.as_str()),
        (
            "tool_registry.version",
            config.tool_registry.version.as_str(),
        ),
        ("tool_registry.digest", config.tool_registry.digest.as_str()),
        ("sandbox.backend", config.sandbox.backend.as_str()),
    ];
    if let Some((field, _)) = required
        .into_iter()
        .find(|(_, value)| value.trim().is_empty())
    {
        return Err(format!(
            "eval run 配置缺少真实 {field}，拒绝生成不可复现报告"
        ));
    }
    if !config
        .run_id
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_'))
    {
        return Err("eval run_id 只能包含字母、数字、点、短横线和下划线".into());
    }
    for (field, digest) in [
        ("prompt.digest", config.prompt.digest.as_str()),
        ("tool_registry.digest", config.tool_registry.digest.as_str()),
    ] {
        let value = digest.strip_prefix("sha256:").unwrap_or_default();
        if value.len() != 64 || !value.chars().all(|ch| ch.is_ascii_hexdigit()) {
            return Err(format!("{field} 必须是完整 sha256:<64 hex> 指纹"));
        }
    }
    if let Some(timeout) = config.request_timeout_seconds {
        if timeout == 0 || timeout > MAX_REQUEST_TIMEOUT_SECONDS {
            return Err(format!(
                "request_timeout_seconds 必须在 1..={MAX_REQUEST_TIMEOUT_SECONDS} 之间"
            ));
        }
    }
    Ok(())
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn write_json(path: &Path, value: &str, label: &str) -> Result<(), String> {
    std::fs::write(path, value).map_err(|error| format!("写入 {label} 失败：{error}"))
}

fn task_workspace(repo_workspace: &Path, task: &EvalTask) -> Result<PathBuf, String> {
    let workspace = task.repo.subdir.as_deref().map_or_else(
        || repo_workspace.to_path_buf(),
        |subdir| repo_workspace.join(subdir),
    );
    if !workspace.is_dir() {
        return Err(format!(
            "任务子目录不存在或不是目录：{}",
            task.repo.subdir.as_deref().unwrap_or(".")
        ));
    }
    Ok(workspace)
}

#[derive(Debug, Clone)]
pub struct EvalTrialOutcome {
    pub status: String,
    pub patch: String,
    pub grader: GraderOutcome,
    pub collected_artifacts: Vec<String>,
    pub duration_ms: u64,
    pub trajectory_events: u64,
    pub patch_digest: String,
    pub trajectory_digest: String,
}

/// 一次 trial 的完整编排。`source_repo` 是本地已准备仓库；`output_dir` 为 run 专用目录，
/// 内部产出 `agent/`、`grader/` 两棵隔离工作树与 `model.patch`。
pub async fn run_trial(
    task: &EvalTask,
    source_repo: &Path,
    output_dir: &Path,
    driver: &dyn AsyncAgentDriver,
    config: &EvalRunConfig,
) -> Result<EvalTrialOutcome, String> {
    validate_eval_task(task)?;
    validate_run_config(config)?;
    if config.sandbox.network_policy != task.limits.network {
        return Err(format!(
            "sandbox network_policy={} 与任务 limits.network={} 不一致",
            config.sandbox.network_policy, task.limits.network
        ));
    }
    if let Some(timeout) = config.request_timeout_seconds {
        if timeout > task.limits.wall_time_seconds {
            return Err(format!(
                "request_timeout_seconds={timeout} 超过任务 wall_time_seconds={}",
                task.limits.wall_time_seconds
            ));
        }
    }
    std::fs::create_dir_all(output_dir).map_err(|error| format!("创建输出目录失败：{error}"))?;
    let started = Instant::now();
    let started_at = chrono::Utc::now().to_rfc3339();
    let task_info = TaskInfo {
        suite: task.suite.clone(),
        suite_version: config.suite_version.clone(),
        task_id: task.task_id.clone(),
        task_digest: sha256_hex(
            &serde_json::to_vec(task).map_err(|error| format!("序列化任务指纹失败：{error}"))?,
        ),
        repo_base_commit: task.repo.base_commit.clone(),
    };
    let manifest = EvalManifest {
        schema_version: EVAL_REPORT_SCHEMA_VERSION,
        run_id: config.run_id.clone(),
        created_at: started_at.clone(),
        task: task_info.clone(),
        harness: config.harness.clone(),
        model: config.model.clone(),
        prompt: config.prompt.clone(),
        tool_registry: config.tool_registry.clone(),
        sandbox: config.sandbox.clone(),
    };
    write_json(
        &output_dir.join("manifest.json"),
        &manifest.to_json()?,
        "manifest.json",
    )?;
    let workspaces = output_dir.join("workspaces");
    std::fs::create_dir_all(&workspaces)
        .map_err(|error| format!("创建隔离工作树目录失败：{error}"))?;
    let agent_ws = workspaces.join("agent");
    let grader_ws = workspaces.join("grader");

    prepare_worktree(source_repo, &agent_ws, &task.repo.base_commit)?;
    let agent_task_ws = task_workspace(&agent_ws, task)?;
    let trajectory_path = output_dir.join("trajectory.jsonl");
    let mut trajectory = TrajectoryWriter::create(&trajectory_path)?;
    trajectory.append(&TrajectoryEvent {
        ts: started_at.clone(),
        kind: "trial_started".into(),
        fields: serde_json::json!({
            "run_id": config.run_id,
            "task_id": task.task_id,
            "request_timeout_seconds": config.request_timeout_seconds,
        }),
    })?;
    let driver_outcome = match driver.run_async(task, &agent_task_ws).await {
        Ok(outcome) => outcome,
        Err(failure) => {
            let status = failure.status();
            let failure_message = failure.message().to_string();
            let cancelled = matches!(&failure, AgentDriverError::Cancelled(_));
            trajectory.append(&TrajectoryEvent {
                ts: chrono::Utc::now().to_rfc3339(),
                kind: if cancelled {
                    "agent_cancelled"
                } else {
                    "agent_failed"
                }
                .into(),
                fields: serde_json::json!({ "message": failure_message }),
            })?;
            let (_, raw_digest) = trajectory.finish()?;
            let trajectory_digest = format!("sha256:{raw_digest}");
            let patch = String::new();
            write_json(&output_dir.join("model.patch"), &patch, "model.patch")?;
            let patch_digest = sha256_hex(patch.as_bytes());
            let grader_dir = output_dir.join("grader");
            std::fs::create_dir_all(&grader_dir)
                .map_err(|e| format!("创建 grader 日志目录失败：{e}"))?;
            write_json(&grader_dir.join("stdout.log"), "", "grader/stdout.log")?;
            write_json(
                &grader_dir.join("stderr.log"),
                "grader 未运行：Agent 未正常结束\n",
                "grader/stderr.log",
            )?;
            let duration_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
            let report = EvalReport {
                schema_version: EVAL_REPORT_SCHEMA_VERSION,
                harness: config.harness.clone(),
                model: config.model.clone(),
                prompt: config.prompt.clone(),
                tool_registry: config.tool_registry.clone(),
                task: task_info,
                sandbox: config.sandbox.clone(),
                run: RunInfo {
                    started: started_at,
                    finished: chrono::Utc::now().to_rfc3339(),
                    duration_seconds: duration_ms as f64 / 1000.0,
                    input_tokens: 0,
                    output_tokens: 0,
                    cached_tokens: 0,
                    cost_cny: 0.0,
                    steps: 0,
                    tool_calls: 0,
                    retries: 0,
                    patch_digest,
                    trajectory_digest,
                    grader_kind: task.grader.kind.clone(),
                    grader_version: config.grader_version.clone(),
                },
                outcome: OutcomeInfo {
                    status: status.into(),
                    fail_to_pass: 0,
                    pass_to_pass: 0,
                    failure_taxonomy: vec![if cancelled {
                        "cancelled"
                    } else {
                        "agent_driver_error"
                    }
                    .into()],
                    policy_violations: 0,
                },
            };
            write_json(
                &output_dir.join("report.json"),
                &report.to_json()?,
                "report.json",
            )?;
            return Err(format!("{status}: {failure_message}"));
        }
    };
    if !driver_outcome.cost_cny.is_finite() || driver_outcome.cost_cny < 0.0 {
        return Err("AgentDriver 返回了无效 cost_cny".into());
    }
    if driver_outcome.steps > task.limits.max_steps {
        return Err(format!(
            "AgentDriver 超过 max_steps：{} > {}",
            driver_outcome.steps, task.limits.max_steps
        ));
    }
    if driver_outcome.tool_calls > task.limits.max_tool_calls {
        return Err(format!(
            "AgentDriver 超过 max_tool_calls：{} > {}",
            driver_outcome.tool_calls, task.limits.max_tool_calls
        ));
    }
    if driver_outcome.cost_cny > task.limits.max_cost_cny {
        return Err(format!(
            "AgentDriver 超过 max_cost_cny：{:.4} > {:.4}",
            driver_outcome.cost_cny, task.limits.max_cost_cny
        ));
    }
    for event in &driver_outcome.trajectory {
        trajectory.append(event)?;
    }
    trajectory.append(&TrajectoryEvent {
        ts: chrono::Utc::now().to_rfc3339(),
        kind: "agent_finished".into(),
        fields: serde_json::json!({
            "steps": driver_outcome.steps,
            "tool_calls": driver_outcome.tool_calls,
        }),
    })?;
    let patch = collect_patch(&agent_ws, &task.repo.base_commit)?;
    write_json(&output_dir.join("model.patch"), &patch, "model.patch")?;
    let patch_digest = sha256_hex(patch.as_bytes());

    prepare_worktree(source_repo, &grader_ws, &task.repo.base_commit)?;
    apply_patch(&grader_ws, &patch)?;
    let grader_task_ws = task_workspace(&grader_ws, task)?;
    let grader = run_command_grader(&task.grader, &grader_task_ws)?;
    let grader_dir = output_dir.join("grader");
    std::fs::create_dir_all(&grader_dir)
        .map_err(|error| format!("创建 grader 日志目录失败：{error}"))?;
    write_json(
        &grader_dir.join("stdout.log"),
        &grader.stdout,
        "grader/stdout.log",
    )?;
    write_json(
        &grader_dir.join("stderr.log"),
        &grader.stderr,
        "grader/stderr.log",
    )?;
    trajectory.append(&TrajectoryEvent {
        ts: chrono::Utc::now().to_rfc3339(),
        kind: "grader_finished".into(),
        fields: serde_json::json!({
            "passed": grader.passed,
            "exit_code": grader.exit_code,
            "timed_out": grader.timed_out,
            "duration_ms": grader.duration_ms,
        }),
    })?;
    let (trajectory_events, raw_trajectory_digest) = trajectory.finish()?;
    let trajectory_digest = format!("sha256:{raw_trajectory_digest}");
    // 采集任务声明的产物（如 test-results/**），从 grader 干净工作树收集，保留相对结构。
    let collected_artifacts = collect_artifacts(&grader_task_ws, &task.artifacts, output_dir)?;

    let status = if grader.passed {
        OUTCOME_RESOLVED
    } else {
        OUTCOME_UNRESOLVED
    };
    let finished_at = chrono::Utc::now().to_rfc3339();
    let duration_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
    let report = EvalReport {
        schema_version: EVAL_REPORT_SCHEMA_VERSION,
        harness: config.harness.clone(),
        model: config.model.clone(),
        prompt: config.prompt.clone(),
        tool_registry: config.tool_registry.clone(),
        task: task_info,
        sandbox: config.sandbox.clone(),
        run: RunInfo {
            started: started_at,
            finished: finished_at,
            duration_seconds: duration_ms as f64 / 1000.0,
            input_tokens: driver_outcome.input_tokens,
            output_tokens: driver_outcome.output_tokens,
            cached_tokens: driver_outcome.cached_tokens,
            cost_cny: driver_outcome.cost_cny,
            steps: driver_outcome.steps,
            tool_calls: driver_outcome.tool_calls,
            retries: driver_outcome.retries,
            patch_digest: patch_digest.clone(),
            trajectory_digest: trajectory_digest.clone(),
            grader_kind: task.grader.kind.clone(),
            grader_version: config.grader_version.clone(),
        },
        outcome: OutcomeInfo {
            status: status.to_string(),
            fail_to_pass: u64::from(grader.passed),
            pass_to_pass: 0,
            failure_taxonomy: driver_outcome.failure_taxonomy,
            policy_violations: driver_outcome.policy_violations,
        },
    };
    write_json(
        &output_dir.join("report.json"),
        &report.to_json()?,
        "report.json",
    )?;
    Ok(EvalTrialOutcome {
        status: status.to_string(),
        patch,
        grader,
        collected_artifacts,
        duration_ms,
        trajectory_events,
        patch_digest,
        trajectory_digest,
    })
}

/// 测试桩：把 `a.txt` 改为 `fixed`，模拟 Agent 完成任务后的改动。
#[cfg(test)]
pub struct StubAgentDriver;

#[cfg(test)]
impl AsyncAgentDriver for StubAgentDriver {
    fn run_async<'a>(
        &'a self,
        _task: &'a EvalTask,
        workspace: &'a Path,
    ) -> Pin<Box<dyn Future<Output = Result<AgentDriverOutcome, AgentDriverError>> + Send + 'a>>
    {
        Box::pin(async move {
            std::fs::write(workspace.join("a.txt"), "fixed\n")
                .map_err(|error| format!("stub agent 写入失败：{error}"))?;
            Ok(AgentDriverOutcome {
                steps: 1,
                tool_calls: 1,
                trajectory: vec![TrajectoryEvent {
                    ts: "2026-09-05T00:00:00Z".into(),
                    kind: "tool_result".into(),
                    fields: serde_json::json!({"tool": "write_file", "ok": true}),
                }],
                ..Default::default()
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::eval_task::{EvalGrader, EvalLimits, EvalRepo};
    use std::fs;
    use std::process::Command;

    struct OverBudgetDriver;

    impl AsyncAgentDriver for OverBudgetDriver {
        fn run_async<'a>(
            &'a self,
            _task: &'a EvalTask,
            _workspace: &'a Path,
        ) -> Pin<Box<dyn Future<Output = Result<AgentDriverOutcome, AgentDriverError>> + Send + 'a>>
        {
            Box::pin(async {
                Ok(AgentDriverOutcome {
                    steps: 1,
                    tool_calls: 2,
                    ..Default::default()
                })
            })
        }
    }

    fn git(dir: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
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
                max_tool_calls: 100,
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
        let dir =
            std::env::temp_dir().join(format!("deveco-eval-run-src-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("a.txt"), "base\n").unwrap();
        git(&dir, &["init", "-q"]);
        git(&dir, &["add", "a.txt"]);
        git(
            &dir,
            &[
                "-c",
                "user.email=e@x",
                "-c",
                "user.name=t",
                "commit",
                "-q",
                "-m",
                "base",
            ],
        );
        let base = git(&dir, &["rev-parse", "HEAD"]);
        (dir, base)
    }

    fn run_config() -> EvalRunConfig {
        EvalRunConfig {
            run_id: format!("run-{}", uuid::Uuid::new_v4()),
            suite_version: "1".into(),
            grader_version: "command-v1".into(),
            harness: HarnessInfo {
                commit: "0123456789abcdef".into(),
                app_version: env!("CARGO_PKG_VERSION").into(),
                platform: std::env::consts::OS.into(),
            },
            model: ModelInfo {
                provider: "stub".into(),
                model_id: "deterministic-stub".into(),
                protocol: "in-process".into(),
                reasoning_effort: "none".into(),
            },
            prompt: PromptInfo {
                profile_version: "test-v1".into(),
                digest: sha256_hex(b"prompt"),
            },
            tool_registry: ToolRegistryInfo {
                version: "test-v1".into(),
                digest: sha256_hex(b"tools"),
            },
            sandbox: SandboxInfo {
                backend: "test-worktree".into(),
                capabilities: "workspace-write".into(),
                image_digest: None,
                network_policy: "none".into(),
            },
            request_timeout_seconds: None,
        }
    }

    #[test]
    fn request_timeout_seconds_is_validated_and_bounded() {
        let mut config = run_config();
        config.request_timeout_seconds = Some(120);
        assert!(validate_run_config(&config).is_ok());

        config.request_timeout_seconds = Some(0);
        assert!(validate_run_config(&config).is_err());
        config.request_timeout_seconds = Some(3601);
        assert!(validate_run_config(&config).is_err());
        config.request_timeout_seconds = Some(60);
        assert!(validate_run_config(&config).is_ok());
    }

    #[tokio::test]
    async fn request_timeout_exceeding_wall_time_is_rejected_before_worktree() {
        let mut config = run_config();
        config.request_timeout_seconds = Some(120);
        let task = task_with_grader(vec!["grep", "-q", "fixed", "a.txt"]);
        assert!(task.limits.wall_time_seconds < 120);
        let error = run_trial(
            &task,
            Path::new("/nonexistent-eval-source"),
            Path::new("/nonexistent-eval-out"),
            &StubAgentDriver,
            &config,
        )
        .await
        .unwrap_err();
        assert!(error.contains("request_timeout_seconds"), "{error}");
    }

    #[tokio::test]
    async fn run_trial_rejects_driver_outcome_over_tool_call_budget() {
        let (source, base) = source_repo_with_base();
        let mut task = task_with_grader(vec!["grep", "-q", "fixed", "a.txt"]);
        task.repo.base_commit = base;
        task.limits.max_tool_calls = 1;
        let output_dir = std::env::temp_dir().join(format!(
            "deveco-eval-tool-budget-out-{}",
            uuid::Uuid::new_v4()
        ));

        let error = run_trial(&task, &source, &output_dir, &OverBudgetDriver, &run_config())
            .await
            .unwrap_err();
        assert!(error.contains("超过 max_tool_calls"), "{error}");

        std::fs::remove_dir_all(output_dir).ok();
        std::fs::remove_dir_all(source).ok();
    }

    #[tokio::test]
    async fn run_trial_resolves_when_stub_fix_passes_grader() {
        let (source, base) = source_repo_with_base();
        let mut task = task_with_grader(vec!["grep", "-q", "fixed", "a.txt"]);
        task.repo.base_commit = base.clone();
        let output_dir =
            std::env::temp_dir().join(format!("deveco-eval-run-out-{}", uuid::Uuid::new_v4()));

        let outcome = run_trial(&task, &source, &output_dir, &StubAgentDriver, &run_config())
            .await
            .unwrap();
        assert_eq!(outcome.status, OUTCOME_RESOLVED);
        assert!(
            outcome.patch.contains("+fixed"),
            "patch 应含 fixed：{}",
            outcome.patch
        );
        assert!(output_dir.join("model.patch").exists());
        for path in [
            "manifest.json",
            "trajectory.jsonl",
            "report.json",
            "grader/stdout.log",
            "grader/stderr.log",
        ] {
            assert!(output_dir.join(path).exists(), "缺少评测产物 {path}");
        }
        assert_eq!(outcome.trajectory_events, 4);
        assert!(outcome.patch_digest.starts_with("sha256:"));
        assert!(outcome.trajectory_digest.starts_with("sha256:"));
        assert_eq!(
            outcome.patch_digest,
            sha256_hex(&fs::read(output_dir.join("model.patch")).unwrap())
        );
        assert_eq!(
            outcome.trajectory_digest,
            sha256_hex(&fs::read(output_dir.join("trajectory.jsonl")).unwrap())
        );
        let report: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(output_dir.join("report.json")).unwrap())
                .unwrap();
        assert_eq!(report["run"]["patch_digest"], outcome.patch_digest);
        assert_eq!(
            report["run"]["trajectory_digest"],
            outcome.trajectory_digest
        );

        fs::remove_dir_all(source).ok();
        fs::remove_dir_all(output_dir).ok();
    }

    #[tokio::test]
    async fn run_trial_unresolved_when_grader_fails() {
        let (source, base) = source_repo_with_base();
        // grader 要求 a.txt 含 "other"，但 stub 写的是 "fixed"
        let mut task = task_with_grader(vec!["grep", "-q", "other", "a.txt"]);
        task.repo.base_commit = base.clone();
        let output_dir =
            std::env::temp_dir().join(format!("deveco-eval-run-out2-{}", uuid::Uuid::new_v4()));

        let outcome = run_trial(&task, &source, &output_dir, &StubAgentDriver, &run_config())
            .await
            .unwrap();
        assert_eq!(outcome.status, OUTCOME_UNRESOLVED);
        assert!(!outcome.grader.passed);

        fs::remove_dir_all(source).ok();
        fs::remove_dir_all(output_dir).ok();
    }

    struct TerminalDriver(bool);

    impl AsyncAgentDriver for TerminalDriver {
        fn run_async<'a>(
            &'a self,
            _task: &'a EvalTask,
            _workspace: &'a Path,
        ) -> Pin<Box<dyn Future<Output = Result<AgentDriverOutcome, AgentDriverError>> + Send + 'a>>
        {
            Box::pin(async move {
                if self.0 {
                    Err(AgentDriverError::Cancelled("user stopped".into()))
                } else {
                    Err(AgentDriverError::Failed("provider unavailable".into()))
                }
            })
        }
    }

    #[tokio::test]
    async fn driver_failure_and_cancel_still_emit_complete_trial_bundle() {
        for (cancelled, expected) in [(false, OUTCOME_HARNESS_ERROR), (true, OUTCOME_CANCELLED)] {
            let (source, base) = source_repo_with_base();
            let mut task = task_with_grader(vec!["true"]);
            task.repo.base_commit = base;
            let output_dir =
                std::env::temp_dir().join(format!("deveco-eval-terminal-{}", uuid::Uuid::new_v4()));
            let error = run_trial(
                &task,
                &source,
                &output_dir,
                &TerminalDriver(cancelled),
                &run_config(),
            )
            .await
            .unwrap_err();
            assert!(error.starts_with(expected), "{error}");
            for path in [
                "manifest.json",
                "trajectory.jsonl",
                "model.patch",
                "report.json",
                "grader/stdout.log",
                "grader/stderr.log",
            ] {
                assert!(output_dir.join(path).exists(), "{expected} 缺少 {path}");
            }
            let report: serde_json::Value =
                serde_json::from_str(&fs::read_to_string(output_dir.join("report.json")).unwrap())
                    .unwrap();
            assert_eq!(report["outcome"]["status"], expected);
            fs::remove_dir_all(source).ok();
            fs::remove_dir_all(output_dir).ok();
        }
    }

    #[cfg(unix)]
    #[test]
    fn process_driver_uses_stdin_cwd_and_structured_stdout() {
        use std::os::unix::fs::PermissionsExt;
        let workspace =
            std::env::temp_dir().join(format!("deveco-process-driver-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&workspace).unwrap();
        let program = workspace.join("driver.sh");
        fs::write(&program, "#!/bin/sh\nread request\nprintf '%s' \"$request\" > received-task.json\nprintf '{\"steps\":2,\"tool_calls\":1}'\n").unwrap();
        let mut permissions = fs::metadata(&program).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&program, permissions).unwrap();
        let task = task_with_grader(vec!["true"]);
        let outcome = ProcessAgentDriver {
            program,
            args: vec![],
            timeout: Duration::from_secs(10),
        }
        .run(&task, &workspace)
        .unwrap();
        assert_eq!(outcome.steps, 2);
        assert_eq!(outcome.tool_calls, 1);
        let received: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(workspace.join("received-task.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(received["task_id"], task.task_id);
        fs::remove_dir_all(workspace).ok();
    }

    #[tokio::test]
    async fn run_trial_collects_declared_artifacts() {
        let (source, base) = source_repo_with_base();
        // 声明 a.txt 为产物：stub 把 a.txt 改为 fixed，patch 应用后 grader 工作树含 a.txt=fixed。
        let mut task = task_with_grader(vec!["grep", "-q", "fixed", "a.txt"]);
        task.repo.base_commit = base.clone();
        task.artifacts = vec!["a.txt".to_string()];
        let output_dir =
            std::env::temp_dir().join(format!("deveco-eval-run-out3-{}", uuid::Uuid::new_v4()));

        let outcome = run_trial(&task, &source, &output_dir, &StubAgentDriver, &run_config())
            .await
            .unwrap();
        assert_eq!(outcome.status, OUTCOME_RESOLVED);
        assert_eq!(outcome.collected_artifacts, vec!["a.txt".to_string()]);
        assert!(output_dir.join("artifacts/a.txt").exists());
        // 行尾归一到 LF：Windows 上可能写回 CRLF
        assert_eq!(
            fs::read_to_string(output_dir.join("artifacts/a.txt"))
                .unwrap()
                .replace("\r\n", "\n"),
            "fixed\n"
        );

        fs::remove_dir_all(source).ok();
        fs::remove_dir_all(output_dir).ok();
    }

    #[tokio::test]
    async fn run_trial_rejects_missing_reproducibility_fingerprints_before_workspace_changes() {
        let (source, base) = source_repo_with_base();
        let mut task = task_with_grader(vec!["true"]);
        task.repo.base_commit = base;
        let output_dir =
            std::env::temp_dir().join(format!("deveco-eval-run-invalid-{}", uuid::Uuid::new_v4()));
        let mut config = run_config();
        config.model.model_id.clear();
        let error = run_trial(&task, &source, &output_dir, &StubAgentDriver, &config)
            .await
            .unwrap_err();
        assert!(error.contains("model.model_id"), "{error}");
        assert!(!output_dir.exists(), "配置失败不应创建输出目录");
        fs::remove_dir_all(source).ok();
    }

    struct SubdirDriver;

    impl AsyncAgentDriver for SubdirDriver {
        fn run_async<'a>(
            &'a self,
            _task: &'a EvalTask,
            workspace: &'a Path,
        ) -> Pin<Box<dyn Future<Output = Result<AgentDriverOutcome, AgentDriverError>> + Send + 'a>>
        {
            Box::pin(async move {
                if workspace.file_name().and_then(|value| value.to_str()) != Some("package") {
                    return Err(format!("driver 未进入任务 subdir：{}", workspace.display()).into());
                }
                fs::write(workspace.join("a.txt"), "fixed\n").map_err(|error| error.to_string())?;
                Ok(AgentDriverOutcome::default())
            })
        }
    }

    #[tokio::test]
    async fn run_trial_scopes_driver_grader_and_artifacts_to_repo_subdir() {
        let source =
            std::env::temp_dir().join(format!("deveco-eval-subdir-src-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(source.join("package")).unwrap();
        fs::write(source.join("package/a.txt"), "base\n").unwrap();
        fs::write(source.join("root.txt"), "untouched\n").unwrap();
        git(&source, &["init", "-q"]);
        git(&source, &["add", "."]);
        git(
            &source,
            &[
                "-c",
                "user.email=e@x",
                "-c",
                "user.name=t",
                "commit",
                "-q",
                "-m",
                "base",
            ],
        );
        let base = git(&source, &["rev-parse", "HEAD"]);
        let mut task = task_with_grader(vec!["grep", "-q", "fixed", "a.txt"]);
        task.repo.base_commit = base;
        task.repo.subdir = Some("package".into());
        task.artifacts = vec!["a.txt".into()];
        let output_dir =
            std::env::temp_dir().join(format!("deveco-eval-subdir-out-{}", uuid::Uuid::new_v4()));

        let outcome = run_trial(&task, &source, &output_dir, &SubdirDriver, &run_config())
            .await
            .unwrap();
        assert_eq!(outcome.status, OUTCOME_RESOLVED);
        assert_eq!(outcome.collected_artifacts, vec!["a.txt"]);
        // 行尾归一到 LF：Windows 上可能写回 CRLF
        assert_eq!(
            fs::read_to_string(output_dir.join("artifacts/a.txt"))
                .unwrap()
                .replace("\r\n", "\n"),
            "fixed\n"
        );
        assert!(outcome.patch.contains("package/a.txt"));
        assert!(!outcome.patch.contains("root.txt"));

        fs::remove_dir_all(source).ok();
        fs::remove_dir_all(output_dir).ok();
    }
}
