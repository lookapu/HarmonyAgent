//! HarmonyOS API 使用、权限、设备能力和模块配置的一致性审计。

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, params};
use serde::Serialize;

use crate::services::harmony_model::{HarmonyModule, HarmonySemanticModel};
use crate::services::sdk_api::{ApiIndex, ApiModule, ProjectApiContext};

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ConsistencyIssue {
    /// error / warning / info
    pub severity: String,
    pub code: String,
    pub module: String,
    pub source: String,
    pub message: String,
    pub evidence: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct HarmonyConsistencyReport {
    pub files_scanned: usize,
    pub api_imports: usize,
    pub errors: usize,
    pub warnings: usize,
    pub infos: usize,
    pub issues: Vec<ConsistencyIssue>,
}

#[derive(Debug, Clone)]
struct ApiUsage {
    owner_module: String,
    sdk_module: String,
    symbols: Vec<String>,
    source_file: String,
    line: usize,
}

/// 执行只读一致性审计。`official_db` 可选；缺失时只跳过官方设备类型证据。
pub fn analyze(
    root: &Path,
    model: &HarmonySemanticModel,
    context: &ProjectApiContext,
    index: Option<&ApiIndex>,
    official_db: Option<&Connection>,
) -> HarmonyConsistencyReport {
    let (files_scanned, usages) = collect_api_usages(root, &model.modules);
    let modules = index
        .map(|index| {
            index
                .modules
                .iter()
                .map(|module| (module.module.to_ascii_lowercase(), module))
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default();
    let mut issues = Vec::new();

    audit_module_configuration(model, &mut issues);
    if index.is_none() && !usages.is_empty() {
        issues.push(ConsistencyIssue {
            severity: "info".into(),
            code: "sdk_index_unavailable".into(),
            module: ".".into(),
            source: context.project_path.clone(),
            message: "未找到本机 SDK API 索引，本轮跳过 API 版本、权限和 SystemCapability 精确核对"
                .into(),
            evidence: vec!["配置可用 SDK 后重新运行 check_sdk_alignment".into()],
        });
    } else {
        for usage in &usages {
            audit_usage(
                model,
                context,
                usage,
                modules.get(&usage.sdk_module.to_ascii_lowercase()).copied(),
                official_db,
                &mut issues,
            );
        }
    }

    issues.sort_by(|left, right| {
        severity_rank(&left.severity)
            .cmp(&severity_rank(&right.severity))
            .then_with(|| {
                (&left.module, &left.source, &left.code).cmp(&(
                    &right.module,
                    &right.source,
                    &right.code,
                ))
            })
    });
    issues.dedup_by(|left, right| {
        left.code == right.code
            && left.module == right.module
            && left.source == right.source
            && left.message == right.message
    });
    HarmonyConsistencyReport {
        files_scanned,
        api_imports: usages.len(),
        errors: issues
            .iter()
            .filter(|issue| issue.severity == "error")
            .count(),
        warnings: issues
            .iter()
            .filter(|issue| issue.severity == "warning")
            .count(),
        infos: issues
            .iter()
            .filter(|issue| issue.severity == "info")
            .count(),
        issues,
    }
}

pub fn render(report: &HarmonyConsistencyReport) -> String {
    let mut out = format!(
        "工程一致性审计：扫描 {} 个源码文件、{} 条 SDK API import；{} error / {} warning / {} info",
        report.files_scanned, report.api_imports, report.errors, report.warnings, report.infos
    );
    if report.issues.is_empty() {
        out.push_str("\n- 未发现 API、权限、设备能力或模块配置不一致。");
        return out;
    }
    for issue in report.issues.iter().take(80) {
        out.push_str(&format!(
            "\n- [{}] {} · {} · {}：{}",
            issue.severity.to_ascii_uppercase(),
            issue.code,
            issue.module,
            issue.source,
            issue.message
        ));
        for evidence in issue.evidence.iter().take(3) {
            out.push_str(&format!("\n  证据：{evidence}"));
        }
    }
    if report.issues.len() > 80 {
        out.push_str(&format!("\n- 其余 {} 条已省略。", report.issues.len() - 80));
    }
    out
}

fn audit_module_configuration(model: &HarmonySemanticModel, issues: &mut Vec<ConsistencyIssue>) {
    let included_modules = model
        .products
        .iter()
        .flat_map(|product| product.modules.iter().cloned())
        .collect::<BTreeSet<_>>();
    for module in &model.modules {
        let manifest = format!("{}/src/main/module.json5", module.rel_path);
        if !included_modules.contains(&module.rel_path) && !included_modules.contains(&module.name)
        {
            push_issue(
                issues,
                "error",
                "module_not_in_product",
                module,
                &manifest,
                "模块未被任何 product 纳入，源码与配置不会进入预期构建产物",
                model
                    .products
                    .iter()
                    .map(|product| {
                        format!(
                            "product {} modules={}",
                            product.name,
                            product.modules.join(",")
                        )
                    })
                    .collect(),
            );
        }
        if module.device_types.is_empty() {
            push_issue(
                issues,
                "warning",
                "device_types_missing",
                module,
                &manifest,
                "module.json5 未声明 deviceTypes，无法证明模块的目标设备范围",
                Vec::new(),
            );
        }
        if module.artifact_kind == "hap" {
            match module.main_element.as_deref() {
                Some(main) if module.abilities.iter().any(|ability| ability.name == main) => {}
                Some(main) => push_issue(
                    issues,
                    "error",
                    "main_element_missing",
                    module,
                    &manifest,
                    &format!("mainElement={main} 未匹配 abilities 中的入口"),
                    module
                        .abilities
                        .iter()
                        .map(|ability| format!("ability={}", ability.name))
                        .collect(),
                ),
                None => push_issue(
                    issues,
                    "warning",
                    "main_element_missing",
                    module,
                    &manifest,
                    "HAP 模块未声明 mainElement",
                    Vec::new(),
                ),
            }
        }
        let abilities = module
            .abilities
            .iter()
            .map(|ability| ability.name.as_str())
            .chain(
                module
                    .extension_abilities
                    .iter()
                    .map(|ability| ability.name.as_str()),
            )
            .collect::<BTreeSet<_>>();
        for permission in &module.permissions {
            if (!permission.abilities.is_empty() || permission.when.is_some())
                && permission
                    .reason
                    .as_deref()
                    .is_none_or(|reason| reason.trim().is_empty())
            {
                push_issue(
                    issues,
                    "warning",
                    "permission_reason_missing",
                    module,
                    &manifest,
                    &format!("权限 {} 未提供 reason", permission.name),
                    Vec::new(),
                );
            }
            for ability in &permission.abilities {
                if !abilities.contains(ability.as_str()) {
                    push_issue(
                        issues,
                        "error",
                        "permission_used_scene_invalid",
                        module,
                        &manifest,
                        &format!(
                            "权限 {} 的 usedScene 引用了不存在的 Ability {ability}",
                            permission.name
                        ),
                        abilities
                            .iter()
                            .map(|value| format!("available={value}"))
                            .collect(),
                    );
                }
            }
        }
    }
}

fn audit_usage(
    model: &HarmonySemanticModel,
    context: &ProjectApiContext,
    usage: &ApiUsage,
    sdk_module: Option<&ApiModule>,
    official_db: Option<&Connection>,
    issues: &mut Vec<ConsistencyIssue>,
) {
    let Some(owner) = model
        .modules
        .iter()
        .find(|module| module.rel_path == usage.owner_module)
    else {
        return;
    };
    let Some(sdk_module) = sdk_module else {
        push_issue(
            issues,
            "error",
            "sdk_module_unresolved",
            owner,
            &format!("{}:{}", usage.source_file, usage.line),
            &format!("本机 SDK 索引中找不到模块 {}", usage.sdk_module),
            vec!["确认 import 名称、compileSdkVersion 与本机 SDK 安装内容".into()],
        );
        return;
    };

    if context
        .compile_api
        .or(context.installed_api)
        .zip(sdk_module.since_min)
        .is_some_and(|(compile, since)| since > compile)
    {
        push_issue(
            issues,
            "error",
            "api_above_compile_sdk",
            owner,
            &format!("{}:{}", usage.source_file, usage.line),
            &format!(
                "{} 从 API {} 引入，高于当前 compile API {}",
                usage.sdk_module,
                sdk_module.since_min.unwrap_or_default(),
                context
                    .compile_api
                    .or(context.installed_api)
                    .unwrap_or_default()
            ),
            vec![sdk_module.path.clone()],
        );
    }

    let declared_permissions = owner
        .permissions
        .iter()
        .map(|permission| permission.name.as_str())
        .collect::<BTreeSet<_>>();
    let capability_guards = model
        .graph
        .system_capabilities
        .iter()
        .filter(|reference| reference.module == owner.rel_path)
        .map(|reference| reference.capability.as_str())
        .collect::<BTreeSet<_>>();
    let mut matched_symbol = false;
    for symbol_name in &usage.symbols {
        let Some(symbol) = sdk_module
            .symbols
            .iter()
            .find(|symbol| symbol.name.eq_ignore_ascii_case(symbol_name))
        else {
            continue;
        };
        matched_symbol = true;
        let source = format!("{}:{}", usage.source_file, usage.line);
        if symbol.deprecated {
            push_issue(
                issues,
                "warning",
                "deprecated_api",
                owner,
                &source,
                &format!("{}::{} 已废弃", usage.sdk_module, symbol.name),
                symbol
                    .replacement
                    .as_deref()
                    .map(|replacement| vec![format!("@useinstead {replacement}")])
                    .unwrap_or_default(),
            );
        }
        if symbol
            .since
            .zip(context.compile_api.or(context.installed_api))
            .is_some_and(|(since, compile)| since > compile)
        {
            push_issue(
                issues,
                "error",
                "api_above_compile_sdk",
                owner,
                &source,
                &format!(
                    "{}::{} 从 API {} 引入，高于当前 compile API {}",
                    usage.sdk_module,
                    symbol.name,
                    symbol.since.unwrap_or_default(),
                    context
                        .compile_api
                        .or(context.installed_api)
                        .unwrap_or_default()
                ),
                vec![sdk_module.path.clone()],
            );
        }
        for permission in &symbol.permissions {
            if !declared_permissions.contains(permission.as_str()) {
                push_issue(
                    issues,
                    "error",
                    "api_permission_missing",
                    owner,
                    &source,
                    &format!(
                        "{}::{} 需要权限 {permission}，但模块未声明",
                        usage.sdk_module, symbol.name
                    ),
                    vec![format!("SDK declaration={}", sdk_module.path)],
                );
            }
        }
        if let Some(capability) = symbol.syscap.as_deref() {
            if owner.device_types.len() > 1 && !capability_guards.contains(capability) {
                push_issue(
                    issues,
                    "warning",
                    "system_capability_unguarded",
                    owner,
                    &source,
                    &format!(
                        "{}::{} 依赖 {}，模块覆盖多种设备但未发现 canIUse 守卫",
                        usage.sdk_module, symbol.name, capability
                    ),
                    vec![format!("deviceTypes={}", owner.device_types.join(","))],
                );
            }
        }
    }

    if !matched_symbol && !sdk_module.permissions.is_empty() {
        push_issue(
            issues,
            "info",
            "module_permission_review",
            owner,
            &format!("{}:{}", usage.source_file, usage.line),
            &format!(
                "{} 含权限型 API；当前 import 形态无法精确到成员，请按实际调用复核",
                usage.sdk_module
            ),
            sdk_module
                .permissions
                .iter()
                .map(|permission| format!("possible={permission}"))
                .collect(),
        );
    }

    if let Some(device_types) = official_device_types(official_db, &usage.sdk_module) {
        let unsupported = owner
            .device_types
            .iter()
            .filter(|device| {
                !device_types
                    .iter()
                    .any(|supported| supported.eq_ignore_ascii_case(device))
            })
            .cloned()
            .collect::<Vec<_>>();
        if !unsupported.is_empty() {
            push_issue(
                issues,
                "warning",
                "api_device_type_mismatch",
                owner,
                &format!("{}:{}", usage.source_file, usage.line),
                &format!(
                    "{} 的官方参考未覆盖模块声明的设备类型：{}",
                    usage.sdk_module,
                    unsupported.join(", ")
                ),
                vec![format!("official deviceTypes={}", device_types.join(","))],
            );
        }
    }
}

fn collect_api_usages(root: &Path, modules: &[HarmonyModule]) -> (usize, Vec<ApiUsage>) {
    let mut files_scanned = 0;
    let mut usages = Vec::new();
    for module in modules {
        let module_root = if module.rel_path == "." {
            root.to_path_buf()
        } else {
            root.join(&module.rel_path)
        };
        let mut files = Vec::new();
        collect_source_files(&module_root.join("src/main"), 0, &mut files);
        files.sort();
        files.truncate(2_000usize.saturating_sub(files_scanned));
        files_scanned += files.len();
        for path in files {
            let Ok(text) = fs::read_to_string(&path) else {
                continue;
            };
            let source_file = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            for (line_index, sdk_module, symbols) in parse_sdk_imports(&text) {
                usages.push(ApiUsage {
                    owner_module: module.rel_path.clone(),
                    sdk_module,
                    symbols,
                    source_file: source_file.clone(),
                    line: line_index,
                });
            }
        }
    }
    (files_scanned, usages)
}

fn collect_source_files(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > 12 || out.len() >= 2_000 {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            let name = entry.file_name();
            if !name.to_string_lossy().starts_with('.') {
                collect_source_files(&path, depth + 1, out);
            }
        } else if path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| matches!(extension, "ets" | "ts"))
        {
            out.push(path);
        }
    }
}

fn parse_sdk_import(line: &str) -> Option<(String, Vec<String>)> {
    let trimmed = line.trim();
    if !trimmed.starts_with("import ") && !trimmed.starts_with("export ") {
        return None;
    }
    let from = trimmed.find(" from ")?;
    let bindings = trimmed[..from]
        .trim_start_matches("import ")
        .trim_start_matches("export ")
        .trim();
    let tail = &trimmed[from + 6..];
    let quote_start = tail.find(['\'', '"'])?;
    let quote = tail.as_bytes()[quote_start] as char;
    let value = &tail[quote_start + 1..];
    let quote_end = value.find(quote)?;
    let sdk_module = value[..quote_end].to_string();
    if !sdk_module.starts_with("@ohos.") && !sdk_module.starts_with("@kit.") {
        return None;
    }
    let symbols = bindings
        .strip_prefix('{')
        .and_then(|value| value.strip_suffix('}'))
        .map(|value| {
            value
                .split(',')
                .filter_map(|binding| {
                    let name = binding.split_whitespace().next()?;
                    (!name.is_empty()).then(|| name.to_string())
                })
                .collect()
        })
        .unwrap_or_default();
    Some((sdk_module, symbols))
}

fn parse_sdk_imports(text: &str) -> Vec<(usize, String, Vec<String>)> {
    let lines = text.lines().collect::<Vec<_>>();
    let mut out = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let trimmed = lines[index].trim();
        if !trimmed.starts_with("import ") && !trimmed.starts_with("export ") {
            index += 1;
            continue;
        }
        let start = index;
        let mut statement = trimmed.to_string();
        while index + 1 < lines.len()
            && !statement.contains(" from ")
            && index.saturating_sub(start) < 20
        {
            index += 1;
            statement.push(' ');
            statement.push_str(lines[index].trim());
        }
        if let Some((module, symbols)) = parse_sdk_import(&statement) {
            out.push((start + 1, module, symbols));
        }
        index += 1;
    }
    out
}

fn official_device_types(db: Option<&Connection>, module: &str) -> Option<Vec<String>> {
    let conn = db?;
    let value = conn
        .query_row(
            "SELECT device_types FROM api_details WHERE lower(module)=lower(?1) AND device_types IS NOT NULL LIMIT 1",
            params![module],
            |row| row.get::<_, String>(0),
        )
        .ok()?;
    let lower = value.to_ascii_lowercase();
    let known = ["phone", "tablet", "2in1", "pc", "tv", "wearable", "car"];
    let devices = known
        .iter()
        .filter(|device| lower.contains(**device))
        .map(|device| (*device).to_string())
        .collect::<Vec<_>>();
    (!devices.is_empty()).then_some(devices)
}

fn push_issue(
    issues: &mut Vec<ConsistencyIssue>,
    severity: &str,
    code: &str,
    module: &HarmonyModule,
    source: &str,
    message: &str,
    evidence: Vec<String>,
) {
    issues.push(ConsistencyIssue {
        severity: severity.into(),
        code: code.into(),
        module: module.rel_path.clone(),
        source: source.into(),
        message: message.into(),
        evidence,
    });
}

fn severity_rank(severity: &str) -> usize {
    match severity {
        "error" => 0,
        "warning" => 1,
        _ => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::harmony_model::{HarmonyAbility, HarmonyPermission, HarmonyProduct};
    use crate::services::sdk_api::ApiSymbol;

    #[test]
    fn detects_missing_permission_api_level_capability_and_manifest_errors() {
        let root = std::env::temp_dir().join(format!("harmony-consistency-{}", std::process::id()));
        fs::remove_dir_all(&root).ok();
        fs::create_dir_all(root.join("entry/src/main/ets")).unwrap();
        fs::write(
            root.join("entry/src/main/ets/Index.ets"),
            "import { CameraManager } from '@ohos.multimedia.camera'\n",
        )
        .unwrap();
        let model = HarmonySemanticModel {
            products: vec![HarmonyProduct {
                name: "default".into(),
                modules: vec!["entry".into()],
                ..Default::default()
            }],
            modules: vec![HarmonyModule {
                name: "entry".into(),
                rel_path: "entry".into(),
                artifact_kind: "hap".into(),
                device_types: vec!["phone".into(), "tablet".into()],
                main_element: Some("MissingAbility".into()),
                abilities: vec![HarmonyAbility {
                    name: "EntryAbility".into(),
                    ..Default::default()
                }],
                permissions: vec![HarmonyPermission {
                    name: "ohos.permission.LOCATION".into(),
                    abilities: vec!["GhostAbility".into()],
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        };
        let index = ApiIndex {
            modules: vec![ApiModule {
                module: "@ohos.multimedia.camera".into(),
                since_min: Some(14),
                symbols: vec![ApiSymbol {
                    name: "CameraManager".into(),
                    kind: "class".into(),
                    since: Some(14),
                    deprecated: false,
                    syscap: Some("SystemCapability.Multimedia.Camera.Core".into()),
                    permissions: vec!["ohos.permission.CAMERA".into()],
                    replacement: None,
                }],
                path: "/sdk/@ohos.multimedia.camera.d.ts".into(),
                ..empty_api_module()
            }],
            ..Default::default()
        };
        let report = analyze(
            &root,
            &model,
            &ProjectApiContext {
                compile_api: Some(12),
                ..Default::default()
            },
            Some(&index),
            None,
        );
        for code in [
            "api_above_compile_sdk",
            "api_permission_missing",
            "system_capability_unguarded",
            "main_element_missing",
            "permission_used_scene_invalid",
        ] {
            assert!(
                report.issues.iter().any(|issue| issue.code == code),
                "missing {code}"
            );
        }
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn parses_named_sdk_imports_only() {
        let (module, symbols) =
            parse_sdk_import("import { UIAbility, Want as AbilityWant } from '@kit.AbilityKit';")
                .unwrap();
        assert_eq!(module, "@kit.AbilityKit");
        assert_eq!(symbols, vec!["UIAbility", "Want"]);
        assert!(parse_sdk_import("import local from './local'").is_none());
        let multiline = parse_sdk_imports(
            "import {\n  CameraManager,\n  CameraInput as Input\n} from '@ohos.multimedia.camera';\n",
        );
        assert_eq!(multiline[0].0, 1);
        assert_eq!(multiline[0].2, vec!["CameraManager", "CameraInput"]);
    }

    fn empty_api_module() -> ApiModule {
        ApiModule {
            module: String::new(),
            kit: None,
            syscap: None,
            system_capabilities: Vec::new(),
            permissions: Vec::new(),
            since_min: None,
            since_max: None,
            declarations: Vec::new(),
            symbols: Vec::new(),
            deprecated: false,
            path: String::new(),
        }
    }
}
