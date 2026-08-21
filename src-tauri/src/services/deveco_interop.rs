//! DevEco Studio 公共工程配置互操作报告。
//!
//! 只把可提交的 HarmonyOS/Hvigor/OHPM 配置视为构建契约；`.idea` 与
//! `local.properties` 仅报告存在性，不读取内容，也不进入公共配置指纹。

use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

use crate::services::harmony_model::HarmonySemanticModel;

const MAX_CONFIG_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PublicConfig {
    pub path: String,
    pub role: String,
    pub present: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DevEcoInteropReport {
    pub schema_version: u32,
    pub public_configs: Vec<PublicConfig>,
    pub public_contract_sha256: String,
    pub products: Vec<String>,
    pub modules: Vec<String>,
    pub project_hvigor_wrapper: bool,
    pub external_hvigor_available: bool,
    pub ide_private_state_present: bool,
    pub ide_private_state_required: bool,
    pub local_properties_present: bool,
    pub machine_local_path_fields: Vec<String>,
    pub sensitive_config_fields: Vec<String>,
    pub cli_ready: bool,
    pub risks: Vec<String>,
}

pub fn analyze(
    root: &Path,
    model: &HarmonySemanticModel,
    external_hvigor_available: bool,
) -> DevEcoInteropReport {
    let configs = public_config_candidates(model);
    let public_configs = configs
        .iter()
        .map(|(path, role)| PublicConfig {
            path: path.clone(),
            role: role.clone(),
            present: root.join(path).is_file(),
        })
        .collect::<Vec<_>>();
    let public_contract_sha256 = public_contract_hash(root, &configs);
    let project_hvigor_wrapper = [
        "hvigorw",
        "hvigorw.bat",
        "hvigor/hvigor-wrapper.js",
        "hvigor/hvigorw.js",
    ]
    .iter()
    .any(|path| root.join(path).is_file());
    let mut machine_local_path_fields = Vec::new();
    let mut sensitive_config_fields = Vec::new();
    for (path, _) in &configs {
        inspect_config_fields(
            root,
            path,
            &mut machine_local_path_fields,
            &mut sensitive_config_fields,
        );
    }
    machine_local_path_fields.sort();
    machine_local_path_fields.dedup();
    sensitive_config_fields.sort();
    sensitive_config_fields.dedup();

    let root_profile = root.join("build-profile.json5").is_file();
    let app_manifest = root.join("AppScope/app.json5").is_file();
    let cli_ready = root_profile
        && app_manifest
        && !model.modules.is_empty()
        && (project_hvigor_wrapper || external_hvigor_available);
    let mut risks = Vec::new();
    if !root_profile {
        risks.push(
            "缺少根 build-profile.json5，DevEco 与 CLI 无法共享 product/module 契约。".into(),
        );
    }
    if !app_manifest {
        risks.push("缺少 AppScope/app.json5，应用身份无法由公开配置复现。".into());
    }
    if !project_hvigor_wrapper && !external_hvigor_available {
        risks.push("工程包装脚本和外部 Hvigor 均不可用，命令行构建不可复现。".into());
    }
    if !machine_local_path_fields.is_empty() {
        risks.push("公开配置含机器绝对路径；应改为环境变量、相对路径或隔离凭据引用。".into());
    }
    if !sensitive_config_fields.is_empty() {
        risks.push("公开配置含敏感字段；报告仅显示字段路径，值未写入输出。".into());
    }
    if root.join("local.properties").is_file() {
        risks.push("local.properties 属于机器本地提示，不进入公共配置指纹或构建事实。".into());
    }
    if root.join(".idea").exists() {
        risks.push("检测到 .idea；窗口、缓存、最近文件等 IDE 私有状态被明确忽略。".into());
    }

    DevEcoInteropReport {
        schema_version: 1,
        public_configs,
        public_contract_sha256,
        products: model
            .products
            .iter()
            .map(|product| product.name.clone())
            .collect(),
        modules: model
            .modules
            .iter()
            .map(|module| format!("{}:{}", module.name, module.artifact_kind))
            .collect(),
        project_hvigor_wrapper,
        external_hvigor_available,
        ide_private_state_present: root.join(".idea").exists(),
        ide_private_state_required: false,
        local_properties_present: root.join("local.properties").is_file(),
        machine_local_path_fields,
        sensitive_config_fields,
        cli_ready,
        risks,
    }
}

pub fn render(report: &DevEcoInteropReport) -> String {
    let configs = report
        .public_configs
        .iter()
        .map(|config| {
            format!(
                "{}={} ({})",
                config.path,
                if config.present { "present" } else { "missing" },
                config.role
            )
        })
        .collect::<Vec<_>>()
        .join("；");
    format!(
        "[DevEco 公共配置互操作]\n- CLI 可复现: {}\n- 公共配置指纹: {}\n- products: {}\n- modules: {}\n- 配置: {}\n- Hvigor: project_wrapper={} external={}\n- IDE 私有状态: present={} required=false（不读取 .idea 内容）\n- local.properties: present={}（不读取、不进入指纹）\n- 机器路径字段: {}\n- 敏感字段: {}\n- 风险: {}\n",
        report.cli_ready,
        &report.public_contract_sha256[..12],
        display_or_none(&report.products),
        display_or_none(&report.modules),
        configs,
        report.project_hvigor_wrapper,
        report.external_hvigor_available,
        report.ide_private_state_present,
        report.local_properties_present,
        display_or_none(&report.machine_local_path_fields),
        display_or_none(&report.sensitive_config_fields),
        display_or_none(&report.risks)
    )
}

fn public_config_candidates(model: &HarmonySemanticModel) -> Vec<(String, String)> {
    let mut configs = vec![
        ("AppScope/app.json5".into(), "application_manifest".into()),
        (
            "build-profile.json5".into(),
            "product_module_contract".into(),
        ),
        ("oh-package.json5".into(), "root_dependencies".into()),
        ("oh-package-lock.json5".into(), "dependency_lock".into()),
        ("hvigorfile.ts".into(), "root_build_logic".into()),
        (
            "hvigor/hvigor-config.json5".into(),
            "build_runtime_config".into(),
        ),
    ];
    for module in &model.modules {
        for (name, role) in [
            ("build-profile.json5", "module_build_contract"),
            ("oh-package.json5", "module_dependencies"),
            ("oh-package-lock.json5", "module_dependency_lock"),
            ("hvigorfile.ts", "module_build_logic"),
            ("src/main/module.json5", "module_manifest"),
        ] {
            configs.push((
                PathBuf::from(&module.rel_path)
                    .join(name)
                    .to_string_lossy()
                    .replace('\\', "/"),
                role.into(),
            ));
        }
    }
    configs.sort();
    configs.dedup();
    configs
}

fn public_contract_hash(root: &Path, configs: &[(String, String)]) -> String {
    let mut hasher = Sha256::new();
    for (path, _) in configs {
        let full = root.join(path);
        let Some(content) = fs::metadata(&full)
            .ok()
            .filter(|metadata| metadata.is_file() && metadata.len() <= MAX_CONFIG_BYTES)
            .and_then(|_| fs::read(&full).ok())
        else {
            continue;
        };
        hasher.update(path.as_bytes());
        hasher.update([0]);
        hasher.update(content);
        hasher.update([0xff]);
    }
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn inspect_config_fields(
    root: &Path,
    relative: &str,
    local_paths: &mut Vec<String>,
    sensitive: &mut Vec<String>,
) {
    if !relative.ends_with(".json5") {
        return;
    }
    let Ok(content) = fs::read_to_string(root.join(relative)) else {
        return;
    };
    let Ok(value) = crate::services::harmony::parse_json5(&content) else {
        return;
    };
    inspect_value(relative, "$", &value, local_paths, sensitive);
}

fn inspect_value(
    file: &str,
    path: &str,
    value: &serde_json::Value,
    local_paths: &mut Vec<String>,
    sensitive: &mut Vec<String>,
) {
    match value {
        serde_json::Value::Object(values) => {
            for (key, value) in values {
                let field = format!("{file}:{path}.{key}");
                let lower = key.to_ascii_lowercase();
                if [
                    "password",
                    "storepassword",
                    "keypassword",
                    "token",
                    "secret",
                ]
                .iter()
                .any(|marker| lower.contains(marker))
                {
                    sensitive.push(field.clone());
                }
                inspect_value(
                    file,
                    &format!("{path}.{key}"),
                    value,
                    local_paths,
                    sensitive,
                );
            }
        }
        serde_json::Value::Array(values) => {
            for (index, value) in values.iter().enumerate() {
                inspect_value(
                    file,
                    &format!("{path}[{index}]"),
                    value,
                    local_paths,
                    sensitive,
                );
            }
        }
        serde_json::Value::String(value) if is_absolute_machine_path(value) => {
            local_paths.push(format!("{file}:{path}"));
        }
        _ => {}
    }
}

fn is_absolute_machine_path(value: &str) -> bool {
    value.starts_with('/')
        || value.starts_with("\\\\")
        || value
            .as_bytes()
            .get(1)
            .is_some_and(|separator| *separator == b':')
}

fn display_or_none(values: &[String]) -> String {
    if values.is_empty() {
        "none".into()
    } else {
        values.join("；")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::harmony_model::{HarmonyModule, HarmonyProduct};

    #[test]
    fn private_ide_state_does_not_change_public_contract() {
        let root = std::env::temp_dir().join(format!("deveco-interop-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join("AppScope")).unwrap();
        fs::create_dir_all(root.join("entry/src/main")).unwrap();
        fs::create_dir_all(root.join("hvigor")).unwrap();
        fs::create_dir_all(root.join(".idea")).unwrap();
        fs::write(
            root.join("AppScope/app.json5"),
            "{app:{bundleName:'com.demo'}}",
        )
        .unwrap();
        fs::write(
            root.join("build-profile.json5"),
            "{app:{products:[],modules:[]}}",
        )
        .unwrap();
        fs::write(root.join("hvigor/hvigor-config.json5"), "{}").unwrap();
        fs::write(root.join("hvigorw"), "wrapper").unwrap();
        fs::write(root.join("local.properties"), "sdk.dir=/private/sdk").unwrap();
        fs::write(root.join(".idea/workspace.xml"), "private-a").unwrap();
        let model = HarmonySemanticModel {
            products: vec![HarmonyProduct {
                name: "default".into(),
                ..Default::default()
            }],
            modules: vec![HarmonyModule {
                name: "entry".into(),
                rel_path: "entry".into(),
                artifact_kind: "hap".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let first = analyze(&root, &model, false);
        fs::write(root.join(".idea/workspace.xml"), "private-b").unwrap();
        fs::write(root.join("local.properties"), "sdk.dir=/different/sdk").unwrap();
        let second = analyze(&root, &model, false);
        assert!(first.cli_ready);
        assert!(!first.ide_private_state_required);
        assert_eq!(first.public_contract_sha256, second.public_contract_sha256);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reports_sensitive_and_machine_path_fields_without_values() {
        let root = std::env::temp_dir().join(format!("deveco-fields-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("build-profile.json5"),
            r#"{"app":{"signingConfigs":[{"storePassword":"do-not-render","certpath":"/Users/demo/cert.cer"}]}}"#,
        )
        .unwrap();
        let report = analyze(&root, &HarmonySemanticModel::default(), true);
        assert!(report
            .sensitive_config_fields
            .iter()
            .any(|field| field.contains("storePassword")));
        assert!(report
            .machine_local_path_fields
            .iter()
            .any(|field| field.contains("certpath")));
        let rendered = render(&report);
        assert!(!rendered.contains("do-not-render"));
        assert!(!rendered.contains("/Users/demo/cert.cer"));
        fs::remove_dir_all(root).unwrap();
    }
}
