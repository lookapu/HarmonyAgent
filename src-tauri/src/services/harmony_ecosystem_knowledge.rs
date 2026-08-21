//! 可审计的鸿蒙生态知识记录。
//!
//! 统一表达三方包兼容性、常见错误和设备差异。静态条目只引用本仓库已验证的
//! 回归场景；包记录由官方 ohpm registry 审计动态生成，不把“没有元数据”推断成兼容。

use serde::Serialize;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::services::ohpm_audit::PackageAudit;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct KnowledgeSource {
    pub kind: String,
    pub reference: String,
    pub version: Option<String>,
    pub observed_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct EcosystemKnowledgeRecord {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub package_name: Option<String>,
    pub package_version: Option<String>,
    pub api_min: Option<u32>,
    pub api_max: Option<u32>,
    pub device_types: Vec<String>,
    pub error_fingerprints: Vec<String>,
    pub symptom: String,
    pub cause: String,
    pub resolution: String,
    pub applicability: String,
    pub verification_status: String,
    pub sources: Vec<KnowledgeSource>,
    pub limitations: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct KnowledgeQuery<'a> {
    pub keyword: &'a str,
    pub api_level: Option<u32>,
    pub device_type: Option<&'a str>,
    pub error_code: Option<&'a str>,
}

struct BuiltinRecord {
    id: &'static str,
    kind: &'static str,
    title: &'static str,
    keywords: &'static [&'static str],
    api_min: Option<u32>,
    api_max: Option<u32>,
    device_types: &'static [&'static str],
    errors: &'static [&'static str],
    symptom: &'static str,
    cause: &'static str,
    resolution: &'static str,
    applicability: &'static str,
    verification: &'static str,
    source: &'static str,
    limitations: &'static [&'static str],
}

const BUILTINS: &[BuiltinRecord] = &[
    BuiltinRecord {
        id: "hvigor-api-level-mismatch",
        kind: "common_error",
        title: "Hvigor SDK/API Level 不匹配",
        keywords: &["hvigor", "sdk", "api level", "00303083", "compatibleSdkVersion"],
        api_min: None,
        api_max: None,
        device_types: &[],
        errors: &["00303083", "configured sdk version does not exist"],
        symptom: "Hvigor 在配置或 ArkTS 编译阶段报告 SDK 组合不存在、API 不可用或类型定义不匹配。",
        cause: "工程 compile/compatible/target API、实际构建 SDK 与所用符号引入版本没有形成一致约束。",
        resolution: "读取实际构建使用的 sdk-pkg.json 与本机 .d.ts；选择当前 API 可用符号，或在确有产品要求时升级 compileSdkVersion，并为 compatible API 增加运行时守卫和回退。",
        applicability: "所有 HarmonyOS Stage 工程；结论必须重新绑定当前 product。",
        verification: "regression_verified",
        source: "services::harmony_api_diagnosis::tests::maps_type_error_to_local_definition_and_official_change",
        limitations: &["具体平台版本/API 组合只能从本机 SDK 读取，不能从条目示例推断。"],
    },
    BuiltinRecord {
        id: "unsigned-hap-install",
        kind: "common_error",
        title: "unsigned HAP 不能作为真机安装闭环证据",
        keywords: &["unsigned", "hap", "signing", "No signingConfig", "install"],
        api_min: None,
        api_max: None,
        device_types: &[],
        errors: &["No signingConfig", "sign verify"],
        symptom: "Hvigor 构建成功但产物为 unsigned HAP，后续设备安装失败或无法开始。",
        cause: "编译/打包成功与签名/安装成功是不同后置条件。",
        resolution: "先用产物清单复验签名结构；通过隔离凭据或 DevEco 显式签名重新构建，再选择已授权在线设备安装。",
        applicability: "所有需要真机或模拟器安装的 HAP。",
        verification: "build_and_regression_verified",
        source: "agent::tools::build_tools::build_workflow_tests::deploy_requires_online_authorized_device_capabilities",
        limitations: &["知识条目不保存、复制或生成证书和口令。"],
    },
    BuiltinRecord {
        id: "device-authorization-boundary",
        kind: "device_difference",
        title: "设备在线与调试授权是独立状态",
        keywords: &["device", "hdc", "unauthorized", "offline", "授权"],
        api_min: None,
        api_max: None,
        device_types: &["default", "tablet", "2in1", "wearable", "tv", "car"],
        errors: &["unauthorized", "device_offline"],
        symptom: "设备可被发现但未授权，或历史设备标识仍存在但连接已离线。",
        cause: "连接、授权和 install/Ability/Hilog 能力不能由单一设备列表字符串替代。",
        resolution: "重新读取设备快照；仅在 online 且 authorized=true 并具备所需能力时部署，授权失败不得通过改签名或重复安装绕过。",
        applicability: "USB、无线连接、模拟器及多设备任务。",
        verification: "regression_verified",
        source: "agent::tools::build_tools::build_workflow_tests::deploy_requires_online_authorized_device_capabilities",
        limitations: &["不同设备的授权 UI 和策略需在目标设备上观察。"],
    },
    BuiltinRecord {
        id: "install-identity-conflict",
        kind: "device_difference",
        title: "同包名应用的签名/更新身份冲突",
        keywords: &["signature mismatch", "install conflict", "update incompatible", "bundle"],
        api_min: None,
        api_max: None,
        device_types: &["default", "tablet", "2in1", "wearable", "tv", "car"],
        errors: &["install_conflict", "INSTALL_FAILED_UPDATE_INCOMPATIBLE"],
        symptom: "某台设备安装失败，而其他设备可成功；失败设备已有同 bundle 的不同签名或更新身份。",
        cause: "安装状态属于逐设备事实，不能把单台冲突归因扩散到整个批次。",
        resolution: "逐设备读取已安装应用信息；只有用户确认可丢弃旧应用及数据时才卸载，恢复时只重试失败设备。",
        applicability: "覆盖安装与多设备部署。",
        verification: "regression_verified",
        source: "agent::tools::build_tools::build_workflow_tests::deployment_failures_distinguish_authorization_and_install_conflicts",
        limitations: &["卸载是破坏性恢复，必须显式确认。"],
    },
    BuiltinRecord {
        id: "device-capability-guard",
        kind: "device_difference",
        title: "deviceTypes 声明不能替代 SystemCapability 守卫",
        keywords: &["deviceTypes", "SystemCapability", "tablet", "wearable", "capability"],
        api_min: None,
        api_max: None,
        device_types: &["default", "tablet", "2in1", "wearable", "tv", "car"],
        errors: &["system capability", "not support"],
        symptom: "工程声明支持某类设备，但特定 API、窗口形态或硬件能力在目标设备不可用。",
        cause: "设备类别、系统 API Level、SystemCapability 与真实硬件能力是不同维度。",
        resolution: "在清单与本机 SDK 定义中核对能力要求，源码增加显式能力探测/低版本回退，并在目标设备矩阵逐台验证。",
        applicability: "跨 Phone/Tablet/2in1/Wearable/TV/Car 的工程。",
        verification: "regression_verified",
        source: "services::harmony_consistency::tests::detects_missing_permission_api_level_capability_and_manifest_errors",
        limitations: &["静态审计不能证明布局、性能或硬件行为已通过真机验证。"],
    },
    BuiltinRecord {
        id: "ohpm-compatibility-unknown",
        kind: "package_compatibility",
        title: "ohpm 包缺少兼容声明时保持未知",
        keywords: &["ohpm", "package", "兼容", "api", "license", "security"],
        api_min: None,
        api_max: None,
        device_types: &[],
        errors: &[],
        symptom: "registry 能找到包，但没有可机器判定的最低 API、漏洞公告或完整兼容矩阵。",
        cause: "包存在、下载量或未发现公告都不能证明与当前工程/设备兼容。",
        resolution: "锁定明确版本，核对许可证、完整性和源码公告；在目标工程安装后执行一致性检查、lint、测试、构建和设备验证。",
        applicability: "所有准备采用的第三方 ohpm 包。",
        verification: "registry_rule_verified",
        source: "services::ohpm_audit::tests::parses_version_compatibility_license_and_supply_chain_risks",
        limitations: &["ohpm registry 当前元数据不等同于漏洞数据库。"],
    },
];

pub fn search(query: &KnowledgeQuery<'_>, limit: usize) -> Vec<EcosystemKnowledgeRecord> {
    let keyword = query.keyword.to_ascii_lowercase();
    let error = query.error_code.unwrap_or("").to_ascii_lowercase();
    let device = query.device_type.unwrap_or("").to_ascii_lowercase();
    let mut scored = BUILTINS
        .iter()
        .filter_map(|record| {
            let haystack = format!(
                "{} {} {} {} {}",
                record.id,
                record.title,
                record.keywords.join(" "),
                record.errors.join(" "),
                record.symptom
            )
            .to_ascii_lowercase();
            let mut score = 0usize;
            if !keyword.is_empty() && haystack.contains(&keyword) {
                score += 4;
            }
            if !error.is_empty() && haystack.contains(&error) {
                score += 8;
            }
            if !device.is_empty()
                && record
                    .device_types
                    .iter()
                    .any(|value| value.eq_ignore_ascii_case(&device))
            {
                score += 2;
            }
            if let Some(api) = query.api_level {
                if record.api_min.is_some_and(|minimum| api < minimum)
                    || record.api_max.is_some_and(|maximum| api > maximum)
                {
                    return None;
                }
                score += usize::from(record.api_min.is_some() || record.api_max.is_some());
            }
            (score > 0).then_some((score, to_record(record)))
        })
        .collect::<Vec<_>>();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.id.cmp(&b.1.id)));
    scored
        .into_iter()
        .take(limit)
        .map(|(_, record)| record)
        .collect()
}

pub fn from_package_audit(audit: &PackageAudit) -> EcosystemKnowledgeRecord {
    let verification_status = match audit.api_compatible {
        Some(true) => "registry_compatible",
        Some(false) => "registry_incompatible",
        None => "compatibility_unknown",
    };
    EcosystemKnowledgeRecord {
        id: format!("ohpm:{}@{}", audit.package_name, audit.selected_version),
        kind: "package_compatibility".into(),
        title: format!("{} {} 采用前兼容记录", audit.package_name, audit.selected_version),
        package_name: Some(audit.package_name.clone()),
        package_version: Some(audit.selected_version.clone()),
        api_min: audit.minimum_api,
        api_max: None,
        device_types: Vec::new(),
        error_fingerprints: Vec::new(),
        symptom: audit.compatibility.join("；"),
        cause: format!(
            "registry 元数据给出的兼容判定为 {}；许可证风险={}；安全状态={}",
            verification_status, audit.license_risk, audit.security_status
        ),
        resolution: "锁定该版本后安装到目标工程，继续执行 check_sdk_alignment、lint、测试、build_project；涉及系统能力或 UI 行为时增加目标设备验证。".into(),
        applicability: audit
            .target_api
            .map(|api| format!("当前查询绑定工程 compatible API {api}"))
            .unwrap_or_else(|| "尚未绑定工程 API，采用前必须补验".into()),
        verification_status: verification_status.into(),
        sources: vec![KnowledgeSource {
            kind: "official_ohpm_registry".into(),
            reference: audit.source_url.clone(),
            version: Some(audit.selected_version.clone()),
            observed_at: Some(now_seconds()),
        }],
        limitations: vec![
            "registry 声明不是构建或真机验证结果。".into(),
            "registry 未提供可核验漏洞公告时安全状态必须保持未知。".into(),
        ],
    }
}

pub fn render(records: &[EcosystemKnowledgeRecord]) -> String {
    let mut output = String::new();
    for record in records {
        output.push_str(&format!(
            "\n[生态:{}] {} ｜ 状态：{}\n  现象: {}\n  根因: {}\n  处理: {}\n  条件: {}\n  来源: {}\n  边界: {}\n",
            record.kind,
            record.title,
            record.verification_status,
            record.symptom,
            record.cause,
            record.resolution,
            record.applicability,
            record
                .sources
                .iter()
                .map(|source| source.reference.as_str())
                .collect::<Vec<_>>()
                .join("；"),
            record.limitations.join("；")
        ));
    }
    output
}

fn to_record(record: &BuiltinRecord) -> EcosystemKnowledgeRecord {
    EcosystemKnowledgeRecord {
        id: record.id.into(),
        kind: record.kind.into(),
        title: record.title.into(),
        package_name: None,
        package_version: None,
        api_min: record.api_min,
        api_max: record.api_max,
        device_types: record
            .device_types
            .iter()
            .map(|value| (*value).into())
            .collect(),
        error_fingerprints: record.errors.iter().map(|value| (*value).into()).collect(),
        symptom: record.symptom.into(),
        cause: record.cause.into(),
        resolution: record.resolution.into(),
        applicability: record.applicability.into(),
        verification_status: record.verification.into(),
        sources: vec![KnowledgeSource {
            kind: "regression_test".into(),
            reference: record.source.into(),
            version: Some(env!("CARGO_PKG_VERSION").into()),
            observed_at: None,
        }],
        limitations: record
            .limitations
            .iter()
            .map(|value| (*value).into())
            .collect(),
    }
}

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn searches_error_and_device_conditions_with_sources() {
        let records = search(
            &KnowledgeQuery {
                keyword: "device",
                device_type: Some("tablet"),
                error_code: Some("unauthorized"),
                ..Default::default()
            },
            4,
        );
        assert_eq!(records[0].id, "device-authorization-boundary");
        assert!(!records[0].sources.is_empty());
        assert!(records.iter().all(|record| !record.limitations.is_empty()));
    }

    #[test]
    fn package_audit_preserves_unknown_compatibility_and_registry_source() {
        let audit = PackageAudit {
            package_name: "@demo/pkg".into(),
            selected_version: "1.2.3".into(),
            latest_version: "1.2.3".into(),
            requested_version: None,
            version_relation: "latest".into(),
            recent_versions: vec!["1.2.3".into()],
            published_at: None,
            license: "Apache-2.0".into(),
            license_risk: "permissive".into(),
            compatibility: Vec::new(),
            minimum_api: None,
            target_api: Some(23),
            api_compatible: None,
            deprecated: None,
            repository: None,
            dependency_count: 0,
            lifecycle_scripts: Vec::new(),
            external_dependencies: Vec::new(),
            integrity: "registry 提供 integrity".into(),
            security_status: "unknown".into(),
            source_url: "https://ohpm.example/@demo/pkg".into(),
        };
        let record = from_package_audit(&audit);
        assert_eq!(record.verification_status, "compatibility_unknown");
        assert_eq!(record.sources[0].kind, "official_ohpm_registry");
        assert!(record.limitations.iter().any(|item| item.contains("真机")));
    }
}
