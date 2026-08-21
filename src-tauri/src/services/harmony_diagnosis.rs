//! HarmonyOS 构建失败的专项诊断：把日志模式与工程语义证据合并为可执行结论。

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HarmonySpecializedDiagnosis {
    /// dependency_conflict / cache_corruption / sdk_missing / signing_failure / api_incompatible
    pub kind: String,
    pub confidence: f32,
    pub evidence: Vec<String>,
    pub recovery_steps: Vec<String>,
    pub auto_recoverable: bool,
}

pub fn diagnose_failure(
    _root: &Path,
    model: &crate::services::harmony_model::HarmonySemanticModel,
    log: &str,
    errors: &[crate::services::harmony::BuildError],
) -> Vec<HarmonySpecializedDiagnosis> {
    let lower = log.to_ascii_lowercase();
    let mut diagnoses = Vec::new();

    let conflicts = dependency_conflicts(model);
    if !conflicts.is_empty()
        || contains_any(
            &lower,
            &[
                "version conflict",
                "conflicting depend",
                "cannot resolve dependency",
                "peer depend",
            ],
        )
    {
        let mut evidence = conflicts;
        push_log_evidence(
            &mut evidence,
            log,
            &["conflict", "resolve dependency", "peer depend"],
        );
        diagnoses.push(HarmonySpecializedDiagnosis {
            kind: "dependency_conflict".into(),
            confidence: if evidence.is_empty() { 0.65 } else { 0.92 },
            evidence,
            recovery_steps: vec![
                "比较各模块 oh-package.json5 对同一包的约束并统一兼容区间".into(),
                "核对锁文件中的精确版本与来源，必要时执行一次受控 OHPM 重新同步".into(),
                "重新构建原受影响模块，禁止只以 ohpm install 退出码作为成功证据".into(),
            ],
            auto_recoverable: false,
        });
    }

    if contains_any(
        &lower,
        &[
            "cache is corrupted",
            "corrupted cache",
            "integrity check failed",
            "checksum mismatch",
            "unexpected end of file",
            "invalid cache",
        ],
    ) {
        let mut evidence = Vec::new();
        push_log_evidence(
            &mut evidence,
            log,
            &[
                "corrupt",
                "integrity",
                "checksum",
                "unexpected end",
                "invalid cache",
            ],
        );
        diagnoses.push(HarmonySpecializedDiagnosis {
            kind: "cache_corruption".into(),
            confidence: 0.9,
            evidence,
            recovery_steps: vec![
                "先用 build_project(clean=true) 执行 Hvigor 官方 clean 并重建".into(),
                "若错误指向 OHPM 完整性，再强制同步依赖并核对缺失包".into(),
                "不要直接删除整个工程或用户 SDK；仍失败时保留日志并只清理被点名的缓存".into(),
            ],
            auto_recoverable: true,
        });
    }

    if errors.iter().any(|error| error.category == "sdk")
        || contains_any(
            &lower,
            &["sdk not found", "cannot find sdk", "deveco_sdk_home"],
        )
    {
        let product_evidence = model
            .products
            .iter()
            .map(|product| {
                format!(
                    "product={} compile={} compatible={} target={}",
                    product.name,
                    product.compile_sdk_version.as_deref().unwrap_or("--"),
                    product.compatible_sdk_version.as_deref().unwrap_or("--"),
                    product.target_sdk_version.as_deref().unwrap_or("--")
                )
            })
            .collect::<Vec<_>>();
        diagnoses.push(HarmonySpecializedDiagnosis {
            kind: "sdk_missing".into(),
            confidence: 0.96,
            evidence: product_evidence,
            recovery_steps: vec![
                "运行环境检查并确认 DEVECO_SDK_HOME 指向含 default/sdk-pkg.json 的 SDK 根".into(),
                "对照产品 compile/compatible/target API Level 安装或选择匹配 SDK".into(),
                "环境证据更新后重试，不通过修改源码掩盖缺失 SDK".into(),
            ],
            auto_recoverable: false,
        });
    }

    if errors.iter().any(|error| error.category == "signing") {
        let evidence = if model.signing_configs.is_empty() {
            vec!["工程未声明 signingConfigs".into()]
        } else {
            model
                .signing_configs
                .iter()
                .map(|config| {
                    format!(
                        "signingConfig={} material={} cert={} profile={} keystore={} alias={}",
                        config.name,
                        config.material_configured,
                        config.certificate_configured,
                        config.profile_configured,
                        config.keystore_configured,
                        config.key_alias_configured
                    )
                })
                .collect()
        };
        diagnoses.push(HarmonySpecializedDiagnosis {
            kind: "signing_failure".into(),
            confidence: 0.97,
            evidence,
            recovery_steps: vec![
                "运行 diagnose_signing 核对产品引用、证书、profile、keystore、alias 与设备".into(),
                "只修复缺失或不匹配的签名引用，不在日志或 checkpoint 中复制密码/私钥".into(),
                "重新构建并验证产物确为签名 HAP 后再部署".into(),
            ],
            auto_recoverable: false,
        });
    }

    if errors.iter().any(|error| error.category == "api_level")
        || contains_any(
            &lower,
            &["requires api", "api version", "not supported in api"],
        )
    {
        let evidence = model
            .products
            .iter()
            .filter_map(|product| {
                Some(format!(
                    "product={} compatibleApi={} targetApi={}",
                    product.name,
                    product.compatible_api_level?,
                    product
                        .target_api_level
                        .map(|level| level.to_string())
                        .unwrap_or_else(|| "--".into())
                ))
            })
            .collect();
        diagnoses.push(HarmonySpecializedDiagnosis {
            kind: "api_incompatible".into(),
            confidence: 0.94,
            evidence,
            recovery_steps: vec![
                "从错误位置确认 API 的引入版本，并与当前产品 compatibleApi 比较".into(),
                "优先使用当前 SDK 中可用的替代 API 或增加显式版本守卫".into(),
                "只有产品真实要求提高最低系统版本时才调整 compatibleSdkVersion".into(),
            ],
            auto_recoverable: false,
        });
    }

    // 同类只保留一条，并按置信度降序，结果稳定可审计。
    let mut by_kind = BTreeMap::new();
    for diagnosis in diagnoses {
        by_kind.entry(diagnosis.kind.clone()).or_insert(diagnosis);
    }
    let mut diagnoses = by_kind.into_values().collect::<Vec<_>>();
    diagnoses.sort_by(|a, b| {
        b.confidence
            .partial_cmp(&a.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.kind.cmp(&b.kind))
    });
    diagnoses
}

fn dependency_conflicts(
    model: &crate::services::harmony_model::HarmonySemanticModel,
) -> Vec<String> {
    let mut versions = BTreeMap::<String, BTreeSet<String>>::new();
    for dependency in &model.dependencies {
        versions.entry(dependency.name.clone()).or_default().insert(
            dependency
                .locked_version
                .clone()
                .unwrap_or_else(|| dependency.requirement.clone()),
        );
    }
    versions
        .into_iter()
        .filter(|(_, versions)| versions.len() > 1)
        .map(|(name, versions)| {
            format!(
                "{name}: {}",
                versions.into_iter().collect::<Vec<_>>().join(" vs ")
            )
        })
        .collect()
}

fn contains_any(text: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| text.contains(needle))
}

fn push_log_evidence(out: &mut Vec<String>, log: &str, needles: &[&str]) {
    for line in log.lines() {
        let lower = line.to_ascii_lowercase();
        if needles.iter().any(|needle| lower.contains(needle)) {
            let redacted = crate::utils::redact::redact_text(line.trim());
            if !redacted.is_empty() && !out.contains(&redacted) {
                out.push(redacted.chars().take(500).collect());
            }
        }
        if out.len() >= 8 {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn combines_log_and_model_evidence_for_specialized_failures() {
        let model = crate::services::harmony_model::HarmonySemanticModel {
            products: vec![crate::services::harmony_model::HarmonyProduct {
                name: "default".into(),
                compatible_api_level: Some(12),
                target_api_level: Some(20),
                ..Default::default()
            }],
            dependencies: vec![
                crate::services::harmony_model::HarmonyDependency {
                    from_module: "entry".into(),
                    name: "pkg".into(),
                    requirement: "^1.0.0".into(),
                    locked_version: Some("1.2.0".into()),
                    ..Default::default()
                },
                crate::services::harmony_model::HarmonyDependency {
                    from_module: "feature".into(),
                    name: "pkg".into(),
                    requirement: "^2.0.0".into(),
                    locked_version: Some("2.1.0".into()),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let errors = crate::services::harmony::parse_build_errors(
            "ERROR: ArkTS:ERROR File: entry/Index.ets:1:1 This API requires API version 14",
        );
        let diagnoses = diagnose_failure(
            Path::new("."),
            &model,
            "version conflict: pkg\ncache is corrupted\nrequires API 14",
            &errors,
        );
        assert!(diagnoses
            .iter()
            .any(|item| item.kind == "dependency_conflict"));
        assert!(diagnoses.iter().any(|item| item.kind == "cache_corruption"));
        let api = diagnoses
            .iter()
            .find(|item| item.kind == "api_incompatible")
            .unwrap();
        assert!(api
            .evidence
            .iter()
            .any(|item| item.contains("compatibleApi=12")));
    }

    #[test]
    fn diagnoses_sdk_and_signing_with_safe_model_evidence() {
        let model = crate::services::harmony_model::HarmonySemanticModel {
            products: vec![crate::services::harmony_model::HarmonyProduct {
                name: "default".into(),
                compile_sdk_version: Some("6.0.0(20)".into()),
                compatible_sdk_version: Some("5.0.0(12)".into()),
                ..Default::default()
            }],
            ..Default::default()
        };
        let log = "ERROR: [00303312] Cannot find the corresponding SDK version.\nSigning configuration failed";
        let errors = crate::services::harmony::parse_build_errors(log);
        let diagnoses = diagnose_failure(Path::new("."), &model, log, &errors);
        let sdk = diagnoses
            .iter()
            .find(|item| item.kind == "sdk_missing")
            .unwrap();
        assert!(sdk
            .evidence
            .iter()
            .any(|item| item.contains("compatible=5.0.0(12)")));
        let signing = diagnoses
            .iter()
            .find(|item| item.kind == "signing_failure")
            .unwrap();
        assert_eq!(signing.evidence, vec!["工程未声明 signingConfigs"]);
        assert!(!signing.auto_recoverable);
    }
}
