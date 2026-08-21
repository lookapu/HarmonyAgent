//! HarmonyOS 构建闭环的持久工作流状态与只读证据收集。

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

const CHECKPOINT_SCHEMA: u32 = 1;
const SKIP_DIRS: &[&str] = &[
    ".git",
    ".hvigor",
    ".ohpm",
    "build",
    "node_modules",
    "oh_modules",
];

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HarmonyBuildCheckpoint {
    pub schema_version: u32,
    pub workflow_key: String,
    pub project_fingerprint: String,
    /// running / failed / completed
    pub status: String,
    pub completed_stages: Vec<String>,
    pub current_stage: String,
    pub last_error: Option<String>,
    pub artifacts: Vec<HarmonyBuildArtifact>,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HarmonyBuildArtifact {
    pub path: String,
    pub kind: String,
    pub size: u64,
    pub modified_at: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HarmonyDependencyState {
    pub declared: usize,
    pub missing: Vec<String>,
}

/// 一次 Hvigor 调用对应的最小可构建目标。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarmonyBuildTarget {
    pub module: String,
    pub module_path: String,
    pub product: String,
    pub mode: String,
    /// assembleHap / assembleHsp / assembleHar
    pub task: String,
    pub reason: String,
}

/// 构建前由工程模型和文件影响范围生成的可审计计划。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HarmonyBuildPlan {
    /// explicit / incremental / full / default
    pub scope: String,
    pub changed_files: Vec<String>,
    pub targets: Vec<HarmonyBuildTarget>,
}

/// 将用户约束与影响分析合并为最小顶层产物集合。
///
/// 依赖模块本身不重复构建：如果受影响的 HAR/HSP 还有受影响的上游 HAP，
/// 只构建该 HAP，由 Hvigor 在同一依赖闭包内完成底层产物。
pub fn plan_build(
    root: &Path,
    model: &crate::services::harmony_model::HarmonySemanticModel,
    requested_module: Option<&str>,
    requested_product: Option<&str>,
    requested_mode: &str,
    changed_files: &[String],
) -> Result<HarmonyBuildPlan, String> {
    let explicit_module = requested_module.and_then(|requested| {
        model
            .modules
            .iter()
            .find(|module| module.name == requested || module.rel_path == requested)
    });
    if let Some(requested) = requested_module {
        if explicit_module.is_none() {
            let available = model
                .modules
                .iter()
                .map(|module| module.name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            return Err(format!(
                "指定模块 {requested} 不存在，工程可构建模块：{}",
                if available.is_empty() {
                    "(未识别)"
                } else {
                    &available
                }
            ));
        }
    }

    let explicit_product = requested_product.and_then(|requested| {
        model
            .products
            .iter()
            .find(|product| product.name == requested)
    });
    if let Some(requested) = requested_product {
        if explicit_product.is_none() && !model.products.is_empty() {
            return Err(format!(
                "指定产品 {requested} 不存在，工程产品：{}",
                model
                    .products
                    .iter()
                    .map(|product| product.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }

    if !model.build_modes.is_empty() && !model.build_modes.iter().any(|mode| mode == requested_mode)
    {
        return Err(format!(
            "构建模式 {requested_mode} 不在工程 buildModeSet 中：{}",
            model.build_modes.join(", ")
        ));
    }

    let impact = (!changed_files.is_empty())
        .then(|| crate::services::harmony_model::analyze_impact(root, model, changed_files));
    let scope = if requested_module.is_some() || requested_product.is_some() {
        "explicit".to_string()
    } else if let Some(impact) = &impact {
        impact.mode.clone()
    } else {
        "default".to_string()
    };

    let product_names = if let Some(requested) = requested_product {
        vec![requested.to_string()]
    } else if let Some(impact) = &impact {
        if impact.verification.products.is_empty() {
            default_product_names(model)
        } else {
            impact.verification.products.clone()
        }
    } else {
        default_product_names(model)
    };

    let affected = impact
        .as_ref()
        .map(|item| {
            item.affected_modules
                .iter()
                .cloned()
                .collect::<BTreeSet<_>>()
        })
        .unwrap_or_default();
    let base_modules = if let Some(module) = explicit_module {
        vec![module]
    } else if !affected.is_empty() {
        let buildable = model
            .modules
            .iter()
            .filter(|module| {
                affected.contains(&module.rel_path)
                    && artifact_task(&module.artifact_kind).is_some()
            })
            .collect::<Vec<_>>();
        let top_level = buildable
            .iter()
            .copied()
            .filter(|module| !has_affected_downstream(model, &module.rel_path, &affected))
            .collect::<Vec<_>>();
        if top_level.is_empty() {
            buildable
        } else {
            top_level
        }
    } else {
        model
            .modules
            .iter()
            .find(|module| module.kind == "entry")
            .or_else(|| {
                model
                    .modules
                    .iter()
                    .find(|module| module.artifact_kind == "hap")
            })
            .or_else(|| {
                model
                    .modules
                    .iter()
                    .find(|module| artifact_task(&module.artifact_kind).is_some())
            })
            .into_iter()
            .collect()
    };
    if base_modules.is_empty() {
        return Err("工程模型中没有可构建的 HAP/HSP/HAR 模块".into());
    }

    let mut targets = Vec::new();
    let mut seen = BTreeSet::new();
    for product_name in product_names {
        let product = model
            .products
            .iter()
            .find(|product| product.name == product_name);
        for module in &base_modules {
            if product.is_some_and(|product| {
                !product.modules.is_empty() && !product.modules.contains(&module.rel_path)
            }) {
                if requested_module.is_some() || requested_product.is_some() {
                    return Err(format!("模块 {} 不属于产品 {product_name}", module.name));
                }
                continue;
            }
            if !module.build_modes.is_empty()
                && !module.build_modes.iter().any(|mode| mode == requested_mode)
            {
                return Err(format!(
                    "模块 {} 不支持构建模式 {requested_mode}；可用模式：{}",
                    module.name,
                    module.build_modes.join(", ")
                ));
            }
            let Some(task) = artifact_task(&module.artifact_kind) else {
                continue;
            };
            let key = format!("{}@{}:{requested_mode}", module.name, product_name);
            if seen.insert(key) {
                targets.push(HarmonyBuildTarget {
                    module: module.name.clone(),
                    module_path: module.rel_path.clone(),
                    product: product_name.clone(),
                    mode: requested_mode.into(),
                    task: task.into(),
                    reason: if requested_module.is_some() {
                        "用户显式指定".into()
                    } else if impact.is_some() {
                        "文件影响范围的顶层产物".into()
                    } else {
                        "默认入口产物".into()
                    },
                });
            }
        }
    }
    targets.sort_by(|a, b| {
        a.product
            .cmp(&b.product)
            .then_with(|| a.module_path.cmp(&b.module_path))
    });
    if targets.is_empty() {
        return Err("影响范围没有映射到所选产品中的可构建模块".into());
    }
    Ok(HarmonyBuildPlan {
        scope,
        changed_files: impact.map(|item| item.changed_files).unwrap_or_default(),
        targets,
    })
}

fn default_product_names(
    model: &crate::services::harmony_model::HarmonySemanticModel,
) -> Vec<String> {
    vec![model
        .products
        .iter()
        .find(|product| product.name == "default")
        .or_else(|| model.products.first())
        .map(|product| product.name.clone())
        .unwrap_or_else(|| "default".into())]
}

fn artifact_task(kind: &str) -> Option<&'static str> {
    match kind {
        "hap" => Some("assembleHap"),
        "hsp" => Some("assembleHsp"),
        "har" => Some("assembleHar"),
        _ => None,
    }
}

fn has_affected_downstream(
    model: &crate::services::harmony_model::HarmonySemanticModel,
    module_path: &str,
    affected: &BTreeSet<String>,
) -> bool {
    model.dependencies.iter().any(|dependency| {
        dependency.target_module.as_deref() == Some(module_path)
            && affected.contains(&dependency.from_module)
    }) || model.graph.cross_module_refs.iter().any(|reference| {
        reference.to_module == module_path && affected.contains(&reference.from_module)
    })
}

pub fn begin(root: &Path, workflow_key: &str, fingerprint: &str) -> (HarmonyBuildCheckpoint, bool) {
    if let Some(mut checkpoint) = load(root) {
        let resumable = checkpoint.schema_version == CHECKPOINT_SCHEMA
            && checkpoint.workflow_key == workflow_key
            && checkpoint.project_fingerprint == fingerprint
            && matches!(checkpoint.status.as_str(), "running" | "failed");
        if resumable {
            checkpoint.status = "running".into();
            checkpoint.last_error = None;
            checkpoint.updated_at = now();
            save(root, &checkpoint);
            return (checkpoint, true);
        }
    }
    let checkpoint = HarmonyBuildCheckpoint {
        schema_version: CHECKPOINT_SCHEMA,
        workflow_key: workflow_key.into(),
        project_fingerprint: fingerprint.into(),
        status: "running".into(),
        current_stage: "environment".into(),
        updated_at: now(),
        ..Default::default()
    };
    save(root, &checkpoint);
    (checkpoint, false)
}

pub fn stage_completed(root: &Path, checkpoint: &mut HarmonyBuildCheckpoint, stage: &str) {
    if !checkpoint.completed_stages.iter().any(|item| item == stage) {
        checkpoint.completed_stages.push(stage.into());
    }
    checkpoint.current_stage = stage.into();
    checkpoint.status = "running".into();
    checkpoint.last_error = None;
    checkpoint.updated_at = now();
    save(root, checkpoint);
}

pub fn stage_failed(
    root: &Path,
    checkpoint: &mut HarmonyBuildCheckpoint,
    stage: &str,
    error: &str,
) {
    checkpoint.current_stage = stage.into();
    checkpoint.status = "failed".into();
    checkpoint.last_error = Some(
        crate::utils::redact::redact_text(error)
            .chars()
            .take(2000)
            .collect(),
    );
    checkpoint.updated_at = now();
    save(root, checkpoint);
}

pub fn completed(
    root: &Path,
    checkpoint: &mut HarmonyBuildCheckpoint,
    artifacts: Vec<HarmonyBuildArtifact>,
) {
    stage_completed(root, checkpoint, "artifacts");
    checkpoint.status = "completed".into();
    checkpoint.current_stage = "completed".into();
    checkpoint.artifacts = artifacts;
    checkpoint.updated_at = now();
    save(root, checkpoint);
}

pub fn project_fingerprint(root: &Path) -> String {
    fn walk(path: &Path, root: &Path, hasher: &mut Sha256) {
        let Ok(entries) = std::fs::read_dir(path) else {
            return;
        };
        let mut entries = entries.flatten().collect::<Vec<_>>();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let child = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if child.is_dir() {
                if !SKIP_DIRS.contains(&name.as_str()) && !name.starts_with('.') {
                    walk(&child, root, hasher);
                }
                continue;
            }
            let relevant = matches!(
                child.extension().and_then(|ext| ext.to_str()),
                Some("ets" | "ts" | "json5" | "json" | "cpp" | "c" | "h" | "hpp" | "xml")
            );
            if !relevant {
                continue;
            }
            if let Ok(relative) = child.strip_prefix(root) {
                hasher.update(relative.to_string_lossy().as_bytes());
            }
            if let Ok(bytes) = std::fs::read(&child) {
                hasher.update(&bytes);
            }
        }
    }
    let mut hasher = Sha256::new();
    walk(root, root, &mut hasher);
    format!("{:x}", hasher.finalize())
}

pub fn dependency_state(
    root: &Path,
    model: &crate::services::harmony_model::HarmonySemanticModel,
) -> HarmonyDependencyState {
    let mut missing = Vec::new();
    let mut declared = 0;
    for dependency in &model.dependencies {
        if dependency.target_module.is_some()
            || dependency.requirement.starts_with("file:")
            || dependency.requirement.starts_with("link:")
        {
            continue;
        }
        declared += 1;
        let module_root = if dependency.from_module == "." {
            root.to_path_buf()
        } else {
            root.join(&dependency.from_module)
        };
        let candidates = [module_root.join("oh_modules"), root.join("oh_modules")];
        let installed = candidates
            .iter()
            .any(|base| package_dir(base, &dependency.name).is_dir());
        if !installed {
            missing.push(format!("{}:{}", dependency.from_module, dependency.name));
        }
    }
    missing.sort();
    missing.dedup();
    HarmonyDependencyState { declared, missing }
}

pub fn discover_artifacts(root: &Path) -> Vec<HarmonyBuildArtifact> {
    fn walk(path: &Path, root: &Path, out: &mut Vec<HarmonyBuildArtifact>) {
        let Ok(entries) = std::fs::read_dir(path) else {
            return;
        };
        for entry in entries.flatten() {
            let child = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if child.is_dir() {
                if !SKIP_DIRS
                    .iter()
                    .any(|skip| *skip == name && name != "build")
                {
                    walk(&child, root, out);
                }
                continue;
            }
            let Some(kind) = child.extension().and_then(|ext| ext.to_str()) else {
                continue;
            };
            if !matches!(kind, "hap" | "hsp" | "har") {
                continue;
            }
            let metadata = entry.metadata().ok();
            let modified_at = metadata
                .as_ref()
                .and_then(|item| item.modified().ok())
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|duration| duration.as_secs() as i64)
                .unwrap_or(0);
            out.push(HarmonyBuildArtifact {
                path: child
                    .strip_prefix(root)
                    .unwrap_or(&child)
                    .to_string_lossy()
                    .replace('\\', "/"),
                kind: kind.into(),
                size: metadata.map(|item| item.len()).unwrap_or(0),
                modified_at,
            });
        }
    }
    let mut artifacts = Vec::new();
    walk(root, root, &mut artifacts);
    artifacts.sort_by(|a, b| {
        b.modified_at
            .cmp(&a.modified_at)
            .then_with(|| a.path.cmp(&b.path))
    });
    artifacts
}

fn package_dir(base: &Path, name: &str) -> PathBuf {
    if let Some((scope, package)) = name.split_once('/') {
        base.join(scope).join(package)
    } else {
        base.join(name)
    }
}

fn checkpoint_path(root: &Path) -> PathBuf {
    root.join(".deveco-agent")
        .join("harmony-build-workflow.json")
}

fn load(root: &Path) -> Option<HarmonyBuildCheckpoint> {
    let text = std::fs::read_to_string(checkpoint_path(root)).ok()?;
    serde_json::from_str(&text).ok()
}

fn save(root: &Path, checkpoint: &HarmonyBuildCheckpoint) {
    let path = checkpoint_path(root);
    let Some(parent) = path.parent() else {
        return;
    };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    if let Ok(text) = serde_json::to_string_pretty(checkpoint) {
        let _ = std::fs::write(path, text);
    }
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn module(name: &str, path: &str, kind: &str) -> crate::services::harmony_model::HarmonyModule {
        crate::services::harmony_model::HarmonyModule {
            name: name.into(),
            rel_path: path.into(),
            kind: if kind == "hap" { "entry" } else { "shared" }.into(),
            artifact_kind: kind.into(),
            build_modes: vec!["debug".into(), "release".into()],
            ..Default::default()
        }
    }

    fn planning_model() -> crate::services::harmony_model::HarmonySemanticModel {
        crate::services::harmony_model::HarmonySemanticModel {
            build_modes: vec!["debug".into(), "release".into()],
            products: vec![
                crate::services::harmony_model::HarmonyProduct {
                    name: "default".into(),
                    modules: vec!["entry".into(), "libs/core".into(), "shared/kit".into()],
                    ..Default::default()
                },
                crate::services::harmony_model::HarmonyProduct {
                    name: "paid".into(),
                    modules: vec!["entry".into(), "libs/core".into()],
                    ..Default::default()
                },
            ],
            modules: vec![
                module("entry", "entry", "hap"),
                module("core", "libs/core", "har"),
                module("kit", "shared/kit", "hsp"),
            ],
            dependencies: vec![crate::services::harmony_model::HarmonyDependency {
                from_module: "entry".into(),
                name: "core".into(),
                requirement: "file:../libs/core".into(),
                target_module: Some("libs/core".into()),
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    fn root(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("harmony-build-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("entry/src/main/ets")).unwrap();
        root
    }

    #[test]
    fn checkpoint_resumes_only_matching_incomplete_workflow() {
        let root = root("resume");
        std::fs::write(
            root.join("entry/src/main/ets/Index.ets"),
            "@Entry struct Index {}",
        )
        .unwrap();
        let fingerprint = project_fingerprint(&root);
        let (mut checkpoint, resumed) = begin(&root, "entry:debug", &fingerprint);
        assert!(!resumed);
        stage_completed(&root, &mut checkpoint, "environment");
        stage_failed(&root, &mut checkpoint, "build", "interrupted");
        let (checkpoint, resumed) = begin(&root, "entry:debug", &fingerprint);
        assert!(resumed);
        assert!(checkpoint
            .completed_stages
            .contains(&"environment".to_string()));
        std::fs::write(
            root.join("entry/src/main/ets/Index.ets"),
            "@Entry struct Changed {}",
        )
        .unwrap();
        let (_, resumed) = begin(&root, "entry:debug", &project_fingerprint(&root));
        assert!(!resumed);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn dependency_and_artifact_evidence_are_structured() {
        let root = root("evidence");
        std::fs::create_dir_all(root.join("entry/build/default/outputs/default")).unwrap();
        std::fs::write(
            root.join("entry/build/default/outputs/default/app.hap"),
            "hap",
        )
        .unwrap();
        let model = crate::services::harmony_model::HarmonySemanticModel {
            dependencies: vec![crate::services::harmony_model::HarmonyDependency {
                from_module: "entry".into(),
                name: "@ohos/router".into(),
                requirement: "^1.0.0".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let state = dependency_state(&root, &model);
        assert_eq!(state.declared, 1);
        assert_eq!(state.missing, vec!["entry:@ohos/router"]);
        std::fs::create_dir_all(root.join("entry/oh_modules/@ohos/router")).unwrap();
        assert!(dependency_state(&root, &model).missing.is_empty());
        let artifacts = discover_artifacts(&root);
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].kind, "hap");
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn impact_plan_collapses_dependency_modules_into_top_level_artifacts() {
        let model = planning_model();
        let plan = plan_build(
            Path::new("/workspace"),
            &model,
            None,
            None,
            "debug",
            &["libs/core/src/main/ets/Core.ets".into()],
        )
        .unwrap();
        assert_eq!(plan.scope, "incremental");
        assert_eq!(plan.targets.len(), 2);
        assert!(plan.targets.iter().all(|target| target.module == "entry"));
        assert_eq!(
            plan.targets
                .iter()
                .map(|target| target.product.as_str())
                .collect::<Vec<_>>(),
            ["default", "paid"]
        );
        assert!(plan
            .targets
            .iter()
            .all(|target| target.task == "assembleHap"));
    }

    #[test]
    fn impact_plan_builds_independent_hsp_without_unrelated_hap() {
        let model = planning_model();
        let plan = plan_build(
            Path::new("/workspace"),
            &model,
            None,
            None,
            "release",
            &["shared/kit/src/main/ets/Kit.ets".into()],
        )
        .unwrap();
        assert_eq!(plan.targets.len(), 1);
        assert_eq!(plan.targets[0].module, "kit");
        assert_eq!(plan.targets[0].product, "default");
        assert_eq!(plan.targets[0].task, "assembleHsp");
    }

    #[test]
    fn explicit_target_rejects_product_membership_and_unknown_mode() {
        let model = planning_model();
        assert!(plan_build(
            Path::new("/workspace"),
            &model,
            Some("kit"),
            Some("paid"),
            "debug",
            &[],
        )
        .unwrap_err()
        .contains("不属于产品"));
        assert!(plan_build(
            Path::new("/workspace"),
            &model,
            Some("entry"),
            None,
            "profile",
            &[],
        )
        .unwrap_err()
        .contains("buildModeSet"));
    }

    #[test]
    fn planned_target_maps_to_exact_hvigor_parameters() {
        let args = crate::services::harmony::assemble_target_args(
            "assembleHar",
            Some("core"),
            "paid",
            "release",
        );
        assert_eq!(args[0], "assembleHar");
        assert!(args.contains(&"module=core@paid".to_string()));
        assert!(args.contains(&"product=paid".to_string()));
        assert!(args.contains(&"buildMode=release".to_string()));
        assert!(!args.iter().any(|arg| arg.contains("@default")));
    }
}
