//! 统一工具结果协议。验收、恢复、评测与 UI 都消费同一份机器可读证据，
//! 自然语言输出仅作为有界诊断附件，不能单独证明任务完成。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArtifactEvidence {
    pub path: String,
    pub kind: String,
    pub operation: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct VerificationEvidence {
    pub kind: String,
    pub passed: bool,
    pub detail: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolErrorEvidence {
    pub code: String,
    pub retryable: bool,
    pub category: String,
    pub message: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
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

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModificationEvidence {
    pub target: String,
    pub operation: String,
    pub effect_kind: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecoveryEvidence {
    pub policy: String,
    pub retry_safe: bool,
    pub compensation: Option<CompensationEvidence>,
    pub instruction: String,
}

fn protocol_v2() -> u32 { 2 }

/// 所有内置、MCP 与子 Agent 工具的统一结果契约。
///
/// `serde(default)` 可读取迁移前的不完整 V2 记录；`extensions` 保留未来新增字段，
/// 因而协议消费者无需与生产者同步升级。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolResultV2 {
    #[serde(default = "protocol_v2")]
    pub schema_version: u32,
    pub tool: String,
    pub status: String,
    pub summary: String,
    /// 对用户说明本次结果对真实状态的影响；旧 V2 记录缺失时由 serde default 兼容。
    pub impact: String,
    pub modifications: Vec<ModificationEvidence>,
    pub recovery: RecoveryEvidence,
    pub suggestions: Vec<String>,
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
    #[serde(flatten)]
    pub extensions: BTreeMap<String, serde_json::Value>,
}

/// 兼容内部旧调用名；序列化协议统一以 `ToolResultV2` 为准。
pub type ToolResultEnvelope = ToolResultV2;

impl ToolResultV2 {
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
        let committed = matches!(status, "ok" | "partial" | "partial_success");
        let mut artifacts = argument_artifacts(tool, args);
        merge_native_artifacts(output, &mut artifacts);
        merge_spilled_artifacts(output, &mut artifacts);
        let mut verification = Vec::new();
        if let Some(kind) = declared_validator(tool, args) {
            verification.push(VerificationEvidence {
                kind: kind.into(),
                passed: succeeded,
                detail: first_line(output),
            });
        }
        let side_effects = if contract.effect.as_str() == "read" || !committed {
            Vec::new()
        } else if artifacts.is_empty() {
            vec![format!("{} operation completed", contract.effect.as_str())]
        } else {
            artifacts
                .iter()
                .map(|artifact| format!("{}:{}", artifact.operation, artifact.path))
                .collect()
        };
        let error = (!succeeded && !matches!(status, "waiting_approval" | "pending_approval"))
            .then(|| classify_error(status, output, contract.retry_safe));
        let compensation = compensation(contract.recovery_action, succeeded, &artifacts);
        let modifications = if committed && contract.effect.as_str() != "read" {
            if artifacts.is_empty() {
                vec![ModificationEvidence {
                    target: tool.into(),
                    operation: "execute".into(),
                    effect_kind: contract.effect.as_str().into(),
                }]
            } else {
                artifacts.iter().map(|artifact| ModificationEvidence {
                    target: artifact.path.clone(),
                    operation: artifact.operation.clone(),
                    effect_kind: contract.effect.as_str().into(),
                }).collect()
            }
        } else {
            Vec::new()
        };
        let mut suggestions = result_suggestions(tool, status, output, error.as_ref(), compensation.as_ref());
        let recovery = RecoveryEvidence {
            policy: contract.recovery.as_str().into(),
            retry_safe: contract.retry_safe,
            compensation: compensation.clone(),
            instruction: recovery_instruction(contract.recovery.as_str(), error.as_ref(), compensation.as_ref()),
        };
        if !succeeded && !suggestions.contains(&recovery.instruction) {
            suggestions.push(recovery.instruction.clone());
        }
        suggestions.truncate(8);
        let metrics = ToolMetrics {
            duration_ms: duration_ms.max(0),
            output_chars: output.chars().count(),
            artifact_count: artifacts.len(),
        };
        let normalized = normalized_status(status, &verification, error.as_ref());
        Self {
            schema_version: 2,
            tool: tool.into(),
            status: normalized.into(),
            summary: first_line(output),
            impact: result_impact(status, contract.effect.as_str(), &modifications),
            modifications,
            recovery,
            suggestions,
            effect_kind: contract.effect.as_str().into(),
            recovery_policy: contract.recovery.as_str().into(),
            retry_safe: contract.retry_safe,
            artifacts,
            side_effects,
            verification,
            raw_excerpt: output.chars().take(1000).collect(),
            outcome: normalized.into(),
            error,
            compensation,
            metrics,
            extensions: BTreeMap::new(),
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

fn result_impact(status: &str, effect_kind: &str, modifications: &[ModificationEvidence]) -> String {
    if matches!(status, "partial" | "partial_success") {
        return format!("已完成 {} 项修改，其余步骤未确认完成", modifications.len());
    }
    if status == "ok" {
        return if effect_kind == "read" {
            "只读取真实状态，未产生外部修改".into()
        } else {
            format!("已提交 {} 项可追踪修改或副作用", modifications.len().max(1))
        };
    }
    if effect_kind == "read" {
        "读取未完成，未提交外部状态变更".into()
    } else {
        "副作用结果未完全确认；重试前必须按恢复指引核验真实状态".into()
    }
}

fn classify_error(status: &str, output: &str, retry_safe: bool) -> ToolErrorEvidence {
    let lower = output.to_lowercase();
    let (code, category, transient) = if status == "cancelled" {
        ("TOOL_CANCELLED", "cancelled", false)
    } else if lower.contains("参数未通过 schema 校验")
        || lower.contains("argument schema validation")
    {
        ("TOOL_ARGUMENT_INVALID", "argument", false)
    } else if lower.contains("panic") {
        ("TOOL_WORKER_PANIC", "worker_crash", false)
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

fn normalized_status(
    status: &str,
    verification: &[VerificationEvidence],
    error: Option<&ToolErrorEvidence>,
) -> &'static str {
    match status {
        "ok" if verification.iter().any(|item| !item.passed) => "verification_failed",
        "ok" => "succeeded",
        "partial" | "partial_success" => "partial_success",
        "verification_failed" => "verification_failed",
        "waiting_approval" | "pending_approval" => "waiting_approval",
        "cancelled" => "cancelled",
        _ if error.is_some_and(|item| item.retryable) => "retryable_failure",
        _ => "permanent_failure",
    }
}

fn result_suggestions(
    tool: &str,
    status: &str,
    output: &str,
    error: Option<&ToolErrorEvidence>,
    compensation: Option<&CompensationEvidence>,
) -> Vec<String> {
    let mut items = Vec::new();
    if status == "waiting_approval" || status == "pending_approval" {
        items.push("等待用户审批；审批前不要执行该副作用操作".into());
    }
    if let Some(advice) = crate::agent::tools::diagnose_tool_error(tool, output) {
        items.push(advice.into());
    }
    if error.is_some_and(|item| item.retryable) {
        items.push("确认环境状态后，可使用相同幂等参数重试一次".into());
    }
    if let Some(item) = compensation {
        items.push(item.instruction.clone());
    }
    items.sort();
    items.dedup();
    items.truncate(8);
    items
}

fn recovery_instruction(
    policy: &str,
    error: Option<&ToolErrorEvidence>,
    compensation: Option<&CompensationEvidence>,
) -> String {
    if let Some(item) = compensation {
        return item.instruction.clone();
    }
    if error.is_some_and(|item| item.retryable) {
        return "确认调用未产生副作用后，允许按原幂等键重试".into();
    }
    match policy {
        "replay" => "该只读调用可安全重放".into(),
        "verify" => "先读取真实状态确认副作用，再决定继续或补偿".into(),
        _ => "需要人工核对真实状态，不得自动重放".into(),
    }
}

fn compensation(
    action: crate::agent::tools::contracts::RecoveryAction,
    succeeded: bool,
    artifacts: &[ArtifactEvidence],
) -> Option<CompensationEvidence> {
    use crate::agent::tools::contracts::RecoveryAction;
    if !succeeded || action == RecoveryAction::None {
        return None;
    }
    let (strategy, approval, instruction) = match action {
        RecoveryAction::RestoreSnapshot => (
            "restore_snapshot",
            false,
            "Restore the pre-tool file snapshot",
        ),
        RecoveryAction::GitRevert => ("git_revert", true, "Create a compensating revert commit"),
        RecoveryAction::RedeployPrevious => (
            "redeploy_previous",
            true,
            "Verify device state and redeploy the previous artifact",
        ),
        RecoveryAction::VerifyThenCompensate if !artifacts.is_empty() => (
            "verify_then_compensate",
            true,
            "Verify external state before applying a compensating action",
        ),
        RecoveryAction::VerifyThenCompensate => (
            "verify_then_compensate",
            true,
            "Verify external state before applying a compensating action",
        ),
        RecoveryAction::ManualReview => (
            "manual_review",
            true,
            "Inspect the real external state and follow the tool-specific recovery guide",
        ),
        RecoveryAction::None => return None,
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

fn merge_spilled_artifacts(output: &str, artifacts: &mut Vec<ArtifactEvidence>) {
    for marker in ["已完整保存到 ", "完整内容已保存到 "] {
        let mut rest = output;
        while let Some(start) = rest.find(marker) {
            let after = &rest[start + marker.len()..];
            let end = after
                .find(['；', '，', '\n', '\r'])
                .unwrap_or(after.len());
            let path = after[..end].trim();
            if !path.is_empty()
                && (path.contains(".deveco-agent/spill/")
                    || path.contains(".deveco-agent/tool-output/"))
            {
                artifacts.push(ArtifactEvidence {
                    path: path.replace('\\', "/"),
                    kind: "tool_output".into(),
                    operation: "produce".into(),
                });
            }
            rest = &after[end..];
        }
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

fn declared_validator(tool: &str, args: &str) -> Option<&'static str> {
    use crate::agent::tools::contracts::ValidatorKind;
    let validator = crate::agent::tools::contracts::contract(tool).validator?;
    if validator == ValidatorKind::Command
        && !["test", "build", "cargo check", "git diff", "git status"]
            .iter()
            .any(|word| args.to_lowercase().contains(word))
    {
        return None;
    }
    Some(validator.as_str())
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
        assert_eq!(value.status, "succeeded");
        assert_eq!(value.modifications[0].target, "src/a.rs");
        assert_eq!(value.recovery.policy, "verify");
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
        assert_eq!(value.status, "permanent_failure");
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

    #[test]
    fn spilled_output_is_a_first_class_artifact() {
        let value = ToolResultV2::from_execution(
            "run_tests",
            "{}",
            "preview\n…(输出过长，已完整保存到 .deveco-agent/spill/tests.txt；可读取)…\nfailed",
            "error",
        );
        assert!(value.artifacts.iter().any(|item| {
            item.kind == "tool_output" && item.path == ".deveco-agent/spill/tests.txt"
        }));
        assert!(value.raw_excerpt.chars().count() <= 1000);
    }

    #[test]
    fn validators_and_recovery_actions_come_from_tool_contracts() {
        let test = ToolResultV2::from_execution(
            "run_command", r#"{"command":"cargo test"}"#, "ok", "ok",
        );
        assert_eq!(test.verification[0].kind, "command");
        assert_eq!(test.recovery.compensation.as_ref().unwrap().strategy, "manual_review");

        let arbitrary = ToolResultV2::from_execution(
            "run_command", r#"{"command":"generate assets"}"#, "ok", "ok",
        );
        assert!(arbitrary.verification.is_empty());
        assert!(!arbitrary.recovery.instruction.is_empty());
    }

    #[test]
    fn worker_panic_is_non_retryable_and_machine_readable() {
        let value = ToolResultEnvelope::from_execution(
            "read_file", "{}", "工具执行器发生 panic，已隔离当前调用", "error",
        );
        let error = value.error.unwrap();
        assert_eq!(error.code, "TOOL_WORKER_PANIC");
        assert!(!error.retryable);
    }

    #[test]
    fn schema_validation_failure_has_stable_argument_error_code() {
        let value = ToolResultEnvelope::from_execution(
            "read_file", "{}", "工具 `read_file` 参数未通过 schema 校验，本次未执行", "error",
        );
        let error = value.error.unwrap();
        assert_eq!(error.code, "TOOL_ARGUMENT_INVALID");
        assert_eq!(error.category, "argument");
        assert!(!error.retryable);
    }

    #[test]
    fn v2_distinguishes_partial_approval_and_retryable_failures() {
        let partial = ToolResultV2::from_execution(
            "edit_file", r#"{"path":"src/a.rs"}"#, "one edit applied", "partial",
        );
        assert_eq!(partial.status, "partial_success");
        assert_eq!(partial.modifications.len(), 1);

        let approval = ToolResultV2::from_execution(
            "git_push", "{}", "waiting for approval", "waiting_approval",
        );
        assert_eq!(approval.status, "waiting_approval");
        assert!(approval.error.is_none());
        assert!(!approval.suggestions.is_empty());

        let retryable = ToolResultV2::from_execution(
            "web_fetch", "{}", "network timeout", "error",
        );
        assert_eq!(retryable.status, "retryable_failure");
        assert!(!retryable.suggestions.is_empty());
    }

    #[test]
    fn v2_reads_legacy_records_and_preserves_unknown_fields() {
        let value: ToolResultV2 = serde_json::from_value(serde_json::json!({
            "schema_version": 2,
            "tool": "read_file",
            "status": "ok",
            "summary": "legacy",
            "future_evidence": {"version": 3}
        })).unwrap();
        assert!(value.modifications.is_empty());
        assert_eq!(value.extensions["future_evidence"]["version"], 3);
        let encoded = serde_json::to_value(value).unwrap();
        assert_eq!(encoded["future_evidence"]["version"], 3);
    }

    #[test]
    fn every_registered_tool_emits_complete_v2_shape() {
        for spec in crate::agent::tools::TOOL_SPECS {
            let value = serde_json::to_value(ToolResultV2::from_execution(
                spec.name, "{}", "ok", "ok",
            )).unwrap();
            for field in [
                "status", "impact", "modifications", "artifacts", "verification", "recovery",
                "suggestions", "error",
            ] {
                assert!(value.get(field).is_some(), "{} 缺少 {field}", spec.name);
            }
        }
    }

    #[test]
    fn high_frequency_tools_cover_success_failure_timeout_cancel_retry_and_recovery_protocols() {
        let tools = [
            "read_file", "list_dir", "grep_files", "codebase_search", "edit_file", "write_file",
            "run_command", "build_project", "run_tests", "git_status", "git_diff", "list_devices",
        ];
        for tool in tools {
            let success = ToolResultV2::from_execution(tool, "{}", "ok", "ok");
            assert_eq!(success.status, "succeeded", "{tool}");
            assert!(!success.impact.is_empty(), "{tool}");

            let failure = ToolResultV2::from_execution(tool, "{}", "permanent failure", "error");
            assert!(failure.error.is_some(), "{tool}");
            assert!(!failure.recovery.instruction.is_empty(), "{tool}");
            assert!(!failure.suggestions.is_empty(), "{tool}");

            let timeout = ToolResultV2::from_execution(tool, "{}", "tool timeout", "error");
            assert_eq!(timeout.error.as_ref().unwrap().code, "TOOL_TIMEOUT", "{tool}");
            assert_eq!(timeout.error.as_ref().unwrap().retryable, timeout.retry_safe, "{tool}");

            let cancelled = ToolResultV2::from_execution(
                tool, "{}", "用户已停止当前工具", "cancelled",
            );
            assert_eq!(cancelled.status, "cancelled", "{tool}");
            assert_eq!(cancelled.error.as_ref().unwrap().code, "TOOL_CANCELLED", "{tool}");
            assert!(!cancelled.recovery.instruction.is_empty(), "{tool}");
        }
    }
}
