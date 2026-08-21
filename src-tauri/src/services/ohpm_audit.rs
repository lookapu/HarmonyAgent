//! ohpm registry 包元数据审计：版本、HarmonyOS 兼容范围、许可证与供应链风险。

use serde::Serialize;

use crate::utils::net::build_client_auto;

const REGISTRY_BASE: &str = "https://ohpm.openharmony.cn/ohpm";

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PackageAudit {
    pub package_name: String,
    pub selected_version: String,
    pub latest_version: String,
    pub requested_version: Option<String>,
    pub version_relation: String,
    pub recent_versions: Vec<String>,
    pub published_at: Option<String>,
    pub license: String,
    pub license_risk: String,
    pub compatibility: Vec<String>,
    pub minimum_api: Option<u32>,
    pub target_api: Option<u32>,
    pub api_compatible: Option<bool>,
    pub deprecated: Option<String>,
    pub repository: Option<String>,
    pub dependency_count: usize,
    pub lifecycle_scripts: Vec<String>,
    pub external_dependencies: Vec<String>,
    pub integrity: String,
    pub security_status: String,
    pub source_url: String,
}

pub async fn fetch(
    package_name: &str,
    requested_version: Option<&str>,
    target_api: Option<u32>,
) -> Result<PackageAudit, String> {
    let client = build_client_auto()?;
    let source_url = format!("{REGISTRY_BASE}/{package_name}");
    let response = client
        .get(&source_url)
        .send()
        .await
        .map_err(|error| format!("查询 ohpm registry 元数据失败：{error}"))?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Err(format!("ohpm registry 未找到包 {package_name}"));
    }
    let response = response
        .error_for_status()
        .map_err(|error| format!("ohpm registry 返回错误：{error}"))?;
    let metadata: serde_json::Value = response
        .json()
        .await
        .map_err(|error| format!("解析 ohpm registry 元数据失败：{error}"))?;
    parse(
        &metadata,
        package_name,
        requested_version,
        target_api,
        source_url,
    )
}

pub fn parse(
    metadata: &serde_json::Value,
    package_name: &str,
    requested_version: Option<&str>,
    target_api: Option<u32>,
    source_url: String,
) -> Result<PackageAudit, String> {
    let versions = metadata["versions"]
        .as_object()
        .ok_or_else(|| "registry 元数据缺少 versions".to_string())?;
    if versions.is_empty() {
        return Err("registry 元数据 versions 为空".to_string());
    }
    let latest = metadata["dist-tags"]["latest"]
        .as_str()
        .filter(|value| versions.contains_key(*value))
        .or_else(|| {
            versions
                .keys()
                .max_by(|a, b| compare_versions(a, b))
                .map(String::as_str)
        })
        .ok_or_else(|| "registry 元数据无法确定 latest 版本".to_string())?;
    let requested = requested_version
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let selected_version = requested.unwrap_or(latest);
    let selected = versions.get(selected_version).ok_or_else(|| {
        format!("请求版本 {selected_version} 不存在；registry latest 为 {latest}")
    })?;

    let mut recent_versions = versions.keys().cloned().collect::<Vec<_>>();
    recent_versions.sort_by(|a, b| compare_versions(b, a));
    recent_versions.truncate(12);

    let license = string_field(selected, "license")
        .or_else(|| string_field(metadata, "license"))
        .unwrap_or_else(|| "未声明".to_string());
    let license_risk = classify_license(&license).to_string();
    let compatibility = compatibility_claims(selected);
    let minimum_api = minimum_api(selected);
    let api_compatible = target_api
        .zip(minimum_api)
        .map(|(target, minimum)| target >= minimum);
    let deprecated = deprecation(metadata, selected);
    let repository = repository(selected).or_else(|| repository(metadata));
    let dependencies = selected["dependencies"].as_object();
    let dependency_count = dependencies.map_or(0, serde_json::Map::len);
    let external_dependencies = dependencies
        .into_iter()
        .flat_map(|values| values.iter())
        .filter_map(|(name, requirement)| {
            let requirement = requirement.as_str()?;
            is_external_requirement(requirement).then(|| format!("{name}: {requirement}"))
        })
        .collect::<Vec<_>>();
    let lifecycle_scripts = selected["scripts"]
        .as_object()
        .into_iter()
        .flat_map(|scripts| {
            ["preinstall", "install", "postinstall"]
                .into_iter()
                .filter(|key| scripts.contains_key(*key))
        })
        .map(str::to_string)
        .collect::<Vec<_>>();
    let integrity = if selected["dist"]["integrity"].as_str().is_some() {
        "registry 提供 integrity".to_string()
    } else if selected["dist"]["shasum"].as_str().is_some() {
        "registry 仅提供 shasum".to_string()
    } else {
        "registry 未提供完整性摘要".to_string()
    };
    let security_status = security_status(
        deprecated.as_deref(),
        &lifecycle_scripts,
        &external_dependencies,
        &integrity,
    );

    Ok(PackageAudit {
        package_name: package_name.to_string(),
        selected_version: selected_version.to_string(),
        latest_version: latest.to_string(),
        requested_version: requested.map(str::to_string),
        version_relation: version_relation(selected_version, latest).to_string(),
        recent_versions,
        published_at: metadata["time"][selected_version]
            .as_str()
            .and_then(nonempty),
        license,
        license_risk,
        compatibility,
        minimum_api,
        target_api,
        api_compatible,
        deprecated,
        repository,
        dependency_count,
        lifecycle_scripts,
        external_dependencies,
        integrity,
        security_status,
        source_url,
    })
}

pub fn render(audit: &PackageAudit, detail: bool) -> String {
    let mut out = format!(
        "ohpm 包审计：{}\n- 选定版本: {}（latest {}；{}）\n- 许可证: {}（{}）\n- HarmonyOS 兼容: {}\n- 完整性: {}\n- 安全状态: {}\n- 依赖: {} 个；生命周期安装脚本: {}；外部来源依赖: {}\n- registry 证据: {}\n",
        audit.package_name,
        audit.selected_version,
        audit.latest_version,
        audit.version_relation,
        audit.license,
        audit.license_risk,
        render_compatibility(audit),
        audit.integrity,
        audit.security_status,
        audit.dependency_count,
        if audit.lifecycle_scripts.is_empty() { "无".to_string() } else { audit.lifecycle_scripts.join(", ") },
        if audit.external_dependencies.is_empty() { "无".to_string() } else { audit.external_dependencies.join("；") },
        audit.source_url,
    );
    if let Some(value) = &audit.deprecated {
        out.push_str(&format!("- 废弃声明: {value}\n"));
    }
    if let Some(value) = &audit.repository {
        out.push_str(&format!("- 源码仓库: {value}\n"));
    }
    if let Some(value) = &audit.published_at {
        out.push_str(&format!("- 选定版本发布时间: {value}\n"));
    }
    if detail {
        out.push_str(&format!(
            "- 最近版本: {}\n",
            audit.recent_versions.join(", ")
        ));
        if !audit.compatibility.is_empty() {
            out.push_str(&format!(
                "- 原始兼容声明: {}\n",
                audit.compatibility.join("；")
            ));
        }
    }
    out.push_str("- 漏洞边界: ohpm registry 当前元数据不提供可核验的漏洞公告；“无已知漏洞”不能由本报告证明，采用前仍需审阅仓库公告、锁定版本并在构建后运行项目测试。\n");
    out
}

fn render_compatibility(audit: &PackageAudit) -> String {
    match (audit.target_api, audit.minimum_api, audit.api_compatible) {
        (Some(target), Some(minimum), Some(true)) => {
            format!("兼容（工程 API {target} ≥ 包声明 API {minimum}）")
        }
        (Some(target), Some(minimum), Some(false)) => {
            format!("不兼容（工程 API {target} < 包声明 API {minimum}）")
        }
        (_, Some(minimum), _) => format!("包声明最低 API {minimum}；未绑定工程 API，待验证"),
        _ if audit.compatibility.is_empty() => {
            "包未声明可机器判定的 SDK/API 范围，待安装后构建验证".to_string()
        }
        _ => "存在兼容声明但无法归一为 API Level，待人工核对并构建验证".to_string(),
    }
}

fn compatibility_claims(value: &serde_json::Value) -> Vec<String> {
    let mut claims = Vec::new();
    for key in [
        "compatibleSdkVersion",
        "compatibleSdk",
        "apiVersion",
        "apiLevel",
        "engines",
    ] {
        let field = &value[key];
        if !field.is_null() {
            let rendered = field
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| field.to_string());
            claims.push(format!("{key}={rendered}"));
        }
    }
    claims
}

fn minimum_api(value: &serde_json::Value) -> Option<u32> {
    [
        "compatibleSdkVersion",
        "compatibleSdk",
        "apiVersion",
        "apiLevel",
    ]
    .into_iter()
    .find_map(|key| parse_api_level(&value[key]))
    .or_else(|| {
        value["engines"]
            .as_object()?
            .iter()
            .find(|(key, _)| {
                matches!(
                    key.to_ascii_lowercase().as_str(),
                    "harmonyos" | "openharmony" | "ohos"
                )
            })
            .and_then(|(_, value)| parse_api_level(value))
    })
}

fn parse_api_level(value: &serde_json::Value) -> Option<u32> {
    if let Some(value) = value.as_u64() {
        return u32::try_from(value).ok();
    }
    let value = value.as_str()?;
    if let Some((_, suffix)) = value.rsplit_once('(') {
        if let Ok(level) = suffix.trim_end_matches(')').parse() {
            return Some(level);
        }
    }
    let value = value.trim();
    if value.contains('.') {
        return None;
    }
    value
        .split(|character: char| !character.is_ascii_digit())
        .find(|part| !part.is_empty())
        .and_then(|part| part.parse::<u32>().ok())
}

fn deprecation(root: &serde_json::Value, selected: &serde_json::Value) -> Option<String> {
    [&selected["deprecated"], &root["deprecated"]]
        .into_iter()
        .find_map(|value| match value {
            serde_json::Value::String(value) if !value.trim().is_empty() => {
                Some(value.trim().to_string())
            }
            serde_json::Value::Bool(true) => Some("registry 标记为 deprecated".to_string()),
            _ => None,
        })
}

fn repository(value: &serde_json::Value) -> Option<String> {
    match &value["repository"] {
        serde_json::Value::String(value) => nonempty(value),
        serde_json::Value::Object(value) => value
            .get("url")
            .and_then(serde_json::Value::as_str)
            .and_then(nonempty),
        _ => None,
    }
}

fn string_field(value: &serde_json::Value, key: &str) -> Option<String> {
    value[key].as_str().and_then(nonempty)
}

fn nonempty(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn classify_license(value: &str) -> &'static str {
    let normalized = value.to_ascii_uppercase();
    if normalized == "未声明".to_ascii_uppercase() || normalized.trim().is_empty() {
        "高风险：未声明"
    } else if ["MIT", "BSD", "APACHE", "ISC"]
        .iter()
        .any(|item| normalized.contains(item))
    {
        "需履行声明义务"
    } else if ["GPL", "AGPL", "LGPL", "EUPL", "MPL"]
        .iter()
        .any(|item| normalized.contains(item))
    {
        "强/弱 Copyleft：需法务核对传播义务"
    } else if normalized.contains("PROPRIETARY") || normalized.contains("UNLICENSED") {
        "高风险：专有或禁止分发"
    } else {
        "未知标识：需人工核对许可证正文"
    }
}

fn is_external_requirement(value: &str) -> bool {
    let value = value.to_ascii_lowercase();
    [
        "git+",
        "git://",
        "http://",
        "https://",
        "file:",
        "link:",
        "workspace:",
    ]
    .iter()
    .any(|prefix| value.starts_with(prefix))
}

fn security_status(
    deprecated: Option<&str>,
    lifecycle_scripts: &[String],
    external_dependencies: &[String],
    integrity: &str,
) -> String {
    let mut risks = Vec::new();
    if deprecated.is_some() {
        risks.push("包已废弃");
    }
    if !lifecycle_scripts.is_empty() {
        risks.push("包含安装期脚本");
    }
    if !external_dependencies.is_empty() {
        risks.push("依赖绕过 registry");
    }
    if integrity.contains("未提供") {
        risks.push("缺少完整性摘要");
    }
    if risks.is_empty() {
        "未发现元数据级高风险；漏洞状态未知".to_string()
    } else {
        format!("需审查：{}；漏洞状态未知", risks.join("、"))
    }
}

fn version_relation(selected: &str, latest: &str) -> &'static str {
    match compare_versions(selected, latest) {
        std::cmp::Ordering::Less => "落后于 latest",
        std::cmp::Ordering::Equal => "当前 latest",
        std::cmp::Ordering::Greater => "高于 latest（可能为预发布或标签异常）",
    }
}

fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    let parse = |value: &str| {
        let (core, prerelease) = value
            .trim_start_matches('v')
            .split_once('-')
            .unwrap_or((value.trim_start_matches('v'), ""));
        let mut numbers = core
            .split('.')
            .map(|part| part.parse::<u64>().unwrap_or(0))
            .collect::<Vec<_>>();
        numbers.resize(3, 0);
        (numbers, prerelease.to_string())
    };
    let (a_numbers, a_pre) = parse(a);
    let (b_numbers, b_pre) = parse(b);
    a_numbers
        .cmp(&b_numbers)
        .then_with(|| match (a_pre.is_empty(), b_pre.is_empty()) {
            (true, false) => std::cmp::Ordering::Greater,
            (false, true) => std::cmp::Ordering::Less,
            _ => a_pre.cmp(&b_pre),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_version_compatibility_license_and_supply_chain_risks() {
        let metadata = serde_json::json!({
            "dist-tags": { "latest": "2.0.0" },
            "versions": {
                "1.5.0": { "license": "MIT" },
                "2.0.0": {
                    "license": "Apache-2.0",
                    "compatibleSdkVersion": "5.0.0(12)",
                    "repository": { "url": "https://gitee.com/example/pkg.git" },
                    "dependencies": { "safe": "^1.0.0", "risky": "git+https://example.test/risky.git" },
                    "scripts": { "postinstall": "node setup.js" },
                    "dist": { "integrity": "sha512-example" }
                }
            }
        });
        let audit = parse(
            &metadata,
            "@example/pkg",
            Some("1.5.0"),
            Some(11),
            "https://registry.test/pkg".into(),
        )
        .unwrap();
        assert_eq!(audit.version_relation, "落后于 latest");
        assert_eq!(audit.selected_version, "1.5.0");
        assert_eq!(audit.license, "MIT");
        assert_eq!(audit.api_compatible, None);

        let latest = parse(
            &metadata,
            "@example/pkg",
            None,
            Some(11),
            "https://registry.test/pkg".into(),
        )
        .unwrap();
        assert_eq!(latest.minimum_api, Some(12));
        assert_eq!(latest.api_compatible, Some(false));
        assert_eq!(latest.lifecycle_scripts, vec!["postinstall"]);
        assert_eq!(latest.external_dependencies.len(), 1);
        assert!(latest.security_status.contains("需审查"));
    }

    #[test]
    fn stable_release_sorts_after_prerelease() {
        assert_eq!(
            compare_versions("2.0.0", "2.0.0-beta.1"),
            std::cmp::Ordering::Greater
        );
        assert_eq!(
            compare_versions("10.0.0", "9.9.9"),
            std::cmp::Ordering::Greater
        );
    }

    #[test]
    fn falls_back_to_semantic_latest_and_parses_engine_constraint() {
        let metadata = serde_json::json!({
            "versions": {
                "9.9.9": {},
                "10.0.0-beta.1": {},
                "10.0.0": { "engines": { "OpenHarmony": ">=12" } }
            },
            "time": { "10.0.0": "2026-08-20T10:00:00.000Z" }
        });
        let audit = parse(
            &metadata,
            "pkg",
            None,
            Some(12),
            "https://registry.test/pkg".into(),
        )
        .unwrap();
        assert_eq!(audit.latest_version, "10.0.0");
        assert_eq!(audit.minimum_api, Some(12));
        assert_eq!(audit.api_compatible, Some(true));
        assert_eq!(
            audit.published_at.as_deref(),
            Some("2026-08-20T10:00:00.000Z")
        );
    }
}
