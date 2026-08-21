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
}

impl ToolResultEnvelope {
    pub fn from_execution(tool: &str, args: &str, output: &str, status: &str) -> Self {
        let contract = crate::agent::tools::contracts::contract(tool);
        let succeeded = status == "ok";
        let artifacts = argument_artifacts(tool, args);
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
        Self {
            schema_version: 1,
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
        }
    }

    pub fn digest(&self) -> String {
        let bytes = serde_json::to_vec(self).unwrap_or_default();
        let mut digest = Sha256::new();
        digest.update(bytes);
        format!("{:x}", digest.finalize())
    }
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
        assert_eq!(value.digest(), value.digest());
    }

    #[test]
    fn failed_test_is_structured_verification() {
        let value = ToolResultEnvelope::from_execution("run_tests", "{}", "2 failed", "error");
        assert!(!value.verification[0].passed);
        assert!(value.side_effects.is_empty());
    }
}
