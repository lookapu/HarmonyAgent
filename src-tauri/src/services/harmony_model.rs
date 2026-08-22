//! HarmonyOS 工程统一语义模型。
//!
//! 这里是工程、产品、模块、产物类型、Ability 与依赖关系的单一解析真源。
//! 构建/部署所需的精简摘要和 Workspace 能力分析都应从本模型派生。

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
use std::sync::{LazyLock, Mutex};

const SKIP_DIRS: &[&str] = &[
    ".git",
    ".hvigor",
    ".idea",
    ".ohpm",
    "build",
    "node_modules",
    "oh_modules",
];

static MODEL_CACHE: LazyLock<Mutex<BTreeMap<PathBuf, HarmonySemanticModel>>> =
    LazyLock::new(|| Mutex::new(BTreeMap::new()));

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HarmonySemanticModel {
    pub schema_version: u32,
    pub app: HarmonyApp,
    pub signing_configs: Vec<HarmonySigningConfig>,
    pub build_modes: Vec<String>,
    pub products: Vec<HarmonyProduct>,
    pub product_differences: Vec<HarmonyProductDifference>,
    pub modules: Vec<HarmonyModule>,
    pub dependencies: Vec<HarmonyDependency>,
    pub lockfiles: Vec<HarmonyLockfile>,
    pub manifests: Vec<HarmonyManifestSource>,
    pub graph: HarmonyProjectGraph,
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
    pub compile_api_level: Option<i64>,
    pub compatible_api_level: Option<i64>,
    pub target_api_level: Option<i64>,
    pub runtime_os: Option<String>,
    pub signing_config: Option<String>,
    /// 由 module target 的 applyToProducts 反向计算；没有显式约束时模块属于全部产品。
    pub modules: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HarmonySigningConfig {
    pub name: String,
    pub material_configured: bool,
    pub certificate_configured: bool,
    pub profile_configured: bool,
    pub keystore_configured: bool,
    pub key_alias_configured: bool,
    pub sign_alg: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HarmonyProductDifference {
    pub baseline: String,
    pub product: String,
    pub fields: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HarmonyModule {
    pub name: String,
    pub rel_path: String,
    pub src_path: String,
    pub kind: String,
    pub api_type: Option<String>,
    pub build_modes: Vec<String>,
    /// hap / hsp / har / unknown
    pub artifact_kind: String,
    pub package_name: Option<String>,
    pub device_types: Vec<String>,
    pub main_element: Option<String>,
    pub targets: Vec<HarmonyTarget>,
    pub abilities: Vec<HarmonyAbility>,
    pub extension_abilities: Vec<HarmonyExtensionAbility>,
    pub permissions: Vec<HarmonyPermission>,
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
pub struct HarmonyPermission {
    pub name: String,
    pub reason: Option<String>,
    pub abilities: Vec<String>,
    pub when: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HarmonyProjectGraph {
    pub pages: Vec<HarmonyPage>,
    pub system_capabilities: Vec<HarmonySystemCapabilityRef>,
    pub cross_module_refs: Vec<HarmonyCrossModuleRef>,
    pub edges: Vec<HarmonyGraphEdge>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HarmonyPage {
    pub module: String,
    pub path: String,
    /// main_pages / router_map / decorator
    pub source_kind: String,
    pub source_file: String,
    pub route_name: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HarmonySystemCapabilityRef {
    pub module: String,
    pub capability: String,
    pub source_file: String,
    pub line: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HarmonyCrossModuleRef {
    pub from_module: String,
    pub to_module: String,
    pub specifier: String,
    pub source_file: String,
    pub line: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HarmonyGraphEdge {
    pub from: String,
    pub to: String,
    pub kind: String,
    pub source: String,
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
    /// 锁文件中解析到的精确版本；本地依赖或未锁定时为空。
    pub locked_version: Option<String>,
    pub lockfile: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HarmonyLockfile {
    pub path: String,
    pub owner_module: String,
    pub lockfile_version: Option<i64>,
    pub specifiers: Vec<HarmonyLockSpecifier>,
    pub packages: Vec<HarmonyLockedPackage>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HarmonyLockSpecifier {
    pub declared: String,
    pub locked: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HarmonyLockedPackage {
    pub key: String,
    pub name: Option<String>,
    pub version: Option<String>,
    pub resolved: Option<String>,
    pub integrity: Option<String>,
    pub registry_type: Option<String>,
    pub dependencies: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HarmonyManifestSource {
    /// app / root-build-profile / module / oh-package / oh-package-lock
    pub kind: String,
    pub path: String,
    /// `.` 表示工程根清单。
    pub owner_module: String,
    /// parsed / invalid
    pub status: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HarmonyModelUpdate {
    pub mode: String,
    pub changed_files: Vec<String>,
    pub affected_modules: Vec<String>,
    pub verification: HarmonyVerificationScope,
    pub model: HarmonySemanticModel,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HarmonyVerificationScope {
    pub modules: Vec<String>,
    pub products: Vec<String>,
    pub checks: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HarmonyImpactAnalysis {
    pub mode: String,
    pub changed_files: Vec<String>,
    pub direct_modules: Vec<String>,
    pub affected_modules: Vec<String>,
    pub verification: HarmonyVerificationScope,
    pub traces: Vec<HarmonyImpactTrace>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HarmonyImpactTrace {
    pub module: String,
    /// direct / dependency / import / project_structure
    pub kind: String,
    pub source: String,
    pub depends_on: Option<String>,
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
        schema_version: 4,
        ..HarmonySemanticModel::default()
    };
    model.app = parse_app(root);
    let (mut products, declarations, signing_configs, build_modes) = parse_root_profile(root);
    model.signing_configs = signing_configs;
    model.build_modes = build_modes;

    let mut module_paths = declarations
        .iter()
        .map(|d| d.rel_path.clone())
        .collect::<BTreeSet<_>>();
    discover_module_paths(root, root, 0, &mut module_paths);
    if root.join("src/main/module.json5").is_file() {
        module_paths.insert(".".into());
    }
    let manifest_paths = module_paths.iter().cloned().collect::<Vec<_>>();

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
    model.product_differences = product_differences(&products);
    model.products = products;
    model.lockfiles = parse_lockfiles(root, &manifest_paths);
    model.dependencies =
        parse_dependencies(root, &model.modules, &manifest_paths, &model.lockfiles);
    model.manifests = collect_manifest_sources(root, &manifest_paths);
    model.graph = build_project_graph(root, &model);
    model
}

pub fn cached(root: &Path) -> HarmonySemanticModel {
    let key = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let mut cache = MODEL_CACHE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    cache.entry(key).or_insert_with(|| parse(root)).clone()
}

pub fn invalidate_files(root: &Path, changed_files: &[String]) -> HarmonyModelUpdate {
    let key = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let mut cache = MODEL_CACHE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let update = if let Some(previous) = cache.get(&key) {
        refresh_after_changes(root, previous, changed_files)
    } else {
        let model = parse(root);
        let changed_files = changed_files
            .iter()
            .map(|path| normalize_changed_path(root, path))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let affected_modules = model
            .modules
            .iter()
            .map(|module| module.rel_path.clone())
            .collect::<Vec<_>>();
        HarmonyModelUpdate {
            mode: "full".into(),
            verification: verification_scope(&model, &affected_modules, &changed_files),
            changed_files,
            affected_modules,
            model,
        }
    };
    cache.insert(key, update.model.clone());
    update
}

/// 预览一组文件变化的传播范围，不读取或修改文件，也不改变模型缓存。
pub fn analyze_impact(
    root: &Path,
    model: &HarmonySemanticModel,
    changed_files: &[String],
) -> HarmonyImpactAnalysis {
    let changed_files = changed_files
        .iter()
        .map(|path| normalize_changed_path(root, path))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let owners = changed_files
        .iter()
        .map(|path| owning_module(path, &model.modules))
        .collect::<Vec<_>>();
    let structural = changed_files.iter().any(|path| {
        matches!(
            path.as_str(),
            "build-profile.json5" | "AppScope/app.json5" | "app.json5"
        ) || (path.ends_with("module.json5")
            || path.ends_with("build-profile.json5")
            || path.ends_with("oh-package.json5"))
            && owning_module(path, &model.modules).is_none()
    });
    if structural {
        let affected_modules = model
            .modules
            .iter()
            .map(|module| module.rel_path.clone())
            .collect::<Vec<_>>();
        let source = changed_files.join(", ");
        return HarmonyImpactAnalysis {
            mode: "full".into(),
            direct_modules: Vec::new(),
            traces: affected_modules
                .iter()
                .map(|module| HarmonyImpactTrace {
                    module: module.clone(),
                    kind: "project_structure".into(),
                    source: source.clone(),
                    depends_on: None,
                })
                .collect(),
            verification: verification_scope(model, &affected_modules, &changed_files),
            changed_files,
            affected_modules,
        };
    }

    let direct_modules = owners.into_iter().flatten().collect::<BTreeSet<_>>();
    let mut traces = BTreeMap::<String, HarmonyImpactTrace>::new();
    for (file, module) in changed_files
        .iter()
        .filter_map(|file| owning_module(file, &model.modules).map(|module| (file, module)))
    {
        traces.entry(module.clone()).or_insert(HarmonyImpactTrace {
            module,
            kind: "direct".into(),
            source: file.clone(),
            depends_on: None,
        });
    }
    let mut affected = direct_modules.clone();
    loop {
        let before = affected.len();
        for dependency in &model.dependencies {
            let Some(target) = &dependency.target_module else {
                continue;
            };
            if affected.contains(target) && !affected.contains(&dependency.from_module) {
                let module = dependency.from_module.clone();
                affected.insert(module.clone());
                traces.insert(
                    module.clone(),
                    HarmonyImpactTrace {
                        module,
                        kind: "dependency".into(),
                        source: if dependency.from_module == "." {
                            "oh-package.json5".into()
                        } else {
                            format!("{}/oh-package.json5", dependency.from_module)
                        },
                        depends_on: Some(target.clone()),
                    },
                );
            }
        }
        for reference in &model.graph.cross_module_refs {
            if affected.contains(&reference.to_module) && !affected.contains(&reference.from_module)
            {
                let module = reference.from_module.clone();
                affected.insert(module.clone());
                traces.insert(
                    module.clone(),
                    HarmonyImpactTrace {
                        module,
                        kind: "import".into(),
                        source: format!("{}:{}", reference.source_file, reference.line),
                        depends_on: Some(reference.to_module.clone()),
                    },
                );
            }
        }
        if affected.len() == before {
            break;
        }
    }
    let affected_modules = affected.into_iter().collect::<Vec<_>>();
    let verification = verification_scope(model, &affected_modules, &changed_files);
    HarmonyImpactAnalysis {
        mode: "incremental".into(),
        changed_files,
        direct_modules: direct_modules.into_iter().collect(),
        verification,
        affected_modules,
        traces: traces.into_values().collect(),
    }
}

/// 按文件变化增量刷新模型，并沿声明依赖与真实 import 反向计算验证范围。
pub fn refresh_after_changes(
    root: &Path,
    previous: &HarmonySemanticModel,
    changed_files: &[String],
) -> HarmonyModelUpdate {
    let mut changed_files = changed_files
        .iter()
        .map(|path| normalize_changed_path(root, path))
        .collect::<Vec<_>>();
    changed_files.sort();
    changed_files.dedup();
    let root_structural = changed_files.iter().any(|path| {
        matches!(
            path.as_str(),
            "build-profile.json5" | "AppScope/app.json5" | "app.json5"
        )
    });
    let owners = changed_files
        .iter()
        .map(|path| owning_module(path, &previous.modules))
        .collect::<Vec<_>>();
    let unknown_structural = changed_files.iter().zip(&owners).any(|(path, owner)| {
        owner.is_none()
            && (path.ends_with("module.json5")
                || path.ends_with("build-profile.json5")
                || path.ends_with("oh-package.json5"))
    });
    if root_structural || unknown_structural {
        let model = parse(root);
        let affected_modules = model
            .modules
            .iter()
            .map(|module| module.rel_path.clone())
            .collect::<Vec<_>>();
        return HarmonyModelUpdate {
            mode: "full".into(),
            verification: verification_scope(&model, &affected_modules, &changed_files),
            changed_files,
            affected_modules,
            model,
        };
    }

    let mut model = previous.clone();
    let directly_changed = owners.into_iter().flatten().collect::<BTreeSet<_>>();
    for rel_path in &directly_changed {
        let previous_module = previous
            .modules
            .iter()
            .find(|module| &module.rel_path == rel_path);
        let declaration = previous_module.map(|module| RootModuleDecl {
            name: module.name.clone(),
            rel_path: module.rel_path.clone(),
            targets: module.targets.clone(),
        });
        let reparsed = parse_module(root, rel_path, declaration.as_ref());
        model.modules.retain(|module| &module.rel_path != rel_path);
        if let Some(module) = reparsed {
            model.modules.push(module);
        }
    }
    model.modules.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    for product in &mut model.products {
        product.modules.clear();
    }
    assign_product_modules(&mut model.products, &model.modules);
    model.product_differences = product_differences(&model.products);

    let mut manifest_paths = model
        .modules
        .iter()
        .map(|module| module.rel_path.clone())
        .chain(
            previous
                .manifests
                .iter()
                .filter(|source| source.owner_module != ".")
                .map(|source| source.owner_module.clone()),
        )
        .collect::<Vec<_>>();
    manifest_paths.sort();
    manifest_paths.dedup();
    model.lockfiles = parse_lockfiles(root, &manifest_paths);
    model.dependencies =
        parse_dependencies(root, &model.modules, &manifest_paths, &model.lockfiles);
    model.manifests = collect_manifest_sources(root, &manifest_paths);
    model.graph = build_project_graph(root, &model);

    let affected_modules = affected_module_closure(previous, &directly_changed);
    let verification = verification_scope(&model, &affected_modules, &changed_files);
    HarmonyModelUpdate {
        mode: "incremental".into(),
        changed_files,
        affected_modules,
        verification,
        model,
    }
}

fn owning_module(path: &str, modules: &[HarmonyModule]) -> Option<String> {
    modules
        .iter()
        .filter(|module| {
            module.rel_path == "."
                || path == module.rel_path
                || path
                    .strip_prefix(&module.rel_path)
                    .is_some_and(|tail| tail.starts_with('/'))
        })
        .max_by_key(|module| module.rel_path.matches('/').count())
        .map(|module| module.rel_path.clone())
}

fn affected_module_closure(
    model: &HarmonySemanticModel,
    directly_changed: &BTreeSet<String>,
) -> Vec<String> {
    let mut affected = directly_changed.clone();
    loop {
        let before = affected.len();
        for dependency in &model.dependencies {
            if dependency
                .target_module
                .as_ref()
                .is_some_and(|target| affected.contains(target))
            {
                affected.insert(dependency.from_module.clone());
            }
        }
        for reference in &model.graph.cross_module_refs {
            if affected.contains(&reference.to_module) {
                affected.insert(reference.from_module.clone());
            }
        }
        if affected.len() == before {
            break;
        }
    }
    affected.into_iter().collect()
}

fn verification_scope(
    model: &HarmonySemanticModel,
    modules: &[String],
    changed_files: &[String],
) -> HarmonyVerificationScope {
    let module_set = modules.iter().collect::<BTreeSet<_>>();
    let products = model
        .products
        .iter()
        .filter(|product| {
            product
                .modules
                .iter()
                .any(|module| module_set.contains(module))
        })
        .map(|product| product.name.clone())
        .collect::<Vec<_>>();
    let mut checks = BTreeSet::new();
    checks.insert("build".to_string());
    if changed_files
        .iter()
        .any(|path| path.ends_with(".ets") || path.ends_with(".ts"))
    {
        checks.insert("lint".into());
        checks.insert("test".into());
    }
    if changed_files
        .iter()
        .any(|path| path.contains("oh-package") || path.ends_with("build-profile.json5"))
    {
        checks.insert("dependency_sync".into());
    }
    if changed_files
        .iter()
        .any(|path| path.ends_with("module.json5") || path.ends_with("build-profile.json5"))
    {
        checks.insert("configuration".into());
    }
    HarmonyVerificationScope {
        modules: modules.to_vec(),
        products,
        checks: checks.into_iter().collect(),
    }
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

fn parse_root_profile(
    root: &Path,
) -> (
    Vec<HarmonyProduct>,
    Vec<RootModuleDecl>,
    Vec<HarmonySigningConfig>,
    Vec<String>,
) {
    let Some(value) = read_json5(&root.join("build-profile.json5")) else {
        return (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    };
    let products = value
        .pointer("/app/products")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .enumerate()
        .map(|(index, product)| {
            let compile_sdk_version = scalar_string(product.get("compileSdkVersion"));
            let compatible_sdk_version = scalar_string(product.get("compatibleSdkVersion"));
            let target_sdk_version = scalar_string(product.get("targetSdkVersion"));
            HarmonyProduct {
                name: string(product, "name").unwrap_or_else(|| {
                    if index == 0 {
                        "default".into()
                    } else {
                        format!("product-{}", index + 1)
                    }
                }),
                compile_api_level: compile_sdk_version.as_deref().and_then(parse_api_level),
                compatible_api_level: compatible_sdk_version.as_deref().and_then(parse_api_level),
                target_api_level: target_sdk_version.as_deref().and_then(parse_api_level),
                compile_sdk_version,
                compatible_sdk_version,
                target_sdk_version,
                runtime_os: string(product, "runtimeOS"),
                signing_config: string(product, "signingConfig"),
                modules: Vec::new(),
            }
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
        .filter_map(parse_signing_config)
        .collect();
    let build_modes = value
        .pointer("/app/buildModeSet")
        .and_then(|value| value.as_array())
        .into_iter()
        .flatten()
        .filter_map(|mode| string(mode, "name"))
        .collect();
    (products, modules, signing_configs, build_modes)
}

fn parse_signing_config(value: &serde_json::Value) -> Option<HarmonySigningConfig> {
    let material = value.get("material");
    Some(HarmonySigningConfig {
        name: string(value, "name")?,
        material_configured: material
            .and_then(|value| value.as_object())
            .is_some_and(|value| !value.is_empty()),
        certificate_configured: material.is_some_and(|value| value.get("certpath").is_some()),
        profile_configured: material.is_some_and(|value| value.get("profile").is_some()),
        keystore_configured: material.is_some_and(|value| value.get("storeFile").is_some()),
        key_alias_configured: material.is_some_and(|value| value.get("keyAlias").is_some()),
        sign_alg: material.and_then(|value| string(value, "signAlg")),
    })
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
    let module_profile = read_json5(&module_root.join("build-profile.json5"));
    let name = string(module, "name")
        .or_else(|| declaration.map(|d| d.name.clone()))
        .unwrap_or_else(|| rel_path.rsplit('/').next().unwrap_or(rel_path).to_string());
    let profile_targets = module_profile
        .as_ref()
        .map(|profile| parse_targets(profile.get("targets")))
        .unwrap_or_default();
    let targets = if profile_targets.is_empty() {
        declaration.map(|d| d.targets.clone()).unwrap_or_default()
    } else {
        profile_targets
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
        api_type: module_profile
            .as_ref()
            .and_then(|profile| string(profile, "apiType")),
        build_modes: module_profile
            .as_ref()
            .and_then(|profile| profile.get("buildOptionSet"))
            .and_then(|value| value.as_array())
            .into_iter()
            .flatten()
            .filter_map(|mode| string(mode, "name"))
            .collect(),
        package_name: read_json5(&module_root.join("oh-package.json5"))
            .as_ref()
            .and_then(|v| string(v, "name")),
        device_types: string_array(module.get("deviceTypes")),
        main_element: string(module, "mainElement"),
        targets,
        abilities: parse_abilities(module.get("abilities")),
        extension_abilities: parse_extension_abilities(module.get("extensionAbilities")),
        permissions: parse_permissions(module.get("requestPermissions")),
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

fn parse_permissions(value: Option<&serde_json::Value>) -> Vec<HarmonyPermission> {
    value
        .and_then(|value| value.as_array())
        .into_iter()
        .flatten()
        .filter_map(|permission| {
            let used_scene = permission.get("usedScene");
            Some(HarmonyPermission {
                name: string(permission, "name")?,
                reason: string(permission, "reason"),
                abilities: used_scene
                    .and_then(|scene| scene.get("abilities"))
                    .map(|value| {
                        value
                            .as_str()
                            .map(|single| vec![single.to_string()])
                            .unwrap_or_else(|| string_array(Some(value)))
                    })
                    .unwrap_or_default(),
                when: used_scene.and_then(|scene| string(scene, "when")),
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
            if explicit.is_empty() || explicit.contains(&&product.name) {
                product.modules.push(module.rel_path.clone());
            }
        }
    }
}

fn product_differences(products: &[HarmonyProduct]) -> Vec<HarmonyProductDifference> {
    let Some(baseline) = products
        .iter()
        .find(|product| product.name == "default")
        .or_else(|| products.first())
    else {
        return Vec::new();
    };
    products
        .iter()
        .filter(|product| product.name != baseline.name)
        .filter_map(|product| {
            let mut fields = Vec::new();
            if product.compile_sdk_version != baseline.compile_sdk_version {
                fields.push("compileSdkVersion".into());
            }
            if product.compatible_sdk_version != baseline.compatible_sdk_version {
                fields.push("compatibleSdkVersion".into());
            }
            if product.target_sdk_version != baseline.target_sdk_version {
                fields.push("targetSdkVersion".into());
            }
            if product.runtime_os != baseline.runtime_os {
                fields.push("runtimeOS".into());
            }
            if product.signing_config != baseline.signing_config {
                fields.push("signingConfig".into());
            }
            if product.modules != baseline.modules {
                fields.push("modules".into());
            }
            (!fields.is_empty()).then(|| HarmonyProductDifference {
                baseline: baseline.name.clone(),
                product: product.name.clone(),
                fields,
            })
        })
        .collect()
}

fn manifest_roots(root: &Path, module_paths: &[String]) -> Vec<(String, PathBuf)> {
    let mut sources = vec![(".".to_string(), root.to_path_buf())];
    sources.extend(module_paths.iter().map(|rel_path| {
        let dir = if rel_path == "." {
            root.to_path_buf()
        } else {
            root.join(rel_path)
        };
        (rel_path.clone(), dir)
    }));
    sources.sort_by(|a, b| a.0.cmp(&b.0));
    sources.dedup_by(|a, b| a.0 == b.0);
    sources
}

fn parse_lockfiles(root: &Path, module_paths: &[String]) -> Vec<HarmonyLockfile> {
    let mut out = Vec::new();
    for (owner_module, dir) in manifest_roots(root, module_paths) {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut paths = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.is_file()
                    && path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(is_ohpm_lockfile)
            })
            .collect::<Vec<_>>();
        paths.sort();
        for path in paths {
            let Some(value) = read_json5(&path) else {
                continue;
            };
            let specifiers = value
                .get("specifiers")
                .and_then(|value| value.as_object())
                .into_iter()
                .flatten()
                .map(|(declared, locked)| HarmonyLockSpecifier {
                    declared: declared.clone(),
                    locked: scalar_string(Some(locked)).unwrap_or_default(),
                })
                .collect();
            let packages = value
                .get("packages")
                .and_then(|value| value.as_object())
                .into_iter()
                .flatten()
                .map(|(key, package)| {
                    let (derived_name, derived_version) = split_package_key(key);
                    HarmonyLockedPackage {
                        key: key.clone(),
                        name: string(package, "name").or(derived_name),
                        version: string(package, "version").or(derived_version),
                        resolved: string(package, "resolved"),
                        integrity: string(package, "integrity"),
                        registry_type: string(package, "registryType"),
                        dependencies: package
                            .get("dependencies")
                            .and_then(|value| value.as_object())
                            .into_iter()
                            .flatten()
                            .filter_map(|(name, version)| {
                                scalar_string(Some(version)).map(|version| (name.clone(), version))
                            })
                            .collect(),
                    }
                })
                .collect();
            out.push(HarmonyLockfile {
                path: path
                    .strip_prefix(root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .replace('\\', "/"),
                owner_module: owner_module.clone(),
                lockfile_version: value
                    .get("lockfileVersion")
                    .and_then(|value| value.as_i64()),
                specifiers,
                packages,
            });
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

fn is_ohpm_lockfile(name: &str) -> bool {
    (name == "oh-package-lock.json5" || name == "oh-package-lock.json")
        || name.starts_with("oh-package-") && name.ends_with("-lock.json5")
}

fn split_package_key(key: &str) -> (Option<String>, Option<String>) {
    let Some((name, version)) = key.rsplit_once('@') else {
        return (None, None);
    };
    if name.is_empty() || version.is_empty() {
        (None, None)
    } else {
        (Some(name.to_string()), Some(version.to_string()))
    }
}

fn parse_dependencies(
    root: &Path,
    modules: &[HarmonyModule],
    module_paths: &[String],
    lockfiles: &[HarmonyLockfile],
) -> Vec<HarmonyDependency> {
    let sources = manifest_roots(root, module_paths);

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
                let (locked_version, lockfile) =
                    resolve_locked_dependency(&from_module, name, &requirement, lockfiles);
                out.push(HarmonyDependency {
                    from_module: from_module.clone(),
                    name: name.clone(),
                    requirement,
                    scope: scope.into(),
                    target_module,
                    locked_version,
                    lockfile,
                });
            }
        }
    }
    out.sort_by(|a, b| {
        (&a.from_module, &a.scope, &a.name).cmp(&(&b.from_module, &b.scope, &b.name))
    });
    out
}

fn resolve_locked_dependency(
    owner_module: &str,
    name: &str,
    requirement: &str,
    lockfiles: &[HarmonyLockfile],
) -> (Option<String>, Option<String>) {
    let declared = format!("{name}@{requirement}");
    for lockfile in lockfiles
        .iter()
        .filter(|lockfile| lockfile.owner_module == owner_module)
        .chain(
            lockfiles
                .iter()
                .filter(|lockfile| lockfile.owner_module == "."),
        )
    {
        let Some(specifier) = lockfile
            .specifiers
            .iter()
            .find(|specifier| specifier.declared == declared)
        else {
            continue;
        };
        let version = lockfile
            .packages
            .iter()
            .find(|package| package.key == specifier.locked)
            .and_then(|package| package.version.clone())
            .or_else(|| split_package_key(&specifier.locked).1);
        return (version, Some(lockfile.path.clone()));
    }
    (None, None)
}

fn collect_manifest_sources(root: &Path, module_paths: &[String]) -> Vec<HarmonyManifestSource> {
    let mut candidates = vec![
        (
            "app".to_string(),
            ".".to_string(),
            root.join("AppScope/app.json5"),
        ),
        ("app".to_string(), ".".to_string(), root.join("app.json5")),
        (
            "root-build-profile".to_string(),
            ".".to_string(),
            root.join("build-profile.json5"),
        ),
    ];
    for (owner, dir) in manifest_roots(root, module_paths) {
        candidates.push((
            "oh-package".into(),
            owner.clone(),
            dir.join("oh-package.json5"),
        ));
        candidates.push((
            "module".into(),
            owner.clone(),
            dir.join("src/main/module.json5"),
        ));
        if owner != "." {
            candidates.push((
                "module-build-profile".into(),
                owner,
                dir.join("build-profile.json5"),
            ));
        }
    }
    for (owner, dir) in manifest_roots(root, module_paths) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for path in entries.flatten().map(|entry| entry.path()).filter(|path| {
            path.is_file()
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(is_ohpm_lockfile)
        }) {
            candidates.push(("oh-package-lock".into(), owner.clone(), path));
        }
    }

    let mut out = candidates
        .into_iter()
        .filter(|(_, _, path)| path.is_file())
        .map(|(kind, owner_module, path)| manifest_source(root, &kind, &owner_module, &path))
        .collect::<Vec<_>>();
    out.sort_by(|a, b| a.path.cmp(&b.path).then_with(|| a.kind.cmp(&b.kind)));
    out.dedup_by(|a, b| a.path == b.path && a.kind == b.kind);
    out
}

fn manifest_source(
    root: &Path,
    kind: &str,
    owner_module: &str,
    path: &Path,
) -> HarmonyManifestSource {
    let parsed = std::fs::read_to_string(path)
        .map_err(|error| error.to_string())
        .and_then(|text| crate::services::harmony::parse_json5(&text).map(|_| ()));
    HarmonyManifestSource {
        kind: kind.into(),
        path: path
            .strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/"),
        owner_module: owner_module.into(),
        status: if parsed.is_ok() { "parsed" } else { "invalid" }.into(),
        error: parsed.err(),
    }
}

fn build_project_graph(root: &Path, model: &HarmonySemanticModel) -> HarmonyProjectGraph {
    let mut graph = HarmonyProjectGraph::default();
    let packages = model
        .modules
        .iter()
        .filter_map(|module| {
            module
                .package_name
                .as_ref()
                .map(|name| (name.clone(), module.rel_path.clone()))
        })
        .collect::<BTreeMap<_, _>>();
    let module_paths = model
        .modules
        .iter()
        .map(|module| module.rel_path.clone())
        .collect::<Vec<_>>();

    for product in &model.products {
        for module in &product.modules {
            graph.edges.push(HarmonyGraphEdge {
                from: format!("product:{}", product.name),
                to: format!("module:{module}"),
                kind: "includes".into(),
                source: "build-profile.json5".into(),
            });
        }
    }
    for module in &model.modules {
        let module_id = format!("module:{}", module.rel_path);
        for ability in &module.abilities {
            graph.edges.push(HarmonyGraphEdge {
                from: module_id.clone(),
                to: format!("ability:{}:{}", module.rel_path, ability.name),
                kind: "contains".into(),
                source: format!("{}/src/main/module.json5", module.rel_path),
            });
        }
        for ability in &module.extension_abilities {
            graph.edges.push(HarmonyGraphEdge {
                from: module_id.clone(),
                to: format!("extension:{}:{}", module.rel_path, ability.name),
                kind: "contains".into(),
                source: format!("{}/src/main/module.json5", module.rel_path),
            });
        }
        for permission in &module.permissions {
            graph.edges.push(HarmonyGraphEdge {
                from: module_id.clone(),
                to: format!("permission:{}", permission.name),
                kind: "requests".into(),
                source: format!("{}/src/main/module.json5", module.rel_path),
            });
        }
        collect_profile_pages(root, module, &mut graph.pages);
        scan_module_sources(root, module, &packages, &module_paths, &mut graph);
    }
    for dependency in &model.dependencies {
        if let Some(target) = &dependency.target_module {
            graph.edges.push(HarmonyGraphEdge {
                from: format!("module:{}", dependency.from_module),
                to: format!("module:{target}"),
                kind: "depends_on".into(),
                source: dependency
                    .lockfile
                    .clone()
                    .unwrap_or_else(|| format!("{}/oh-package.json5", dependency.from_module)),
            });
        }
    }
    for page in &graph.pages {
        graph.edges.push(HarmonyGraphEdge {
            from: format!("module:{}", page.module),
            to: format!("page:{}:{}", page.module, page.path),
            kind: "contains".into(),
            source: page.source_file.clone(),
        });
    }
    for capability in &graph.system_capabilities {
        graph.edges.push(HarmonyGraphEdge {
            from: format!("module:{}", capability.module),
            to: format!("syscap:{}", capability.capability),
            kind: "checks".into(),
            source: format!("{}:{}", capability.source_file, capability.line),
        });
    }
    for reference in &graph.cross_module_refs {
        graph.edges.push(HarmonyGraphEdge {
            from: format!("module:{}", reference.from_module),
            to: format!("module:{}", reference.to_module),
            kind: "imports".into(),
            source: format!("{}:{}", reference.source_file, reference.line),
        });
    }
    graph.pages.sort_by(|a, b| {
        (&a.module, &a.path, &a.source_kind).cmp(&(&b.module, &b.path, &b.source_kind))
    });
    graph.pages.dedup_by(|a, b| {
        a.module == b.module && a.path == b.path && a.source_kind == b.source_kind
    });
    graph.system_capabilities.sort_by(|a, b| {
        (&a.module, &a.capability, &a.source_file, a.line).cmp(&(
            &b.module,
            &b.capability,
            &b.source_file,
            b.line,
        ))
    });
    graph.system_capabilities.dedup_by(|a, b| {
        a.module == b.module
            && a.capability == b.capability
            && a.source_file == b.source_file
            && a.line == b.line
    });
    graph.cross_module_refs.sort_by(|a, b| {
        (&a.from_module, &a.to_module, &a.source_file, a.line).cmp(&(
            &b.from_module,
            &b.to_module,
            &b.source_file,
            b.line,
        ))
    });
    graph.cross_module_refs.dedup_by(|a, b| {
        a.from_module == b.from_module
            && a.to_module == b.to_module
            && a.source_file == b.source_file
            && a.line == b.line
    });
    graph.edges.sort_by(|a, b| {
        (&a.from, &a.to, &a.kind, &a.source).cmp(&(&b.from, &b.to, &b.kind, &b.source))
    });
    graph.edges.dedup_by(|a, b| {
        a.from == b.from && a.to == b.to && a.kind == b.kind && a.source == b.source
    });
    graph
}

fn collect_profile_pages(root: &Path, module: &HarmonyModule, out: &mut Vec<HarmonyPage>) {
    let module_root = module_root(root, &module.rel_path);
    let profile = module_root.join("src/main/resources/base/profile");
    let main_pages = profile.join("main_pages.json");
    if let Some(value) = read_json5(&main_pages) {
        if let Some(pages) = value.get("src").and_then(|value| value.as_array()) {
            for page in pages.iter().filter_map(|value| value.as_str()) {
                out.push(HarmonyPage {
                    module: module.rel_path.clone(),
                    path: normalize_source_path(page),
                    source_kind: "main_pages".into(),
                    source_file: relative_file(root, &main_pages),
                    route_name: None,
                });
            }
        }
    }
    let Ok(entries) = std::fs::read_dir(&profile) else {
        return;
    };
    for path in entries.flatten().map(|entry| entry.path()).filter(|path| {
        path.extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension == "json" || extension == "json5")
    }) {
        let Some(value) = read_json5(&path) else {
            continue;
        };
        let Some(routes) = value.get("routerMap").and_then(|value| value.as_array()) else {
            continue;
        };
        for route in routes {
            let Some(page) = string(route, "pageSourceFile") else {
                continue;
            };
            out.push(HarmonyPage {
                module: module.rel_path.clone(),
                path: normalize_source_path(&page),
                source_kind: "router_map".into(),
                source_file: relative_file(root, &path),
                route_name: string(route, "name"),
            });
        }
    }
}

fn scan_module_sources(
    root: &Path,
    module: &HarmonyModule,
    packages: &BTreeMap<String, String>,
    module_paths: &[String],
    graph: &mut HarmonyProjectGraph,
) {
    let module_root = module_root(root, &module.rel_path);
    let source_root = module_root.join("src/main");
    let mut files = Vec::new();
    collect_source_files(&source_root, 0, &mut files);
    files.sort();
    files.truncate(2_000);
    for path in files {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let source_file = relative_file(root, &path);
        if text.contains("@Entry") || text.contains("@Router") {
            let relative = path
                .strip_prefix(module_root.join("src/main/ets"))
                .or_else(|_| path.strip_prefix(&source_root))
                .unwrap_or(&path)
                .with_extension("")
                .to_string_lossy()
                .replace('\\', "/");
            graph.pages.push(HarmonyPage {
                module: module.rel_path.clone(),
                path: relative,
                source_kind: "decorator".into(),
                source_file: source_file.clone(),
                route_name: None,
            });
        }
        for (index, line) in text.lines().enumerate() {
            for capability in extract_system_capabilities(line) {
                graph.system_capabilities.push(HarmonySystemCapabilityRef {
                    module: module.rel_path.clone(),
                    capability,
                    source_file: source_file.clone(),
                    line: index + 1,
                });
            }
            let Some(specifier) = extract_import_specifier(line) else {
                continue;
            };
            if let Some(target) =
                resolve_import_target(root, &path, &specifier, packages, module_paths)
            {
                if target != module.rel_path {
                    graph.cross_module_refs.push(HarmonyCrossModuleRef {
                        from_module: module.rel_path.clone(),
                        to_module: target,
                        specifier,
                        source_file: source_file.clone(),
                        line: index + 1,
                    });
                }
            }
        }
    }
}

fn collect_source_files(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > 12 || out.len() >= 2_000 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') || SKIP_DIRS.contains(&name.as_ref()) {
                continue;
            }
            collect_source_files(&path, depth + 1, out);
        } else if path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension == "ets" || extension == "ts")
        {
            out.push(path);
        }
    }
}

fn extract_system_capabilities(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut remaining = line;
    while let Some(index) = remaining.find("SystemCapability.") {
        let candidate = &remaining[index..];
        let end = candidate
            .find(|character: char| {
                !(character.is_ascii_alphanumeric() || character == '.' || character == '_')
            })
            .unwrap_or(candidate.len());
        let capability = &candidate[..end];
        if capability.len() > "SystemCapability.".len() {
            out.push(capability.to_string());
        }
        remaining = &candidate[end..];
    }
    out
}

fn extract_import_specifier(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if !trimmed.starts_with("import ") && !trimmed.starts_with("export ") {
        return None;
    }
    let tail = trimmed
        .find(" from ")
        .map(|index| &trimmed[index + 6..])
        .unwrap_or(trimmed.trim_start_matches("import "));
    let quote_index = tail.find(['\'', '"'])?;
    let quote = tail.as_bytes()[quote_index] as char;
    let value = &tail[quote_index + 1..];
    let end = value.find(quote)?;
    Some(value[..end].to_string())
}

fn resolve_import_target(
    root: &Path,
    source_file: &Path,
    specifier: &str,
    packages: &BTreeMap<String, String>,
    module_paths: &[String],
) -> Option<String> {
    if specifier.starts_with('.') {
        let target = lexical_normalize(&source_file.parent()?.join(specifier));
        return module_path_for_file(root, &target, module_paths.iter());
    }
    packages
        .iter()
        .filter(|(package, _)| {
            specifier == package.as_str()
                || specifier
                    .strip_prefix(package.as_str())
                    .is_some_and(|tail| tail.starts_with('/'))
        })
        .max_by_key(|(package, _)| package.len())
        .map(|(_, module)| module.clone())
}

fn module_path_for_file<'a>(
    root: &Path,
    path: &Path,
    modules: impl Iterator<Item = &'a String>,
) -> Option<String> {
    modules
        .filter_map(|module| {
            let module_root = module_root(root, module);
            path.starts_with(&module_root)
                .then_some((module_root.components().count(), module.clone()))
        })
        .max_by_key(|(depth, _)| *depth)
        .map(|(_, module)| module)
}

fn module_root(root: &Path, rel_path: &str) -> PathBuf {
    if rel_path == "." {
        root.to_path_buf()
    } else {
        root.join(rel_path)
    }
}

fn relative_file(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn normalize_source_path(path: &str) -> String {
    path.trim()
        .trim_start_matches("./")
        .trim_start_matches('/')
        .trim_end_matches(".ets")
        .to_string()
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

fn parse_api_level(value: &str) -> Option<i64> {
    if let Some(start) = value.find('(') {
        let end = value[start + 1..].find(')')? + start + 1;
        return value[start + 1..end].trim().parse().ok();
    }
    value.trim().parse().ok()
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

fn normalize_changed_path(root: &Path, path: &str) -> String {
    let path = Path::new(path);
    let relative = if path.is_absolute() {
        path.strip_prefix(root).unwrap_or(path)
    } else {
        path
    };
    normalize_rel(&relative.to_string_lossy())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("harmony-model-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for module in [
            "entry",
            "features/pay",
            "shared/runtime",
            "libs/design",
            "broken",
        ] {
            std::fs::create_dir_all(root.join(module).join("src/main")).unwrap();
        }
        std::fs::create_dir_all(root.join("AppScope")).unwrap();
        std::fs::create_dir_all(root.join("entry/src/main/ets/pages")).unwrap();
        std::fs::create_dir_all(root.join("entry/src/main/resources/base/profile")).unwrap();
        std::fs::create_dir_all(root.join("features/pay/src/main/ets")).unwrap();
        std::fs::write(
            root.join("AppScope/app.json5"),
            r#"{"app":{"bundleName":"com.example.graph","versionCode":7,"versionName":"2.0","label":"Graph"}}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("build-profile.json5"),
            r#"{
              "app":{"signingConfigs":[{"name":"release","material":{"certpath":"configured","profile":"configured","storeFile":"configured","keyAlias":"release","signAlg":"SHA256withECDSA"}}],"buildModeSet":[{"name":"debug"},{"name":"release"}],"products":[
                {"name":"default","compileSdkVersion":"6.0.0(20)","compatibleSdkVersion":"5.0.0(12)","targetSdkVersion":"6.0.0(20)","runtimeOS":"HarmonyOS","signingConfig":"release"},
                {"name":"tablet","compileSdkVersion":20,"compatibleSdkVersion":18,"targetSdkVersion":20,"runtimeOS":"OpenHarmony"}
              ]},
              "modules":[
                {"name":"entry","srcPath":"./entry","targets":[{"name":"default","applyToProducts":["default"]}]},
                {"name":"pay","srcPath":"./features/pay"},
                {"name":"runtime","srcPath":"./shared/runtime"},
                {"name":"design","srcPath":"./libs/design"},
                {"name":"broken","srcPath":"./broken"}
              ]
            }"#,
        )
        .unwrap();
        std::fs::write(
            root.join("oh-package.json5"),
            r#"{"name":"@app/root","dependencies":{"@external/logger":"^1.2.0"}}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("oh-package-lock.json5"),
            r#"{"meta":{"stableOrder":true},"lockfileVersion":3,"specifiers":{"@external/logger@^1.2.0":"@external/logger@1.2.4"},"packages":{"@external/logger@1.2.4":{"name":"@external/logger","version":"1.2.4","resolved":"https://example.invalid/logger.har","integrity":"sha512-test","registryType":"ohpm","dependencies":{"dayjs":"1.11.7"}}}}"#,
        )
        .unwrap();
        let manifests = [
            (
                "entry",
                r#"{"module":{"name":"entry","type":"entry","deviceTypes":["phone"],"mainElement":"EntryAbility","abilities":[{"name":"EntryAbility","srcEntry":"./ets/EntryAbility.ets","exported":true}],"extensionAbilities":[{"name":"BackupExt","type":"backup","srcEntry":"./ets/BackupExt.ets"}],"requestPermissions":[{"name":"ohos.permission.CAMERA","reason":"take photo","usedScene":{"abilities":["EntryAbility"],"when":"inuse"}}]}}"#,
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
        std::fs::write(
            root.join("entry/oh-package-default-lock.json5"),
            r#"{"lockfileVersion":1,"specifiers":{"@app/pay@file:../features/pay":"@app/pay@1.0.0"},"packages":{"@app/pay@1.0.0":{"name":"@app/pay","version":"1.0.0","resolved":"../features/pay","registryType":"local"}}}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("entry/build-profile.json5"),
            r#"{"apiType":"stageMode","buildOptionSet":[{"name":"debug"},{"name":"release"}],"targets":[{"name":"default","applyToProducts":["default"]}]}"#,
        )
        .unwrap();
        std::fs::write(root.join("entry/oh-package-bad-lock.json5"), "{ invalid").unwrap();
        std::fs::write(root.join("libs/design/build-profile.json5"), "{ invalid").unwrap();
        std::fs::write(root.join("broken/src/main/module.json5"), "{ invalid").unwrap();
        std::fs::write(
            root.join("entry/src/main/resources/base/profile/main_pages.json"),
            r#"{"src":["pages/Index"]}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("entry/src/main/resources/base/profile/route_map.json"),
            r#"{"routerMap":[{"name":"PayPage","pageSourceFile":"pages/PayPage.ets","buildFunction":"PayBuilder"}]}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("entry/src/main/ets/pages/Index.ets"),
            "import { PayPage } from '@app/pay'\n@Entry\n@Component\nstruct Index { check = canIUse('SystemCapability.Communication.NetStack') }\n",
        )
        .unwrap();
        std::fs::write(
            root.join("features/pay/src/main/ets/Pay.ets"),
            "import { Runtime } from '@app/runtime'\nexport struct Pay {}\n",
        )
        .unwrap();
        root
    }

    #[test]
    fn parses_products_nested_modules_artifacts_abilities_and_dependency_edges() {
        let root = fixture("full");
        let model = parse(&root);
        assert_eq!(model.app.bundle_name.as_deref(), Some("com.example.graph"));
        assert_eq!(model.products.len(), 2);
        assert_eq!(model.build_modes, vec!["debug", "release"]);
        assert_eq!(model.signing_configs[0].name, "release");
        assert!(model.signing_configs[0].material_configured);
        assert_eq!(model.modules.len(), 4);
        assert_eq!(model.lockfiles.len(), 2);
        let root_lock = model
            .lockfiles
            .iter()
            .find(|lockfile| lockfile.path == "oh-package-lock.json5")
            .unwrap();
        assert_eq!(root_lock.lockfile_version, Some(3));
        assert_eq!(root_lock.packages[0].version.as_deref(), Some("1.2.4"));
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
        assert_eq!(entry.permissions[0].name, "ohos.permission.CAMERA");
        assert_eq!(entry.api_type.as_deref(), Some("stageMode"));
        assert_eq!(entry.build_modes, vec!["debug", "release"]);
        assert!(model.dependencies.iter().any(|d| {
            d.from_module == "entry" && d.target_module.as_deref() == Some("features/pay")
        }));
        assert!(model.dependencies.iter().any(|d| {
            d.from_module == "features/pay" && d.target_module.as_deref() == Some("shared/runtime")
        }));
        let external = model
            .dependencies
            .iter()
            .find(|d| d.name == "@external/logger")
            .unwrap();
        assert_eq!(external.requirement, "^1.2.0");
        assert_eq!(external.locked_version.as_deref(), Some("1.2.4"));
        assert_eq!(external.lockfile.as_deref(), Some("oh-package-lock.json5"));
        let local = model
            .dependencies
            .iter()
            .find(|d| d.from_module == "entry" && d.name == "@app/pay")
            .unwrap();
        assert_eq!(local.locked_version.as_deref(), Some("1.0.0"));
        assert_eq!(
            local.lockfile.as_deref(),
            Some("entry/oh-package-default-lock.json5")
        );
        assert!(model.manifests.iter().any(|source| {
            source.path == "libs/design/build-profile.json5" && source.status == "invalid"
        }));
        assert!(model.manifests.iter().any(|source| {
            source.path == "entry/oh-package-bad-lock.json5" && source.status == "invalid"
        }));
        assert!(model.manifests.iter().any(|source| {
            source.path == "broken/src/main/module.json5" && source.status == "invalid"
        }));
        assert!(model.graph.pages.iter().any(|page| {
            page.module == "entry" && page.path == "pages/Index" && page.source_kind == "main_pages"
        }));
        assert!(model.graph.pages.iter().any(|page| {
            page.route_name.as_deref() == Some("PayPage") && page.source_kind == "router_map"
        }));
        assert!(model.graph.system_capabilities.iter().any(|reference| {
            reference.capability == "SystemCapability.Communication.NetStack"
        }));
        assert!(model.graph.cross_module_refs.iter().any(|reference| {
            reference.from_module == "entry"
                && reference.to_module == "features/pay"
                && reference.specifier == "@app/pay"
        }));
        assert!(model.graph.cross_module_refs.iter().any(|reference| {
            reference.from_module == "features/pay" && reference.to_module == "shared/runtime"
        }));
        assert!(model.graph.edges.iter().any(|edge| {
            edge.from == "module:entry"
                && edge.to == "permission:ohos.permission.CAMERA"
                && edge.kind == "requests"
        }));
        let default = model.products.iter().find(|p| p.name == "default").unwrap();
        assert!(default.modules.contains(&"entry".to_string()));
        let tablet = model.products.iter().find(|p| p.name == "tablet").unwrap();
        assert_eq!(tablet.compile_api_level, Some(20));
        assert_eq!(tablet.compatible_api_level, Some(18));
        assert_eq!(tablet.runtime_os.as_deref(), Some("OpenHarmony"));
        assert!(!tablet.modules.contains(&"entry".to_string()));
        assert!(tablet.modules.contains(&"features/pay".to_string()));
        assert!(model.product_differences.iter().any(|difference| {
            difference.product == "tablet"
                && difference.fields.contains(&"runtimeOS".to_string())
                && difference.fields.contains(&"modules".to_string())
        }));

        let summary = crate::services::harmony::project_summary(&root, &model);
        assert_eq!(summary.entry_module.as_deref(), Some("entry"));
        assert_eq!(summary.main_element.as_deref(), Some("EntryAbility"));
        assert_eq!(summary.api_version, Some(12));
        assert!(summary.signing_configured);
        assert_eq!(
            summary.hap_output_dir.as_deref(),
            Some(root.join("entry/build/default/outputs/default").as_path())
        );
        let legacy_routes = crate::services::harmony::routes_from_model(&model, Some("entry"));
        assert!(legacy_routes.contains(&"pages/Index".to_string()));
        assert!(legacy_routes.contains(&"pages/PayPage".to_string()));

        let impact = analyze_impact(&root, &model, &["features/pay/src/main/ets/Pay.ets".into()]);
        assert_eq!(impact.mode, "incremental");
        assert_eq!(impact.direct_modules, vec!["features/pay"]);
        assert!(impact.affected_modules.contains(&"entry".to_string()));
        assert!(impact
            .verification
            .products
            .contains(&"default".to_string()));
        assert!(impact.verification.checks.contains(&"build".to_string()));
        assert!(impact.verification.checks.contains(&"lint".to_string()));
        assert!(impact.traces.iter().any(|trace| {
            trace.module == "entry"
                && trace.depends_on.as_deref() == Some("features/pay")
                && (trace.kind == "dependency" || trace.kind == "import")
        }));
        let structural_impact = analyze_impact(&root, &model, &["build-profile.json5".into()]);
        assert_eq!(structural_impact.mode, "full");
        assert_eq!(
            structural_impact.affected_modules.len(),
            model.modules.len()
        );

        let unchanged_runtime = serde_json::to_value(
            model
                .modules
                .iter()
                .find(|module| module.rel_path == "shared/runtime")
                .unwrap(),
        )
        .unwrap();
        std::fs::write(
            root.join("features/pay/src/main/ets/Pay.ets"),
            "import { Design } from '@app/design'\nexport struct Pay {}\n",
        )
        .unwrap();
        let update =
            refresh_after_changes(&root, &model, &["features/pay/src/main/ets/Pay.ets".into()]);
        assert_eq!(update.mode, "incremental");
        assert!(update
            .affected_modules
            .contains(&"features/pay".to_string()));
        assert!(update.affected_modules.contains(&"entry".to_string()));
        assert!(update.verification.checks.contains(&"lint".to_string()));
        assert!(update.verification.checks.contains(&"test".to_string()));
        assert!(update
            .model
            .graph
            .cross_module_refs
            .iter()
            .any(|reference| {
                reference.from_module == "features/pay" && reference.to_module == "libs/design"
            }));
        assert_eq!(
            serde_json::to_value(
                update
                    .model
                    .modules
                    .iter()
                    .find(|module| module.rel_path == "shared/runtime")
                    .unwrap()
            )
            .unwrap(),
            unchanged_runtime
        );
        let full = refresh_after_changes(&root, &update.model, &["build-profile.json5".into()]);
        assert_eq!(full.mode, "full");
        assert_eq!(full.affected_modules.len(), full.model.modules.len());

        let cached_model = cached(&root);
        assert!(cached_model
            .graph
            .cross_module_refs
            .iter()
            .any(|reference| reference.to_module == "libs/design"));
        std::fs::write(
            root.join("features/pay/src/main/ets/Pay.ets"),
            "import { Runtime } from '@app/runtime'\nexport struct Pay {}\n",
        )
        .unwrap();
        let changed_path = root.join("features/pay/src/main/ets/Pay.ets");
        let cached_update = invalidate_files(&root, &[changed_path.to_string_lossy().into()]);
        assert_eq!(cached_update.mode, "incremental");
        assert_eq!(
            cached_update.changed_files,
            vec!["features/pay/src/main/ets/Pay.ets"]
        );
        assert!(cached(&root)
            .graph
            .cross_module_refs
            .iter()
            .any(|reference| reference.to_module == "shared/runtime"));
        std::fs::remove_dir_all(root).ok();
    }
}
