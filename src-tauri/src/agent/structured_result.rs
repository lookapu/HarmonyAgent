//! 统一工具结果协议。验收、恢复、评测与 UI 都消费同一份机器可读证据，
//! 自然语言输出仅作为有界诊断附件，不能单独证明任务完成。

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArtifactEvidence {
    pub path: String,
    pub kind: String,
    pub operation: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct VerificationEvidence {
    pub kind: String,
    pub passed: bool,
    pub detail: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolErrorEvidence {
    pub code: String,
    pub retryable: bool,
    pub category: String,
    pub message: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CompensationEvidence {
    pub strategy: String,
    pub requires_approval: bool,
    pub instruction: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ToolMetrics {
    pub duration_ms: i64,
    pub output_chars: usize,
    pub artifact_count: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolResultEnvelope {
    pub schema_version: u32,
    pub tool: String,
    pub status: String,
    pub summary: String,
    pub effect_kind: String,
    pub recovery_policy: String,
    pub retry_safe: bool,
    pub artifacts: Vec<ArtifactEvidence>,
    pub side_effects: Vec<String>,
    pub verification: Vec<VerificationEvidence>,
    pub raw_excerpt: String,
    pub outcome: String,
    pub error: Option<ToolErrorEvidence>,
    pub compensation: Option<CompensationEvidence>,
    pub metrics: ToolMetrics,
}

impl ToolResultEnvelope {
    pub fn from_execution(tool: &str, args: &str, output: &str, status: &str) -> Self {
        Self::from_execution_with_metrics(tool, args, output, status, 0)
    }

    pub fn from_execution_with_metrics(
        tool: &str,
        args: &str,
        output: &str,
        status: &str,
        duration_ms: i64,
    ) -> Self {
        let contract = crate::agent::tools::contracts::contract(tool);
        let succeeded = status == "ok";
        let mut artifacts = argument_artifacts(tool, args);
        merge_native_artifacts(output, &mut artifacts);
        let mut verification = Vec::new();
        if is_verifier(tool, args) {
            verification.push(VerificationEvidence {
                kind: verification_kind(tool).into(),
                passed: succeeded,
                detail: first_line(output),
            });
        }
        let side_effects = if contract.effect.as_str() == "read" || !succeeded {
            Vec::new()
        } else if artifacts.is_empty() {
            vec![format!("{} operation completed", contract.effect.as_str())]
        } else {
            artifacts
                .iter()
                .map(|artifact| format!("{}:{}", artifact.operation, artifact.path))
                .collect()
        };
        let error = (!succeeded).then(|| classify_error(status, output, contract.retry_safe));
        let compensation = compensation(tool, contract.recovery.as_str(), succeeded, &artifacts);
        let metrics = ToolMetrics {
            duration_ms: duration_ms.max(0),
            output_chars: output.chars().count(),
            artifact_count: artifacts.len(),
        };
        Self {
            schema_version: 2,
            tool: tool.into(),
            status: status.into(),
            summary: first_line(output),
            effect_kind: contract.effect.as_str().into(),
            recovery_policy: contract.recovery.as_str().into(),
            retry_safe: contract.retry_safe,
            artifacts,
            side_effects,
            verification,
            raw_excerpt: output.chars().take(1000).collect(),
            outcome: if succeeded {
                "succeeded"
            } else if status == "cancelled" {
                "cancelled"
            } else {
                "failed"
            }
            .into(),
            error,
            compensation,
            metrics,
        }
    }

    pub fn digest(&self) -> String {
        let mut canonical = self.clone();
        // 证据身份必须与机器快慢无关，否则相同副作用在不同耗时下无法去重。
        canonical.metrics.duration_ms = 0;
        let bytes = serde_json::to_vec(&canonical).unwrap_or_default();
        let mut digest = Sha256::new();
        digest.update(bytes);
        format!("{:x}", digest.finalize())
    }
}

fn classify_error(status: &str, output: &str, retry_safe: bool) -> ToolErrorEvidence {
    let lower = output.to_lowercase();
    let (code, category, transient) = if status == "cancelled" {
        ("TOOL_CANCELLED", "cancelled", false)
    } else if lower.contains("timeout") || lower.contains("超时") {
        ("TOOL_TIMEOUT", "timeout", true)
    } else if lower.contains("permission") || lower.contains("权限") {
        ("TOOL_PERMISSION_DENIED", "permission", false)
    } else if lower.contains("not found") || lower.contains("不存在") {
        ("TOOL_NOT_FOUND", "not_found", false)
    } else if lower.contains("network") || lower.contains("connection") || lower.contains("网络")
    {
        ("TOOL_NETWORK", "network", true)
    } else if status == "blocked" {
        ("TOOL_POLICY_BLOCKED", "policy", false)
    } else {
        ("TOOL_EXECUTION_FAILED", "execution", false)
    };
    ToolErrorEvidence {
        code: code.into(),
        retryable: retry_safe && transient,
        category: category.into(),
        message: first_line(output),
    }
}

fn compensation(
    tool: &str,
    recovery: &str,
    succeeded: bool,
    artifacts: &[ArtifactEvidence],
) -> Option<CompensationEvidence> {
    if !succeeded || recovery == "replay" {
        return None;
    }
    let (strategy, approval, instruction) = match tool {
        "write_file" | "edit_file" | "apply_patch" | "delete_file" => (
            "restore_snapshot",
            false,
            "Restore the pre-tool file snapshot",
        ),
        "git_commit" => ("git_revert", true, "Create a compensating revert commit"),
        "deploy" | "deploy_all" => (
            "redeploy_previous",
            true,
            "Verify device state and redeploy the previous artifact",
        ),
        _ if !artifacts.is_empty() => (
            "verify_then_compensate",
            true,
            "Verify external state before applying a compensating action",
        ),
        _ => return None,
    };
    Some(CompensationEvidence {
        strategy: strategy.into(),
        requires_approval: approval,
        instruction: instruction.into(),
    })
}

fn merge_native_artifacts(output: &str, artifacts: &mut Vec<ArtifactEvidence>) {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(output) else {
        return;
    };
    let Some(items) = value.get("artifacts").and_then(|value| value.as_array()) else {
        return;
    };
    for item in items {
        let Some(path) = item.get("path").and_then(|value| value.as_str()) else {
            continue;
        };
        artifacts.push(ArtifactEvidence {
            path: path.replace('\\', "/"),
            kind: item
                .get("kind")
                .and_then(|v| v.as_str())
                .unwrap_or_else(|| artifact_kind(path))
                .into(),
            operation: item
                .get("operation")
                .and_then(|v| v.as_str())
                .unwrap_or("produce")
                .into(),
        });
    }
    artifacts.sort_by(|a, b| a.path.cmp(&b.path).then(a.operation.cmp(&b.operation)));
    artifacts.dedup_by(|a, b| a.path == b.path && a.operation == b.operation);
    artifacts.truncate(64);
}

fn first_line(output: &str) -> String {
    output
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("")
        .trim()
        .chars()
        .take(300)
        .collect()
}

fn argument_artifacts(tool: &str, args: &str) -> Vec<ArtifactEvidence> {
    fn walk(value: &serde_json::Value, key: &str, out: &mut Vec<String>) {
        match value {
            serde_json::Value::String(text) => {
                let key = key.to_lowercase();
                if (key.contains("path") || key.contains("file") || key == "hap")
                    && !text.trim().is_empty()
                {
                    out.push(text.replace('\\', "/"));
                }
            }
            serde_json::Value::Array(items) => items.iter().for_each(|item| walk(item, key, out)),
            serde_json::Value::Object(map) => {
                map.iter().for_each(|(key, value)| walk(value, key, out))
            }
            _ => {}
        }
    }
    let mut paths = Vec::new();
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(args) {
        walk(&value, "", &mut paths);
    }
    if paths.is_empty()
        && (args.contains('/') || args.contains('\\') || args.rsplit_once('.').is_some())
    {
        paths.push(args.trim().to_string());
    }
    paths.sort();
    paths.dedup();
    let operation = match tool {
        "delete_file" => "delete",
        "write_file" | "edit_file" | "apply_patch" | "create_project" => "write",
        _ if crate::agent::tools::contracts::contract(tool)
            .effect
            .as_str()
            == "read" =>
        {
            "read"
        }
        _ => "produce",
    };
    paths
        .into_iter()
        .take(64)
        .map(|path| ArtifactEvidence {
            kind: artifact_kind(&path).into(),
            path,
            operation: operation.into(),
        })
        .collect()
}

fn artifact_kind(path: &str) -> &'static str {
    let lower = path.to_lowercase();
    if lower.ends_with(".hap") || lower.ends_with(".app") || lower.ends_with(".exe") {
        "binary"
    } else if lower.ends_with(".json") || lower.ends_with(".json5") || lower.ends_with(".toml") {
        "config"
    } else if lower.ends_with(".log") {
        "log"
    } else {
        "file"
    }
}

fn is_verifier(tool: &str, args: &str) -> bool {
    matches!(
        tool,
        "read_file"
            | "git_diff"
            | "git_status"
            | "build_project"
            | "build_generic"
            | "run_tests"
            | "test_project"
            | "deploy"
    ) || (tool == "run_command"
        && ["test", "build", "cargo check", "git diff", "git status"]
            .iter()
            .any(|word| args.to_lowercase().contains(word)))
}

fn verification_kind(tool: &str) -> &'static str {
    match tool {
        "run_tests" | "test_project" => "tests",
        "build_project" | "build_generic" => "build",
        "deploy" => "deploy",
        "git_diff" | "git_status" => "diff",
        "read_file" => "artifact_read",
        _ => "command",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_result_has_artifact_and_stable_digest() {
        let value = ToolResultEnvelope::from_execution(
            "edit_file",
            r#"{"path":"src/a.rs","new":"x"}"#,
            "updated",
            "ok",
        );
        assert_eq!(value.artifacts[0].path, "src/a.rs");
        assert_eq!(value.side_effects, vec!["write:src/a.rs"]);
        assert_eq!(value.schema_version, 2);
        assert_eq!(
            value.compensation.as_ref().unwrap().strategy,
            "restore_snapshot"
        );
        assert_eq!(value.digest(), value.digest());
        let slower = ToolResultEnvelope::from_execution_with_metrics(
            "edit_file",
            r#"{"path":"src/a.rs","new":"x"}"#,
            "updated",
            "ok",
            9_999,
        );
        assert_eq!(value.digest(), slower.digest());
    }

    #[test]
    fn failed_test_is_structured_verification() {
        let value = ToolResultEnvelope::from_execution("run_tests", "{}", "2 failed", "error");
        assert!(!value.verification[0].passed);
        assert!(value.side_effects.is_empty());
        assert_eq!(value.error.as_ref().unwrap().code, "TOOL_EXECUTION_FAILED");
    }

    #[test]
    fn retryability_and_native_artifacts_are_machine_readable() {
        let timeout =
            ToolResultEnvelope::from_execution("read_file", "{}", "network timeout", "error");
        assert!(timeout.error.unwrap().retryable);
        let native = ToolResultEnvelope::from_execution(
            "build_generic",
            "{}",
            r#"{"artifacts":[{"path":"dist/app.exe","kind":"binary"}]}"#,
            "ok",
        );
        assert_eq!(native.artifacts[0].path, "dist/app.exe");
    }
}
