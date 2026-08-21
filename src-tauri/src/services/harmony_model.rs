//! HarmonyOS 工程统一语义模型。
//!
//! 这里是工程、产品、模块、产物类型、Ability 与依赖关系的单一解析真源。
//! 构建/部署所需的精简摘要和 Workspace 能力分析都应从本模型派生。

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

const SKIP_DIRS: &[&str] = &[
    ".git",
    ".hvigor",
    ".idea",
    ".ohpm",
    "build",
    "node_modules",
    "oh_modules",
];

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HarmonySemanticModel {
    pub schema_version: u32,
    pub app: HarmonyApp,
    pub signing_configs: Vec<String>,
    pub products: Vec<HarmonyProduct>,
    pub modules: Vec<HarmonyModule>,
    pub dependencies: Vec<HarmonyDependency>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HarmonyApp {
    pub bundle_name: Option<String>,
    pub version_code: Option<i64>,
    pub version_name: Option<String>,
    pub label: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HarmonyProduct {
    pub name: String,
    pub compile_sdk_version: Option<String>,
    pub compatible_sdk_version: Option<String>,
    pub target_sdk_version: Option<String>,
    pub signing_config: Option<String>,
    /// 由 module target 的 applyToProducts 反向计算；没有显式约束时模块属于全部产品。
    pub modules: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HarmonyModule {
    pub name: String,
    pub rel_path: String,
    pub src_path: String,
    pub kind: String,
    /// hap / hsp / har / unknown
    pub artifact_kind: String,
    pub package_name: Option<String>,
    pub device_types: Vec<String>,
    pub main_element: Option<String>,
    pub targets: Vec<HarmonyTarget>,
    pub abilities: Vec<HarmonyAbility>,
    pub extension_abilities: Vec<HarmonyExtensionAbility>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HarmonyTarget {
    pub name: String,
    pub products: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HarmonyAbility {
    pub name: String,
    pub src_entry: Option<String>,
    pub exported: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HarmonyExtensionAbility {
    pub name: String,
    pub extension_type: Option<String>,
    pub src_entry: Option<String>,
    pub exported: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HarmonyDependency {
    /// `.` 表示工程根清单，否则为模块相对路径。
    pub from_module: String,
    pub name: String,
    pub requirement: String,
    /// dependencies / devDependencies / dynamicDependencies
    pub scope: String,
    /// 能解析到工作区内模块时给出目标模块路径。
    pub target_module: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct RootModuleDecl {
    name: String,
    rel_path: String,
    targets: Vec<HarmonyTarget>,
}

/// 容错解析完整 HarmonyOS 工程。任一清单损坏只会使对应部分缺失，不阻断其它信息。
pub fn parse(root: &Path) -> HarmonySemanticModel {
    let mut model = HarmonySemanticModel {
        schema_version: 1,
        ..HarmonySemanticModel::default()
    };
    model.app = parse_app(root);
    let (mut products, declarations, signing_configs) = parse_root_profile(root);
    model.signing_configs = signing_configs;

    let mut module_paths = declarations
        .iter()
        .map(|d| d.rel_path.clone())
        .collect::<BTreeSet<_>>();
    discover_module_paths(root, root, 0, &mut module_paths);
    if root.join("src/main/module.json5").is_file() {
        module_paths.insert(".".into());
    }

    let declarations = declarations
        .into_iter()
        .map(|d| (d.rel_path.clone(), d))
        .collect::<BTreeMap<_, _>>();
    for rel_path in module_paths {
        if let Some(module) = parse_module(root, &rel_path, declarations.get(&rel_path)) {
            model.modules.push(module);
        }
    }
    model.modules.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));

    assign_product_modules(&mut products, &model.modules);
    model.products = products;
    model.dependencies = parse_dependencies(root, &model.modules);
    model
}

fn parse_app(root: &Path) -> HarmonyApp {
    let candidates = [root.join("AppScope/app.json5"), root.join("app.json5")];
    for path in candidates {
        let Some(value) = read_json5(&path) else {
            continue;
        };
        let Some(app) = value.get("app") else {
            continue;
        };
        return HarmonyApp {
            bundle_name: string(app, "bundleName"),
            version_code: app.get("versionCode").and_then(|v| v.as_i64()),
            version_name: string(app, "versionName"),
            label: string(app, "label"),
        };
    }
    HarmonyApp::default()
}

fn parse_root_profile(root: &Path) -> (Vec<HarmonyProduct>, Vec<RootModuleDecl>, Vec<String>) {
    let Some(value) = read_json5(&root.join("build-profile.json5")) else {
        return (Vec::new(), Vec::new(), Vec::new());
    };
    let products = value
        .pointer("/app/products")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .enumerate()
        .map(|(index, product)| HarmonyProduct {
            name: string(product, "name").unwrap_or_else(|| {
                if index == 0 {
                    "default".into()
                } else {
                    format!("product-{}", index + 1)
                }
            }),
            compile_sdk_version: scalar_string(product.get("compileSdkVersion")),
            compatible_sdk_version: scalar_string(product.get("compatibleSdkVersion")),
            target_sdk_version: scalar_string(product.get("targetSdkVersion")),
            signing_config: string(product, "signingConfig"),
            modules: Vec::new(),
        })
        .collect();
    let modules = value
        .get("modules")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter_map(|module| {
            let name = string(module, "name")?;
            let rel_path = normalize_rel(
                string(module, "srcPath")
                    .unwrap_or_else(|| name.clone())
                    .trim_start_matches("./"),
            );
            Some(RootModuleDecl {
                name,
                rel_path,
                targets: parse_targets(module.get("targets")),
            })
        })
        .collect();
    let signing_configs = value
        .pointer("/app/signingConfigs")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter_map(|config| string(config, "name"))
        .collect();
    (products, modules, signing_configs)
}

fn discover_module_paths(root: &Path, dir: &Path, depth: usize, out: &mut BTreeSet<String>) {
    if depth > 8 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || SKIP_DIRS.contains(&name.as_ref()) {
            continue;
        }
        if path.join("src/main/module.json5").is_file() {
            if let Ok(rel) = path.strip_prefix(root) {
                out.insert(normalize_rel(&rel.to_string_lossy()));
            }
        }
        discover_module_paths(root, &path, depth + 1, out);
    }
}

fn parse_module(
    root: &Path,
    rel_path: &str,
    declaration: Option<&RootModuleDecl>,
) -> Option<HarmonyModule> {
    let module_root = if rel_path == "." {
        root.to_path_buf()
    } else {
        root.join(rel_path)
    };
    let value = read_json5(&module_root.join("src/main/module.json5"))?;
    let module = value.get("module")?;
    let kind = string(module, "type").unwrap_or_default();
    let name = string(module, "name")
        .or_else(|| declaration.map(|d| d.name.clone()))
        .unwrap_or_else(|| rel_path.rsplit('/').next().unwrap_or(rel_path).to_string());
    let manifest_targets = parse_targets(module.get("targets"));
    let targets = if manifest_targets.is_empty() {
        declaration.map(|d| d.targets.clone()).unwrap_or_default()
    } else {
        manifest_targets
    };
    Some(HarmonyModule {
        name,
        rel_path: rel_path.to_string(),
        src_path: if rel_path == "." {
            ".".into()
        } else {
            rel_path.into()
        },
        artifact_kind: artifact_kind(&kind).into(),
        kind,
        package_name: read_json5(&module_root.join("oh-package.json5"))
            .as_ref()
            .and_then(|v| string(v, "name")),
        device_types: string_array(module.get("deviceTypes")),
        main_element: string(module, "mainElement"),
        targets,
        abilities: parse_abilities(module.get("abilities")),
        extension_abilities: parse_extension_abilities(module.get("extensionAbilities")),
    })
}

fn parse_targets(value: Option<&serde_json::Value>) -> Vec<HarmonyTarget> {
    value
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter_map(|target| {
            Some(HarmonyTarget {
                name: string(target, "name")?,
                products: string_array(target.get("applyToProducts")),
            })
        })
        .collect()
}

fn parse_abilities(value: Option<&serde_json::Value>) -> Vec<HarmonyAbility> {
    value
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter_map(|ability| {
            Some(HarmonyAbility {
                name: string(ability, "name")?,
                src_entry: string(ability, "srcEntry"),
                exported: ability.get("exported").and_then(|v| v.as_bool()),
            })
        })
        .collect()
}

fn parse_extension_abilities(value: Option<&serde_json::Value>) -> Vec<HarmonyExtensionAbility> {
    value
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter_map(|ability| {
            Some(HarmonyExtensionAbility {
                name: string(ability, "name")?,
                extension_type: string(ability, "type"),
                src_entry: string(ability, "srcEntry"),
                exported: ability.get("exported").and_then(|v| v.as_bool()),
            })
        })
        .collect()
}

fn assign_product_modules(products: &mut [HarmonyProduct], modules: &[HarmonyModule]) {
    for product in products {
        for module in modules {
            let explicit = module
                .targets
                .iter()
                .flat_map(|target| &target.products)
                .collect::<Vec<_>>();
            if explicit.is_empty() || explicit.iter().any(|name| *name == &product.name) {
                product.modules.push(module.rel_path.clone());
            }
        }
    }
}

fn parse_dependencies(root: &Path, modules: &[HarmonyModule]) -> Vec<HarmonyDependency> {
    let mut sources = vec![(".".to_string(), root.to_path_buf())];
    sources.extend(modules.iter().map(|module| {
        let dir = if module.rel_path == "." {
            root.to_path_buf()
        } else {
            root.join(&module.rel_path)
        };
        (module.rel_path.clone(), dir)
    }));
    sources.sort_by(|a, b| a.0.cmp(&b.0));
    sources.dedup_by(|a, b| a.0 == b.0);

    let packages = modules
        .iter()
        .filter_map(|m| {
            m.package_name
                .as_ref()
                .map(|name| (name.clone(), m.rel_path.clone()))
        })
        .collect::<BTreeMap<_, _>>();
    let paths = modules
        .iter()
        .map(|m| (m.rel_path.clone(), m.rel_path.clone()))
        .collect::<BTreeMap<_, _>>();
    let mut out = Vec::new();
    for (from_module, source_dir) in sources {
        let Some(value) = read_json5(&source_dir.join("oh-package.json5")) else {
            continue;
        };
        for scope in ["dependencies", "devDependencies", "dynamicDependencies"] {
            let Some(deps) = value.get(scope).and_then(|v| v.as_object()) else {
                continue;
            };
            for (name, requirement) in deps {
                let requirement = scalar_string(Some(requirement)).unwrap_or_default();
                let target_module = packages.get(name).cloned().or_else(|| {
                    local_dependency_path(root, &source_dir, &requirement)
                        .and_then(|rel| paths.get(&rel).cloned())
                });
                out.push(HarmonyDependency {
                    from_module: from_module.clone(),
                    name: name.clone(),
                    requirement,
                    scope: scope.into(),
                    target_module,
                });
            }
        }
    }
    out.sort_by(|a, b| {
        (&a.from_module, &a.scope, &a.name).cmp(&(&b.from_module, &b.scope, &b.name))
    });
    out
}

fn local_dependency_path(root: &Path, source_dir: &Path, requirement: &str) -> Option<String> {
    let raw = requirement
        .strip_prefix("file:")
        .or_else(|| requirement.strip_prefix("link:"))?;
    let joined = lexical_normalize(&source_dir.join(raw));
    let rel = joined.strip_prefix(root).ok()?;
    Some(normalize_rel(&rel.to_string_lossy()))
}

fn lexical_normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn artifact_kind(kind: &str) -> &'static str {
    match kind.to_ascii_lowercase().as_str() {
        "entry" | "feature" => "hap",
        "shared" | "hsp" => "hsp",
        "har" => "har",
        _ => "unknown",
    }
}

fn read_json5(path: &Path) -> Option<serde_json::Value> {
    let text = std::fs::read_to_string(path).ok()?;
    crate::services::harmony::parse_json5(&text).ok()
}

fn string(value: &serde_json::Value, key: &str) -> Option<String> {
    value.get(key).and_then(|v| v.as_str()).map(String::from)
}

fn scalar_string(value: Option<&serde_json::Value>) -> Option<String> {
    match value? {
        serde_json::Value::String(value) => Some(value.clone()),
        serde_json::Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

fn string_array(value: Option<&serde_json::Value>) -> Vec<String> {
    value
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str().map(String::from))
        .collect()
}

fn normalize_rel(path: &str) -> String {
    let normalized = path.replace('\\', "/").trim_matches('/').to_string();
    if normalized.is_empty() {
        ".".into()
    } else {
        normalized
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("harmony-model-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for module in ["entry", "features/pay", "shared/runtime", "libs/design"] {
            std::fs::create_dir_all(root.join(module).join("src/main")).unwrap();
        }
        std::fs::create_dir_all(root.join("AppScope")).unwrap();
        std::fs::write(
            root.join("AppScope/app.json5"),
            r#"{"app":{"bundleName":"com.example.graph","versionCode":7,"versionName":"2.0","label":"Graph"}}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("build-profile.json5"),
            r#"{
              "app":{"signingConfigs":[{"name":"release"}],"products":[
                {"name":"default","compileSdkVersion":"6.0.0(20)","compatibleSdkVersion":"5.0.0(12)","signingConfig":"release"},
                {"name":"tablet","targetSdkVersion":20}
              ]},
              "modules":[
                {"name":"entry","srcPath":"./entry","targets":[{"name":"default","applyToProducts":["default"]}]},
                {"name":"pay","srcPath":"./features/pay"},
                {"name":"runtime","srcPath":"./shared/runtime"},
                {"name":"design","srcPath":"./libs/design"}
              ]
            }"#,
        )
        .unwrap();
        let manifests = [
            (
                "entry",
                r#"{"module":{"name":"entry","type":"entry","deviceTypes":["phone"],"mainElement":"EntryAbility","abilities":[{"name":"EntryAbility","srcEntry":"./ets/EntryAbility.ets","exported":true}],"extensionAbilities":[{"name":"BackupExt","type":"backup","srcEntry":"./ets/BackupExt.ets"}]}}"#,
                r#"{"name":"@app/entry","dependencies":{"@app/pay":"file:../features/pay"}}"#,
            ),
            (
                "features/pay",
                r#"{"module":{"name":"pay","type":"feature"}}"#,
                r#"{"name":"@app/pay","dependencies":{"@app/runtime":"file:../../shared/runtime"}}"#,
            ),
            (
                "shared/runtime",
                r#"{"module":{"name":"runtime","type":"shared"}}"#,
                r#"{"name":"@app/runtime"}"#,
            ),
            (
                "libs/design",
                r#"{"module":{"name":"design","type":"har"}}"#,
                r#"{"name":"@app/design"}"#,
            ),
        ];
        for (path, manifest, package) in manifests {
            std::fs::write(root.join(path).join("src/main/module.json5"), manifest).unwrap();
            std::fs::write(root.join(path).join("oh-package.json5"), package).unwrap();
        }
        root
    }

    #[test]
    fn parses_products_nested_modules_artifacts_abilities_and_dependency_edges() {
        let root = fixture("full");
        let model = parse(&root);
        assert_eq!(model.app.bundle_name.as_deref(), Some("com.example.graph"));
        assert_eq!(model.products.len(), 2);
        assert_eq!(model.signing_configs, vec!["release"]);
        assert_eq!(model.modules.len(), 4);
        let kinds = model
            .modules
            .iter()
            .map(|m| (m.rel_path.as_str(), m.artifact_kind.as_str()))
            .collect::<BTreeMap<_, _>>();
        assert_eq!(kinds.get("entry"), Some(&"hap"));
        assert_eq!(kinds.get("features/pay"), Some(&"hap"));
        assert_eq!(kinds.get("shared/runtime"), Some(&"hsp"));
        assert_eq!(kinds.get("libs/design"), Some(&"har"));

        let entry = model
            .modules
            .iter()
            .find(|m| m.rel_path == "entry")
            .unwrap();
        assert_eq!(entry.abilities[0].name, "EntryAbility");
        assert_eq!(entry.extension_abilities[0].name, "BackupExt");
        assert!(model.dependencies.iter().any(|d| {
            d.from_module == "entry" && d.target_module.as_deref() == Some("features/pay")
        }));
        assert!(model.dependencies.iter().any(|d| {
            d.from_module == "features/pay" && d.target_module.as_deref() == Some("shared/runtime")
        }));
        let default = model.products.iter().find(|p| p.name == "default").unwrap();
        assert!(default.modules.contains(&"entry".to_string()));
        let tablet = model.products.iter().find(|p| p.name == "tablet").unwrap();
        assert!(!tablet.modules.contains(&"entry".to_string()));
        assert!(tablet.modules.contains(&"features/pay".to_string()));

        let summary = crate::services::harmony::project_summary(&root, &model);
        assert_eq!(summary.entry_module.as_deref(), Some("entry"));
        assert_eq!(summary.main_element.as_deref(), Some("EntryAbility"));
        assert_eq!(summary.api_version, Some(12));
        assert!(summary.signing_configured);
        assert_eq!(
            summary.hap_output_dir.as_deref(),
            Some(root.join("entry/build/default/outputs/default").as_path())
        );
        std::fs::remove_dir_all(root).ok();
    }
}
