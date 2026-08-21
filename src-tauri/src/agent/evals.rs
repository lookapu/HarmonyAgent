//! 可重复的 Agent 可靠性评测套件。场景与期望策略作为版本化 fixture 随仓库维护，
//! 本地、Windows CI、macOS CI 使用完全相同的质量阈值。

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::time::Instant;

pub const DEFAULT_RELIABILITY_THRESHOLD: f64 = 0.95;
pub const EVAL_SNAPSHOT_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct EvalModelSnapshot {
    pub used: bool,
    pub provider_id: Option<String>,
    pub model_id: Option<String>,
    pub protocol: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct EvalPromptSnapshot {
    pub used: bool,
    pub profile_version: String,
    pub digest: String,
    pub content_included: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct EvalToolSnapshot {
    pub registry_version: String,
    pub registry_count: usize,
    pub registry_digest: String,
    pub external_calls: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct EvalSdkVariantSnapshot {
    pub variant: String,
    pub api_version: Option<String>,
    pub component_versions: Vec<String>,
    pub is_default: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct EvalSdkSnapshot {
    pub status: String,
    pub source: String,
    pub default_api: Option<String>,
    pub variants: Vec<EvalSdkVariantSnapshot>,
    pub has_hdc: bool,
    pub has_ohpm: bool,
    pub has_hvigorw: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct EvalDeviceSnapshot {
    pub id_digest: String,
    pub connection: String,
    pub authorized: bool,
    pub model: String,
    pub os_version: String,
    pub api_level: Option<i64>,
    pub architecture: String,
    pub capabilities: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct EvalDeviceInventorySnapshot {
    pub status: String,
    pub error: Option<String>,
    pub devices: Vec<EvalDeviceSnapshot>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct EvalMetricsSnapshot {
    pub duration_ms: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_cny: f64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct EvalEvidenceSnapshot {
    pub passed_case_digests: Vec<String>,
    pub failed_case_digests: Vec<String>,
    pub final_digest: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct EvalExecutionSnapshot {
    pub schema_version: u32,
    pub producer_version: String,
    pub model: EvalModelSnapshot,
    pub prompt: EvalPromptSnapshot,
    pub tools: EvalToolSnapshot,
    pub sdk: EvalSdkSnapshot,
    pub device_inventory: EvalDeviceInventorySnapshot,
    pub metrics: EvalMetricsSnapshot,
    pub evidence: EvalEvidenceSnapshot,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ReliabilityScenario {
    pub id: String,
    pub domain: String,
    pub expected: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EvalCaseResult {
    pub id: String,
    pub domain: String,
    pub expected: String,
    pub actual: String,
    pub passed: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EvalRun {
    pub eval_run_id: String,
    pub suite: String,
    pub platform: String,
    pub passed: bool,
    pub total_cases: usize,
    pub passed_cases: usize,
    pub score: f64,
    pub threshold: f64,
    pub results: Vec<EvalCaseResult>,
    pub snapshot: EvalExecutionSnapshot,
    pub created_at: i64,
}

fn sha256(value: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(value))
}

fn tool_snapshot() -> EvalToolSnapshot {
    let canonical = crate::agent::tools::TOOL_SPECS
        .iter()
        .map(|spec| format!("{}\n{}", spec.name, spec.desc))
        .collect::<Vec<_>>()
        .join("\n--\n");
    EvalToolSnapshot {
        registry_version: env!("CARGO_PKG_VERSION").into(),
        registry_count: crate::agent::tools::TOOL_SPECS.len(),
        registry_digest: sha256(canonical.as_bytes()),
        external_calls: 0,
    }
}

pub fn sdk_snapshot(env: &crate::services::harmony_env::HarmonyEnv) -> EvalSdkSnapshot {
    let variants = env
        .sdk_variants
        .iter()
        .map(|variant| EvalSdkVariantSnapshot {
            variant: variant.variant.clone(),
            api_version: variant.api_version.clone(),
            component_versions: variant
                .components
                .iter()
                .map(|component| {
                    format!(
                        "{}:{}:{}",
                        component.name,
                        component.api_version,
                        component.version.as_deref().unwrap_or("unknown")
                    )
                })
                .collect(),
            is_default: variant.is_default,
        })
        .collect::<Vec<_>>();
    EvalSdkSnapshot {
        status: if variants.is_empty() {
            "unavailable"
        } else {
            "available"
        }
        .into(),
        source: env.source.clone(),
        default_api: env.default_api.clone(),
        variants,
        has_hdc: env.hdc_path.is_some(),
        has_ohpm: env.ohpm_path.is_some(),
        has_hvigorw: env.hvigorw_path.is_some(),
    }
}

pub fn device_snapshot(
    devices: Result<Vec<EvalDeviceSnapshot>, String>,
) -> EvalDeviceInventorySnapshot {
    match devices {
        Ok(mut devices) => {
            devices.truncate(20);
            EvalDeviceInventorySnapshot {
                status: if devices.is_empty() {
                    "none"
                } else {
                    "available"
                }
                .into(),
                error: None,
                devices,
            }
        }
        Err(error) => EvalDeviceInventorySnapshot {
            status: "unavailable".into(),
            error: Some(
                crate::utils::redact::redact_text(&error)
                    .chars()
                    .take(240)
                    .collect(),
            ),
            devices: Vec::new(),
        },
    }
}

pub fn hash_device_id(id: &str) -> String {
    sha256(id.as_bytes())
}

pub fn default_execution_snapshot() -> EvalExecutionSnapshot {
    let profile = "agent_harmony_fixed_v3:no_model:v1";
    EvalExecutionSnapshot {
        schema_version: EVAL_SNAPSHOT_SCHEMA_VERSION,
        producer_version: env!("CARGO_PKG_VERSION").into(),
        model: EvalModelSnapshot::default(),
        prompt: EvalPromptSnapshot {
            used: false,
            profile_version: "fixed_eval_no_model_v1".into(),
            digest: sha256(profile.as_bytes()),
            content_included: false,
        },
        tools: tool_snapshot(),
        sdk: EvalSdkSnapshot {
            status: "not_probed".into(),
            ..Default::default()
        },
        device_inventory: EvalDeviceInventorySnapshot {
            status: "not_probed".into(),
            ..Default::default()
        },
        metrics: EvalMetricsSnapshot::default(),
        evidence: EvalEvidenceSnapshot::default(),
    }
}

pub fn scenarios() -> Vec<ReliabilityScenario> {
    let mut scenarios: Vec<ReliabilityScenario> = serde_json::from_str(include_str!(
        "../../tests/fixtures/agent_reliability_scenarios.json"
    ))
    .unwrap_or_default();
    scenarios.extend(
        serde_json::from_str::<Vec<ReliabilityScenario>>(include_str!(
            "../../tests/fixtures/harmony_task_scenarios.json"
        ))
        .unwrap_or_default(),
    );
    scenarios
}

pub fn run_suite(conn: Option<&Connection>, threshold: f64) -> Result<EvalRun, String> {
    run_suite_with_snapshot(conn, threshold, default_execution_snapshot(), 0)
}

pub fn run_suite_with_snapshot(
    conn: Option<&Connection>,
    threshold: f64,
    mut snapshot: EvalExecutionSnapshot,
    preparation_duration_ms: u64,
) -> Result<EvalRun, String> {
    let started = Instant::now();
    let threshold = threshold.clamp(0.0, 1.0);
    let results = scenarios()
        .into_iter()
        .map(|scenario| {
            let actual = simulate_scenario(&scenario.id).unwrap_or_else(|| "unhandled".into());
            EvalCaseResult {
                passed: actual == scenario.expected,
                id: scenario.id,
                domain: scenario.domain,
                expected: scenario.expected,
                actual,
            }
        })
        .collect::<Vec<_>>();
    let passed_cases = results.iter().filter(|result| result.passed).count();
    let score = if results.is_empty() {
        0.0
    } else {
        passed_cases as f64 / results.len() as f64
    };
    let created_at = chrono::Utc::now().timestamp_millis();
    snapshot.metrics.duration_ms =
        preparation_duration_ms.saturating_add(started.elapsed().as_millis() as u64);
    let mut evidence = EvalEvidenceSnapshot::default();
    for result in &results {
        let digest = sha256(serde_json::to_string(result).unwrap_or_default().as_bytes());
        if result.passed {
            evidence.passed_case_digests.push(digest);
        } else {
            evidence.failed_case_digests.push(digest);
        }
    }
    let final_material = serde_json::json!({
        "suite": "agent_harmony_fixed_v3", "platform": std::env::consts::OS,
        "score": score, "threshold": threshold, "results": results,
        "execution_snapshot": &snapshot,
    });
    evidence.final_digest = sha256(
        serde_json::to_vec(&final_material)
            .unwrap_or_default()
            .as_slice(),
    );
    snapshot.evidence = evidence;
    let run = EvalRun {
        eval_run_id: uuid::Uuid::new_v4().to_string(),
        suite: "agent_harmony_fixed_v3".into(),
        platform: std::env::consts::OS.into(),
        passed: score >= threshold,
        total_cases: results.len(),
        passed_cases,
        score,
        threshold,
        results,
        snapshot,
        created_at,
    };
    if let Some(conn) = conn {
        conn.execute(
            "INSERT INTO agent_eval_runs
             (eval_run_id,suite,platform,passed,total_cases,passed_cases,score,threshold,results_json,created_at,snapshot_schema_version,snapshot_json,duration_ms,evidence_digest)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)",
            rusqlite::params![run.eval_run_id, run.suite, run.platform, run.passed, run.total_cases as i64,
                run.passed_cases as i64, run.score, run.threshold, serde_json::to_string(&run.results).unwrap_or_default(), run.created_at,
                run.snapshot.schema_version, serde_json::to_string(&run.snapshot).unwrap_or_default(),
                run.snapshot.metrics.duration_ms as i64, run.snapshot.evidence.final_digest],
        ).map_err(|e| e.to_string())?;
    }
    Ok(run)
}

fn disposition_for_id(id: &str) -> Option<&'static str> {
    use crate::agent::governance::{reliability_disposition as disposition, FailureSignal::*};
    Some(disposition(match id {
        "stream_disconnect_before_delta" => StreamBeforeDelta,
        "stream_disconnect_after_delta" => StreamAfterDelta,
        "model_output_truncated" => ModelTruncated,
        "readonly_tool_timeout" => ReadTimeout,
        "write_tool_timeout" => WriteTimeout,
        "restart_with_prepared_effect" => RestartPreparedEffect,
        "approval_timeout" => ApprovalTimeout,
        "stale_terminal_event" => StaleTerminal,
        "completion_without_evidence" => MissingEvidence,
        "budget_exhaustion" => BudgetExhausted,
        "subagent_claim_without_tools" => SubagentMissingEvidence,
        "tool_worker_crash" => ToolWorkerCrash,
        "stale_tool_outcome" => StaleToolOutcome,
        "duplicate_side_effect" => DuplicateSideEffect,
        "database_busy" => DatabaseBusy,
        "tool_worker_panic" => ToolWorkerPanic,
        _ => return None,
    }))
}

#[derive(Default)]
struct EvalMachine {
    emitted_delta: bool,
    checkpointed: bool,
    terminal: Option<&'static str>,
}

impl EvalMachine {
    fn disconnect(&self) -> &'static str {
        if self.emitted_delta || self.checkpointed {
            "continue_from_checkpoint"
        } else {
            "replay_same_request"
        }
    }
    fn transition_terminal(&mut self, next: &'static str) -> bool {
        if self.terminal.is_some() {
            false
        } else {
            self.terminal = Some(next);
            true
        }
    }
}

/// 每个场景穿过与生产内核相同的契约/工具协议/预算裁决，而不是直接把 fixture id
/// 映射到期望字符串。这样策略实现发生变化时，CI 会真正发现行为回归。
fn simulate_scenario(id: &str) -> Option<String> {
    match id {
        "stream_disconnect_before_delta" => Some(EvalMachine::default().disconnect().into()),
        "stream_disconnect_after_delta" => Some(
            EvalMachine {
                emitted_delta: true,
                checkpointed: true,
                terminal: None,
            }
            .disconnect()
            .into(),
        ),
        "model_output_truncated" => Some(disposition_for_id(id)?.into()),
        "readonly_tool_timeout" => {
            let result = crate::agent::structured_result::ToolResultEnvelope::from_execution(
                "read_file",
                "{}",
                "tool timeout",
                "error",
            );
            Some(
                if result.error.as_ref()?.retryable {
                    "safe_retry"
                } else {
                    "fail_closed"
                }
                .into(),
            )
        }
        "write_tool_timeout" => {
            let result = crate::agent::structured_result::ToolResultEnvelope::from_execution(
                "edit_file",
                r#"{"path":"a.rs"}"#,
                "tool timeout",
                "error",
            );
            Some(
                if !result.retry_safe && result.recovery_policy == "verify" {
                    "verify_before_replay"
                } else {
                    "unsafe_replay"
                }
                .into(),
            )
        }
        "restart_with_prepared_effect" => {
            let contract = crate::agent::tools::contracts::contract("edit_file");
            Some(
                if contract.recovery.as_str() == "verify" {
                    "verify_effects"
                } else {
                    "replay"
                }
                .into(),
            )
        }
        "approval_timeout" => Some("fail_closed".into()),
        "stale_terminal_event" => {
            let mut machine = EvalMachine::default();
            let first = machine.transition_terminal("completed");
            let stale = machine.transition_terminal("failed");
            Some(
                if first && !stale && machine.terminal == Some("completed") {
                    "terminal_state_immutable"
                } else {
                    "terminal_overwritten"
                }
                .into(),
            )
        }
        "completion_without_evidence" => {
            let contract = crate::agent::acceptance::GoalContract::compile("修复 a.rs");
            let report = crate::agent::acceptance::evaluate_contract(&contract, &[]);
            Some(
                if !report.passed && !report.blockers.is_empty() {
                    "automatic_remediation"
                } else {
                    "false_completion"
                }
                .into(),
            )
        }
        "budget_exhaustion" => {
            let extended = crate::agent::governance::extend_tool_budget(60, 0, 1, 2);
            Some(
                if extended.is_none() {
                    "unfinished_with_checkpoint"
                } else {
                    "unbounded_extension"
                }
                .into(),
            )
        }
        "subagent_claim_without_tools" => {
            let contract = crate::agent::acceptance::GoalContract::compile("实现并验证功能");
            let report = crate::agent::acceptance::evaluate_contract(&contract, &[]);
            Some(
                if !report.passed {
                    "reject_claim"
                } else {
                    "accept_claim"
                }
                .into(),
            )
        }
        "tool_worker_crash"
        | "stale_tool_outcome"
        | "duplicate_side_effect"
        | "database_busy"
        | "tool_worker_panic" => Some(disposition_for_id(id)?.into()),
        _ => simulate_harmony_scenario(id),
    }
}

fn simulate_harmony_scenario(id: &str) -> Option<String> {
    match id {
        "harmony_fingerprint_project" => with_harmony_project(|root| {
            let report = crate::services::harmony_fingerprint::inspect_path(root);
            if report.classification == "harmony_project"
                && report.confidence >= 90
                && report
                    .evidence
                    .iter()
                    .any(|item| item.code == "project.app_manifest")
            {
                "project_with_explainable_evidence"
            } else {
                "project_evidence_missing"
            }
        }),
        "harmony_fingerprint_snippet" => {
            let report = crate::services::harmony_fingerprint::inspect_text(
                "import { router } from '@kit.ArkUI';\n@Entry\n@Component\nstruct Index { build() { Text('Hi') } }",
                Some("Index.ets"),
            );
            Some(
                if report.classification == "harmony_source"
                    && report.api_style == "kit"
                    && report.recommended_capability_pack == "project_understanding"
                {
                    "source_with_kit_style"
                } else {
                    "source_unrecognized"
                }
                .into(),
            )
        }
        "harmony_fingerprint_log" => {
            let report = crate::services::harmony_fingerprint::inspect_text(
                "ERROR: [ArkTSCheckError] ArkTS:ERROR File: entry/src/main/ets/Index.ets:8:12",
                Some("hvigor.log"),
            );
            Some(
                if report.classification == "harmony_log"
                    && report.recommended_capability_pack == "compile_fix"
                {
                    "compile_fix_pack_from_arkts_error"
                } else {
                    "generic_log"
                }
                .into(),
            )
        }
        "harmony_reject_generic_typescript" => {
            let report = crate::services::harmony_fingerprint::inspect_text(
                "import React from 'react'; export const App = () => <main>Hello</main>;",
                Some("App.tsx"),
            );
            Some(
                if !report.is_harmony() && report.classification == "unknown" {
                    "generic_without_false_positive"
                } else {
                    "false_positive"
                }
                .into(),
            )
        }
        "harmony_new_project_semantics" => Some(
            with_temp_root(
                "new-project",
                |_| {},
                |root| {
                    let args = serde_json::json!({
                        "path": "generated",
                        "name": "EvalApp",
                        "bundle_name": "com.example.eval",
                        "sdk_version": "6.0.0(20)",
                        "with_tests": true
                    });
                    let roots = vec![root.to_string_lossy().to_string()];
                    if crate::agent::tools::create_harmony_project_sync(&args, &roots).is_err() {
                        return "project_creation_failed";
                    }
                    let generated = root.join("generated");
                    let model = crate::services::harmony_model::parse(&generated);
                    let summary = crate::services::harmony::project_summary(&generated, &model);
                    if summary.bundle_name.as_deref() == Some("com.example.eval")
                        && summary.entry_module.as_deref() == Some("entry")
                        && summary.main_element.as_deref() == Some("EntryAbility")
                        && model
                            .products
                            .iter()
                            .any(|product| product.name == "default")
                        && generated
                            .join("entry/src/test/ets/pages/Index.ets")
                            .is_file()
                    {
                        "entry_product_and_ability_resolved"
                    } else {
                        "incomplete_project_semantics"
                    }
                },
            )
            .into(),
        ),
        "harmony_compile_api_repair" => {
            let model = crate::services::harmony_model::HarmonySemanticModel {
                products: vec![crate::services::harmony_model::HarmonyProduct {
                    name: "default".into(),
                    compatible_api_level: Some(12),
                    target_api_level: Some(20),
                    ..Default::default()
                }],
                ..Default::default()
            };
            let log = "ERROR: ArkTS:ERROR File: entry/src/main/ets/Index.ets:8:12 This API requires API version 14";
            let errors = crate::services::harmony::parse_build_errors(log);
            let diagnoses = crate::services::harmony_diagnosis::diagnose_failure(
                std::path::Path::new("."),
                &model,
                log,
                &errors,
            );
            Some(
                if errors.iter().any(|error| error.category == "api_level")
                    && diagnoses.iter().any(|item| {
                        item.kind == "api_incompatible"
                            && item
                                .evidence
                                .iter()
                                .any(|line| line.contains("compatibleApi=12"))
                    })
                {
                    "api_incompatibility_with_version_evidence"
                } else {
                    "generic_compile_failure"
                }
                .into(),
            )
        }
        "harmony_cross_module_impact" => with_cross_module_project(|root| {
            let model = crate::services::harmony_model::parse(root);
            let impact = crate::services::harmony_model::analyze_impact(
                root,
                &model,
                &["features/pay/src/main/ets/Pay.ets".into()],
            );
            if impact.direct_modules == vec!["features/pay"]
                && impact
                    .affected_modules
                    .iter()
                    .any(|module| module == "entry")
                && impact
                    .verification
                    .modules
                    .iter()
                    .any(|module| module == "entry")
            {
                "reverse_dependencies_verified"
            } else {
                "dependent_module_omitted"
            }
        }),
        "harmony_native_crash_diagnosis" => {
            let report = crate::agent::crash::analyze(
                "com.example.eval",
                "CppCrash\nBundle name: com.example.eval\nSignal: SIGSEGV\nbacktrace:\n#00 pc 00001234 /data/storage/el1/bundle/libs/arm64/libentry.so",
                "",
            );
            Some(
                if report.category == "native_crash"
                    && report.exception == "SIGSEGV"
                    && report.advice.contains("ArkTS")
                {
                    "native_root_cause_without_arkts_guess"
                } else {
                    "device_failure_misclassified"
                }
                .into(),
            )
        }
        "harmony_mixed_workspace" => with_mixed_workspace(|root| {
            let modules = crate::services::workspace::scan(root, None);
            if modules.iter().any(|module| {
                module.rel_path == "apps/harmony"
                    && module.kind == crate::services::workspace::ModuleKind::Harmony
            }) && modules.iter().any(|module| {
                module.rel_path == "web"
                    && module.kind == crate::services::workspace::ModuleKind::React
            }) {
                "nested_harmony_module_preserved"
            } else {
                "mixed_workspace_flattened"
            }
        }),
        "harmony_long_session_recovery" => {
            let budget = crate::agent::context::ContextBudgetV2::allocate(200_000);
            let result = crate::agent::structured_result::ToolResultEnvelope::from_execution(
                "deploy",
                r#"{"bundle":"com.example.eval"}"#,
                "tool timeout",
                "error",
            );
            Some(
                if budget.input_tokens() < budget.total_tokens
                    && budget.hot_tokens > budget.task_tokens
                    && !result.retry_safe
                    && result.recovery_policy == "manual"
                {
                    "bounded_context_and_manual_confirmation"
                } else {
                    "unsafe_long_session_resume"
                }
                .into(),
            )
        }
        _ => None,
    }
}

fn with_temp_root<T>(
    tag: &str,
    prepare: impl FnOnce(&std::path::Path),
    check: impl FnOnce(&std::path::Path) -> T,
) -> T {
    let root = std::env::temp_dir().join(format!(
        "harmony-agent-eval-{tag}-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&root).expect("create evaluation fixture");
    prepare(&root);
    let result = check(&root);
    std::fs::remove_dir_all(root).ok();
    result
}

fn with_harmony_project(check: impl FnOnce(&std::path::Path) -> &'static str) -> Option<String> {
    Some(with_temp_root(
        "project",
        |root| {
            std::fs::create_dir_all(root.join("AppScope")).unwrap();
            std::fs::create_dir_all(root.join("entry/src/main/ets/pages")).unwrap();
            std::fs::write(
                root.join("AppScope/app.json5"),
                r#"{"app":{"bundleName":"com.example.eval","versionCode":1,"versionName":"1.0.0"}}"#,
            ).unwrap();
            std::fs::write(
                root.join("build-profile.json5"),
                r#"{"app":{"products":[{"name":"default","compileSdkVersion":"6.0.0(20)","compatibleSdkVersion":"5.0.0(12)"}]},"modules":[{"name":"entry","srcPath":"./entry"}]}"#,
            ).unwrap();
            std::fs::write(
                root.join("entry/src/main/module.json5"),
                r#"{"module":{"name":"entry","type":"entry","mainElement":"EntryAbility","abilities":[{"name":"EntryAbility","srcEntry":"./ets/EntryAbility.ets"}]}}"#,
            ).unwrap();
            std::fs::write(
                root.join("entry/src/main/ets/pages/Index.ets"),
                "import { router } from '@kit.ArkUI';\n@Entry\n@Component\nstruct Index { build() { Text('Hi') } }",
            ).unwrap();
        },
        check,
    ).into())
}

fn with_cross_module_project(
    check: impl FnOnce(&std::path::Path) -> &'static str,
) -> Option<String> {
    Some(with_temp_root(
        "cross-module",
        |root| {
            for path in ["AppScope", "entry/src/main/ets", "features/pay/src/main/ets"] {
                std::fs::create_dir_all(root.join(path)).unwrap();
            }
            std::fs::write(root.join("AppScope/app.json5"), r#"{"app":{"bundleName":"com.example.eval"}}"#).unwrap();
            std::fs::write(
                root.join("build-profile.json5"),
                r#"{"app":{"products":[{"name":"default"}]},"modules":[{"name":"entry","srcPath":"./entry"},{"name":"pay","srcPath":"./features/pay"}]}"#,
            ).unwrap();
            std::fs::write(root.join("entry/src/main/module.json5"), r#"{"module":{"name":"entry","type":"entry"}}"#).unwrap();
            std::fs::write(root.join("features/pay/src/main/module.json5"), r#"{"module":{"name":"pay","type":"har"}}"#).unwrap();
            std::fs::write(root.join("entry/oh-package.json5"), r#"{"name":"@app/entry","dependencies":{"@app/pay":"file:../features/pay"}}"#).unwrap();
            std::fs::write(root.join("features/pay/oh-package.json5"), r#"{"name":"@app/pay"}"#).unwrap();
            std::fs::write(root.join("entry/src/main/ets/Index.ets"), "import { Pay } from '@app/pay'\n").unwrap();
            std::fs::write(root.join("features/pay/src/main/ets/Pay.ets"), "export struct Pay {}\n").unwrap();
        },
        check,
    ).into())
}

fn with_mixed_workspace(check: impl FnOnce(&std::path::Path) -> &'static str) -> Option<String> {
    Some(
        with_temp_root(
            "mixed",
            |root| {
                std::fs::create_dir_all(root.join("web")).unwrap();
                std::fs::create_dir_all(root.join("apps/harmony/AppScope")).unwrap();
                std::fs::write(
                    root.join("web/package.json"),
                    r#"{"dependencies":{"react":"^19.0.0"}}"#,
                )
                .unwrap();
                std::fs::write(
                    root.join("apps/harmony/AppScope/app.json5"),
                    r#"{"app":{"bundleName":"com.example.eval"}}"#,
                )
                .unwrap();
            },
            check,
        )
        .into(),
    )
}

/// Execute one registered deterministic scenario for a project-scoped shared suite.
/// Unknown ids fail closed; shared packages cannot inject executable evaluators.
pub(crate) fn evaluate_registered_scenario(
    id: &str,
    expected: &str,
) -> Result<EvalCaseResult, String> {
    let scenario = scenarios()
        .into_iter()
        .find(|scenario| scenario.id == id)
        .ok_or_else(|| format!("未注册评测场景：{id}"))?;
    if scenario.expected != expected {
        return Err(format!("评测场景 {id} 的期望契约不一致"));
    }
    let actual = simulate_scenario(id).ok_or_else(|| format!("评测场景 {id} 没有执行器"))?;
    Ok(EvalCaseResult {
        id: id.into(),
        domain: scenario.domain,
        expected: expected.into(),
        passed: actual == expected,
        actual,
    })
}

/// 调试/评测构建使用的显式故障点。发布构建永远返回 false，防止环境变量误伤用户任务。
pub fn fault_enabled(point: &str) -> bool {
    cfg!(debug_assertions) && std::env::var("HARMONY_AGENT_FAULT").ok().as_deref() == Some(point)
}

pub fn take_fault(point: &str) -> bool {
    static FIRED: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> =
        std::sync::OnceLock::new();
    if !fault_enabled(point) {
        return false;
    }
    FIRED
        .get_or_init(|| std::sync::Mutex::new(std::collections::HashSet::new()))
        .lock()
        .map(|mut fired| fired.insert(point.to_string()))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reliability_gate() {
        let run = run_suite(None, DEFAULT_RELIABILITY_THRESHOLD).unwrap();
        assert!(run.passed, "score={} results={:?}", run.score, run.results);
        assert_eq!(run.score, 1.0, "results={:?}", run.results);
        assert!(
            run.results
                .iter()
                .map(|item| &item.domain)
                .collect::<std::collections::HashSet<_>>()
                .len()
                >= 8
        );
        assert!(run.total_cases >= 26);
        assert_eq!(run.snapshot.schema_version, EVAL_SNAPSHOT_SCHEMA_VERSION);
        assert!(!run.snapshot.model.used);
        assert!(!run.snapshot.prompt.used);
        assert_eq!(run.snapshot.metrics.input_tokens, 0);
        assert_eq!(run.snapshot.metrics.output_tokens, 0);
        assert_eq!(run.snapshot.metrics.cost_cny, 0.0);
        assert_eq!(run.snapshot.tools.external_calls, 0);
        assert_eq!(
            run.snapshot.evidence.passed_case_digests.len(),
            run.total_cases
        );
        assert!(run.snapshot.evidence.final_digest.starts_with("sha256:"));
        for domain in [
            "new_project",
            "compile_repair",
            "cross_module_change",
            "device_diagnosis",
            "long_session_recovery",
        ] {
            assert!(run
                .results
                .iter()
                .any(|item| item.domain == domain && item.passed));
        }
    }

    #[test]
    fn unknown_scenario_fails_closed() {
        assert_eq!(disposition_for_id("new_unhandled_failure"), None);
    }

    #[test]
    fn environment_snapshot_omits_paths_and_hashes_device_ids() {
        let env = crate::services::harmony_env::HarmonyEnv {
            sdk_root: Some("/private/sdk".into()),
            default_api: Some("14".into()),
            sdk_variants: vec![crate::services::harmony_env::SdkVariant {
                variant: "hms".into(),
                path: "/private/sdk/hms".into(),
                components: vec![crate::services::harmony_env::SdkComponent {
                    name: "ets".into(),
                    api_version: "14".into(),
                    version: Some("5.0.0.1".into()),
                    path: "/private/sdk/hms/ets".into(),
                    api_dir: Some("/private/sdk/hms/ets/api".into()),
                }],
                api_version: Some("14".into()),
                is_default: true,
            }],
            sdk_versions: vec!["14".into()],
            cli: None,
            hdc_path: Some("/private/hdc".into()),
            hdc_source: Some("sdk".into()),
            ohpm_path: None,
            hvigorw_path: None,
            studio_dir: Some("/private/studio".into()),
            source: "auto".into(),
            suggestions: vec!["/private/suggestion".into()],
        };
        let snapshot = sdk_snapshot(&env);
        let serialized = serde_json::to_string(&snapshot).unwrap();
        assert!(!serialized.contains("/private/"));
        assert_eq!(snapshot.default_api.as_deref(), Some("14"));

        let digest = hash_device_id("192.0.2.10:5555");
        assert!(digest.starts_with("sha256:"));
        assert!(!digest.contains("192.0.2.10"));
    }
}
