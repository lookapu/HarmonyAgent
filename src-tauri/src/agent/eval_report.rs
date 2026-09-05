//! 真实 Agent 评测 report schema v1（docs/AGENT_EVAL_HARNESS.md §5 必填字段）。
//!
//! 与 `eval_task` 的“任务进”相对，这里定义“报告出”：一次 trial 的固定运行条件、
//! 模型/工具/prompt 指纹、沙箱边界、资源消耗与 grader 结论。报告字段由 runner 采集，
//! 与 manifest/trajectory 相互独立，不得相互替代。

use serde::Serialize;

pub const EVAL_REPORT_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize)]
pub struct EvalReport {
    pub schema_version: u32,
    pub harness: HarnessInfo,
    pub model: ModelInfo,
    pub prompt: PromptInfo,
    pub tool_registry: ToolRegistryInfo,
    pub task: TaskInfo,
    pub sandbox: SandboxInfo,
    pub run: RunInfo,
    pub outcome: OutcomeInfo,
}

#[derive(Debug, Clone, Serialize)]
pub struct HarnessInfo {
    pub commit: String,
    pub app_version: String,
    pub platform: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModelInfo {
    pub provider: String,
    pub model_id: String,
    pub protocol: String,
    pub reasoning_effort: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PromptInfo {
    pub profile_version: String,
    pub digest: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolRegistryInfo {
    pub version: String,
    pub digest: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct TaskInfo {
    pub suite: String,
    pub suite_version: String,
    pub task_id: String,
    pub repo_base_commit: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SandboxInfo {
    pub backend: String,
    pub capabilities: String,
    pub image_digest: Option<String>,
    pub network_policy: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RunInfo {
    pub started: String,
    pub finished: String,
    pub duration_seconds: f64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_tokens: u64,
    pub cost_cny: f64,
    pub steps: u64,
    pub tool_calls: u64,
    pub retries: u64,
    pub patch_digest: String,
    pub trajectory_digest: String,
    pub grader_kind: String,
    pub grader_version: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct OutcomeInfo {
    /// `resolved | unresolved | harness_error | cancelled`
    pub status: String,
    pub fail_to_pass: u64,
    pub pass_to_pass: u64,
    pub failure_taxonomy: Vec<String>,
    pub policy_violations: u64,
}

impl EvalReport {
    pub fn to_json(&self) -> Result<String, String> {
        serde_json::to_string_pretty(self).map_err(|error| format!("序列化报告失败：{error}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_report() -> EvalReport {
        EvalReport {
            schema_version: EVAL_REPORT_SCHEMA_VERSION,
            harness: HarnessInfo {
                commit: "8d5d8d9".into(),
                app_version: "v2.1.1".into(),
                platform: std::env::consts::OS.into(),
            },
            model: ModelInfo {
                provider: "test".into(),
                model_id: "pinned-model".into(),
                protocol: "chat".into(),
                reasoning_effort: "high".into(),
            },
            prompt: PromptInfo {
                profile_version: "1".into(),
                digest: "sha256:abc".into(),
            },
            tool_registry: ToolRegistryInfo {
                version: "1".into(),
                digest: "sha256:def".into(),
            },
            task: TaskInfo {
                suite: "harmonybench-smoke-v0".into(),
                suite_version: "1".into(),
                task_id: "smoke__example-1".into(),
                repo_base_commit: "0123456789abcdef0123456789abcdef01234567".into(),
            },
            sandbox: SandboxInfo {
                backend: "host-direct".into(),
                capabilities: "none".into(),
                image_digest: None,
                network_policy: "none".into(),
            },
            run: RunInfo {
                started: "2026-09-05T00:00:00Z".into(),
                finished: "2026-09-05T00:05:00Z".into(),
                duration_seconds: 300.0,
                input_tokens: 1000,
                output_tokens: 500,
                cached_tokens: 200,
                cost_cny: 1.5,
                steps: 12,
                tool_calls: 20,
                retries: 1,
                patch_digest: "sha256:patch".into(),
                trajectory_digest: "sha256:traj".into(),
                grader_kind: "command".into(),
                grader_version: "1".into(),
            },
            outcome: OutcomeInfo {
                status: "resolved".into(),
                fail_to_pass: 3,
                pass_to_pass: 10,
                failure_taxonomy: vec![],
                policy_violations: 0,
            },
        }
    }

    #[test]
    fn report_serializes_all_required_sections() {
        let json = sample_report().to_json().unwrap();
        for required in [
            "schema_version",
            "harness",
            "model",
            "prompt",
            "tool_registry",
            "task",
            "sandbox",
            "run",
            "outcome",
            "status",
            "fail_to_pass",
            "pass_to_pass",
            "policy_violations",
            "patch_digest",
            "trajectory_digest",
        ] {
            assert!(json.contains(required), "缺少必填字段 {required}：{json}");
        }
    }

    #[test]
    fn report_status_is_one_of_known_values() {
        let value: serde_json::Value =
            serde_json::from_str(&sample_report().to_json().unwrap()).unwrap();
        let status = value["outcome"]["status"].as_str().unwrap();
        assert!(
            matches!(status, "resolved" | "unresolved" | "harness_error" | "cancelled"),
            "未知 outcome 状态：{status}"
        );
    }
}
