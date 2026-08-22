//! HarmonyOS 构建闭环的持久工作流状态与只读证据收集。

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::io::Read;
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
    #[serde(default)]
    pub discovered_at: i64,
    #[serde(default)]
    pub sha256: String,
    /// verified_signed / unsigned / claimed_signed / unknown / not_applicable
    #[serde(default)]
    pub signing_status: String,
    #[serde(default)]
    pub signature_evidence: Option<String>,
    #[serde(default)]
    pub module: Option<String>,
    #[serde(default)]
    pub product: Option<String>,
    #[serde(default)]
    pub build_mode: Option<String>,
    #[serde(default)]
    pub source_step: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HarmonyArtifactManifest {
    pub schema_version: u32,
    pub generated_at: i64,
    pub workflow_key: String,
    pub project_fingerprint: String,
    pub artifacts: Vec<HarmonyBuildArtifact>,
}

#[derive(Debug, Clone)]
pub struct HarmonyDeployArtifact {
    pub absolute_path: PathBuf,
    pub artifact: HarmonyBuildArtifact,
}

#[derive(Debug, Clone)]
pub struct HarmonyArtifactVerification {
    pub sha256: String,
    pub signing_status: String,
    pub signature_evidence: Option<String>,
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
    discover_artifacts_with_context(root, None, None)
}

/// 发现产物、补齐可验证元数据并持久化本次构建清单。
pub fn record_artifact_manifest(
    root: &Path,
    model: &crate::services::harmony_model::HarmonySemanticModel,
    plan: &HarmonyBuildPlan,
    workflow_key: &str,
    project_fingerprint: &str,
) -> Result<HarmonyArtifactManifest, String> {
    let artifacts = discover_artifacts_with_context(root, Some(model), Some(plan));
    if artifacts.is_empty() {
        return Err("Hvigor 返回成功，但未发现 HAP/HSP/HAR 产物".into());
    }
    if artifacts.iter().any(|artifact| artifact.sha256.is_empty()) {
        return Err("至少一个构建产物无法读取，未能生成完整 SHA-256 清单".into());
    }
    let manifest = HarmonyArtifactManifest {
        schema_version: 1,
        generated_at: now(),
        workflow_key: workflow_key.into(),
        project_fingerprint: project_fingerprint.into(),
        artifacts,
    };
    let path = artifact_manifest_path(root);
    let parent = path
        .parent()
        .ok_or_else(|| "产物清单路径缺少父目录".to_string())?;
    std::fs::create_dir_all(parent).map_err(|error| format!("创建产物清单目录失败：{error}"))?;
    let text = serde_json::to_string_pretty(&manifest)
        .map_err(|error| format!("序列化产物清单失败：{error}"))?;
    std::fs::write(&path, text).map_err(|error| format!("写入产物清单失败：{error}"))?;
    Ok(manifest)
}

pub fn load_artifact_manifest(root: &Path) -> Option<HarmonyArtifactManifest> {
    let text = std::fs::read_to_string(artifact_manifest_path(root)).ok()?;
    serde_json::from_str(&text).ok()
}

/// 从最近一次成功构建清单选择默认部署产物。
///
/// 只接受内容哈希仍匹配、签名结构仍可验证且有本次 build 来源的 HAP。
/// 跨产品/模块或同一时间存在多个候选时返回要求显式确认的错误。
pub fn select_deploy_artifact(
    root: &Path,
    requested_product: Option<&str>,
    requested_module: Option<&str>,
) -> Result<HarmonyDeployArtifact, String> {
    let manifest = load_artifact_manifest(root)
        .ok_or_else(|| "缺少持久产物清单；请先运行 build_project，再部署".to_string())?;
    let current_fingerprint = project_fingerprint(root);
    if manifest.project_fingerprint != current_fingerprint {
        return Err(
            "产物清单生成后工程源码或配置已变化；请先重新运行 build_project，或显式传 hap 确认部署旧产物"
                .into(),
        );
    }
    let canonical_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let mut candidates = Vec::new();
    let mut rejected = Vec::new();
    for artifact in manifest
        .artifacts
        .iter()
        .filter(|artifact| artifact.kind == "hap")
        .filter(|artifact| artifact.source_step.starts_with("build:"))
        .filter(|artifact| {
            requested_product.is_none_or(|product| artifact.product.as_deref() == Some(product))
        })
        .filter(|artifact| {
            requested_module.is_none_or(|module| artifact.module.as_deref() == Some(module))
        })
    {
        let relative = Path::new(&artifact.path);
        if relative.is_absolute()
            || relative
                .components()
                .any(|component| matches!(component, std::path::Component::ParentDir))
        {
            rejected.push(format!("{}: unsafe_path", artifact.path));
            continue;
        }
        let absolute = root.join(relative);
        let canonical = match std::fs::canonicalize(&absolute) {
            Ok(path) if path.starts_with(&canonical_root) => path,
            _ => {
                rejected.push(format!("{}: missing_or_outside_root", artifact.path));
                continue;
            }
        };
        let verification = match verify_artifact_file(&canonical, "hap") {
            Ok(verification) => verification,
            Err(error) => {
                rejected.push(format!("{}: {error}", artifact.path));
                continue;
            }
        };
        if verification.sha256 != artifact.sha256 {
            rejected.push(format!("{}: hash_mismatch", artifact.path));
            continue;
        }
        if verification.signing_status != "verified_signed" {
            rejected.push(format!(
                "{}: signing={}",
                artifact.path, verification.signing_status
            ));
            continue;
        }
        if artifact.module.is_none() || artifact.product.is_none() {
            rejected.push(format!("{}: ambiguous_provenance", artifact.path));
            continue;
        }
        candidates.push(HarmonyDeployArtifact {
            absolute_path: canonical,
            artifact: artifact.clone(),
        });
    }
    if candidates.is_empty() {
        let filter = format!(
            "product={}, module={}",
            requested_product.unwrap_or("*"),
            requested_module.unwrap_or("*")
        );
        return Err(format!(
            "没有满足 {filter} 的可验证最新签名 HAP；请重新 build_project 或显式传 hap。拒绝证据：{}",
            if rejected.is_empty() {
                "清单内没有本次构建来源的 HAP".into()
            } else {
                rejected.into_iter().take(6).collect::<Vec<_>>().join("; ")
            }
        ));
    }
    candidates.sort_by(|a, b| {
        b.artifact
            .modified_at
            .cmp(&a.artifact.modified_at)
            .then_with(|| a.artifact.path.cmp(&b.artifact.path))
    });
    let groups = candidates
        .iter()
        .map(|candidate| {
            format!(
                "{}@{}",
                candidate.artifact.module.as_deref().unwrap_or("unknown"),
                candidate.artifact.product.as_deref().unwrap_or("unknown")
            )
        })
        .collect::<BTreeSet<_>>();
    let newest_time = candidates[0].artifact.modified_at;
    let newest_count = candidates
        .iter()
        .take_while(|candidate| candidate.artifact.modified_at == newest_time)
        .count();
    if groups.len() > 1 || newest_count > 1 {
        let choices = candidates
            .iter()
            .take(8)
            .map(|candidate| {
                format!(
                    "{} [module={} product={} mtime={} sha256={}]",
                    candidate.artifact.path,
                    candidate.artifact.module.as_deref().unwrap_or("unknown"),
                    candidate.artifact.product.as_deref().unwrap_or("unknown"),
                    candidate.artifact.modified_at,
                    &candidate.artifact.sha256[..12]
                )
            })
            .collect::<Vec<_>>()
            .join("; ");
        return Err(format!(
            "部署产物存在歧义，需要用户确认：{choices}。请再次调用 deploy/deploy_all 并显式传 hap，或用 product + module 缩小范围"
        ));
    }
    Ok(candidates.remove(0))
}

pub fn verify_artifact_file(
    path: &Path,
    kind: &str,
) -> Result<HarmonyArtifactVerification, String> {
    let sha256 = file_sha256(path).ok_or_else(|| format!("无法读取产物：{}", path.display()))?;
    let (signing_status, signature_evidence) = signature_status(path, kind);
    Ok(HarmonyArtifactVerification {
        sha256,
        signing_status,
        signature_evidence,
    })
}

fn discover_artifacts_with_context(
    root: &Path,
    model: Option<&crate::services::harmony_model::HarmonySemanticModel>,
    plan: Option<&HarmonyBuildPlan>,
) -> Vec<HarmonyBuildArtifact> {
    fn walk(
        path: &Path,
        root: &Path,
        model: Option<&crate::services::harmony_model::HarmonySemanticModel>,
        plan: Option<&HarmonyBuildPlan>,
        discovered_at: i64,
        out: &mut Vec<HarmonyBuildArtifact>,
    ) {
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
                    walk(&child, root, model, plan, discovered_at, out);
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
            let relative = child
                .strip_prefix(root)
                .unwrap_or(&child)
                .to_string_lossy()
                .replace('\\', "/");
            let provenance = artifact_provenance(&relative, kind, model, plan);
            let (signing_status, signature_evidence) = signature_status(&child, kind);
            out.push(HarmonyBuildArtifact {
                path: relative,
                kind: kind.into(),
                size: metadata.map(|item| item.len()).unwrap_or(0),
                modified_at,
                discovered_at,
                sha256: file_sha256(&child).unwrap_or_default(),
                signing_status,
                signature_evidence,
                module: provenance.as_ref().map(|item| item.module.clone()),
                product: provenance.as_ref().and_then(|item| item.product.clone()),
                build_mode: provenance.as_ref().and_then(|item| item.build_mode.clone()),
                source_step: provenance
                    .map(|item| item.source_step)
                    .unwrap_or_else(|| "workspace_discovery".into()),
            });
        }
    }
    let mut artifacts = Vec::new();
    walk(root, root, model, plan, now(), &mut artifacts);
    artifacts.sort_by(|a, b| {
        b.modified_at
            .cmp(&a.modified_at)
            .then_with(|| a.path.cmp(&b.path))
    });
    artifacts
}

#[derive(Debug, Clone)]
struct ArtifactProvenance {
    module: String,
    product: Option<String>,
    build_mode: Option<String>,
    source_step: String,
}

fn artifact_provenance(
    relative: &str,
    kind: &str,
    model: Option<&crate::services::harmony_model::HarmonySemanticModel>,
    plan: Option<&HarmonyBuildPlan>,
) -> Option<ArtifactProvenance> {
    let model = model?;
    let module = model
        .modules
        .iter()
        .filter(|module| {
            relative == module.rel_path
                || relative
                    .strip_prefix(&module.rel_path)
                    .is_some_and(|tail| tail.starts_with('/'))
        })
        .max_by_key(|module| module.rel_path.len())?;
    let product = infer_product(relative, &module.rel_path, model);
    let target = plan.and_then(|plan| {
        plan.targets.iter().find(|target| {
            target.module_path == module.rel_path
                && artifact_task(kind) == Some(target.task.as_str())
                && product
                    .as_ref()
                    .map_or(plan.targets.len() == 1, |name| name == &target.product)
        })
    });
    Some(ArtifactProvenance {
        module: module.name.clone(),
        product: product.or_else(|| target.map(|item| item.product.clone())),
        build_mode: target.map(|item| item.mode.clone()),
        source_step: target
            .map(|item| {
                format!(
                    "build:{}@{}/{}:{}",
                    item.module, item.product, item.mode, item.task
                )
            })
            .unwrap_or_else(|| "workspace_discovery".into()),
    })
}

fn infer_product(
    relative: &str,
    module_path: &str,
    model: &crate::services::harmony_model::HarmonySemanticModel,
) -> Option<String> {
    let tail = relative.strip_prefix(module_path).unwrap_or(relative);
    let components = tail
        .split('/')
        .filter(|component| !component.is_empty())
        .collect::<Vec<_>>();
    let filename = components.last().copied().unwrap_or_default();
    let candidates = model
        .products
        .iter()
        .filter(|product| {
            product.modules.is_empty() || product.modules.iter().any(|path| path == module_path)
        })
        .filter(|product| {
            components
                .iter()
                .any(|component| *component == product.name)
                || filename.contains(&format!("-{}-", product.name))
                || filename.starts_with(&format!("{}-", product.name))
        })
        .map(|product| product.name.clone())
        .collect::<Vec<_>>();
    if candidates.len() == 1 {
        return candidates.into_iter().next();
    }
    let eligible = model
        .products
        .iter()
        .filter(|product| {
            product.modules.is_empty() || product.modules.iter().any(|path| path == module_path)
        })
        .collect::<Vec<_>>();
    (eligible.len() == 1).then(|| eligible[0].name.clone())
}

fn file_sha256(path: &Path) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).ok()?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Some(format!("{:x}", hasher.finalize()))
}

fn signature_status(path: &Path, kind: &str) -> (String, Option<String>) {
    if kind == "har" {
        return ("not_applicable".into(), Some("HAR library archive".into()));
    }
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if filename.contains("unsigned") {
        return ("unsigned".into(), Some("filename:unsigned".into()));
    }
    if let Ok(file) = std::fs::File::open(path) {
        if let Ok(mut archive) = zip::ZipArchive::new(file) {
            let mut manifests = Vec::new();
            let mut signature_blocks = Vec::new();
            for index in 0..archive.len() {
                let Ok(entry) = archive.by_index(index) else {
                    continue;
                };
                let name = entry.name().to_ascii_lowercase();
                if !name.starts_with("meta-inf/") {
                    continue;
                }
                match Path::new(&name).extension().and_then(|ext| ext.to_str()) {
                    Some("sf") => manifests.push(name),
                    Some("rsa" | "dsa" | "ec" | "p7b" | "cer" | "cert") => {
                        signature_blocks.push(name)
                    }
                    _ => {}
                }
            }
            if !manifests.is_empty() && !signature_blocks.is_empty() {
                manifests.sort();
                signature_blocks.sort();
                return (
                    "verified_signed".into(),
                    Some(format!(
                        "archive:manifest={} block={}",
                        manifests.join(","),
                        signature_blocks.join(",")
                    )),
                );
            }
        }
    }
    if filename.contains("signed") {
        return ("claimed_signed".into(), Some("filename:signed".into()));
    }
    ("unknown".into(), None)
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

fn artifact_manifest_path(root: &Path) -> PathBuf {
    root.join(".deveco-agent").join("harmony-artifacts.json")
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
    use std::io::Write;

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

    fn write_signed_hap(path: &Path) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        let file = std::fs::File::create(path).unwrap();
        let mut archive = zip::ZipWriter::new(file);
        let options: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default();
        archive.start_file("module.json", options).unwrap();
        archive.write_all(b"{}").unwrap();
        archive.start_file("META-INF/APP.RSA", options).unwrap();
        archive.write_all(b"signature").unwrap();
        archive.start_file("META-INF/APP.SF", options).unwrap();
        archive.write_all(b"manifest digest").unwrap();
        archive.finish().unwrap();
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

    #[test]
    fn artifact_manifest_records_hash_signature_product_and_source_step() {
        let root = root("manifest");
        let output = root.join("entry/build/default/outputs/default");
        std::fs::create_dir_all(&output).unwrap();
        let hap = output.join("entry-default-release.hap");
        write_signed_hap(&hap);

        let model = crate::services::harmony_model::HarmonySemanticModel {
            products: vec![crate::services::harmony_model::HarmonyProduct {
                name: "default".into(),
                modules: vec!["entry".into()],
                ..Default::default()
            }],
            modules: vec![module("entry", "entry", "hap")],
            ..Default::default()
        };
        let plan = HarmonyBuildPlan {
            scope: "default".into(),
            targets: vec![HarmonyBuildTarget {
                module: "entry".into(),
                module_path: "entry".into(),
                product: "default".into(),
                mode: "release".into(),
                task: "assembleHap".into(),
                reason: "test".into(),
            }],
            ..Default::default()
        };
        let source = root.join("entry/src/main/ets/Index.ets");
        std::fs::write(&source, "@Entry struct Before {}").unwrap();
        let fingerprint = project_fingerprint(&root);
        let manifest =
            record_artifact_manifest(&root, &model, &plan, "workflow", &fingerprint).unwrap();
        assert_eq!(manifest.artifacts.len(), 1);
        let artifact = &manifest.artifacts[0];
        assert_eq!(artifact.signing_status, "verified_signed");
        assert_eq!(artifact.product.as_deref(), Some("default"));
        assert_eq!(artifact.module.as_deref(), Some("entry"));
        assert_eq!(artifact.build_mode.as_deref(), Some("release"));
        assert_eq!(
            artifact.source_step,
            "build:entry@default/release:assembleHap"
        );
        assert_eq!(artifact.sha256.len(), 64);
        assert_eq!(
            load_artifact_manifest(&root).unwrap().workflow_key,
            "workflow"
        );
        let selected = select_deploy_artifact(&root, None, None).unwrap();
        assert_eq!(selected.artifact.path, artifact.path);
        std::fs::write(&source, "@Entry struct After {}").unwrap();
        assert!(select_deploy_artifact(&root, None, None)
            .unwrap_err()
            .contains("源码或配置已变化"));
        std::fs::write(&source, "@Entry struct Before {}").unwrap();
        let mut file = std::fs::OpenOptions::new().append(true).open(&hap).unwrap();
        file.write_all(b"tampered").unwrap();
        assert!(select_deploy_artifact(&root, None, None)
            .unwrap_err()
            .contains("hash_mismatch"));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn deploy_selection_requires_confirmation_across_products() {
        let root = root("deploy-ambiguity");
        write_signed_hap(&root.join("entry/build/default/outputs/default/app-default.hap"));
        write_signed_hap(&root.join("entry/build/paid/outputs/paid/app-paid.hap"));
        let model = crate::services::harmony_model::HarmonySemanticModel {
            products: vec![
                crate::services::harmony_model::HarmonyProduct {
                    name: "default".into(),
                    modules: vec!["entry".into()],
                    ..Default::default()
                },
                crate::services::harmony_model::HarmonyProduct {
                    name: "paid".into(),
                    modules: vec!["entry".into()],
                    ..Default::default()
                },
            ],
            modules: vec![module("entry", "entry", "hap")],
            ..Default::default()
        };
        let plan = HarmonyBuildPlan {
            scope: "full".into(),
            targets: ["default", "paid"]
                .into_iter()
                .map(|product| HarmonyBuildTarget {
                    module: "entry".into(),
                    module_path: "entry".into(),
                    product: product.into(),
                    mode: "debug".into(),
                    task: "assembleHap".into(),
                    reason: "test".into(),
                })
                .collect(),
            ..Default::default()
        };
        record_artifact_manifest(
            &root,
            &model,
            &plan,
            "workflow",
            &project_fingerprint(&root),
        )
        .unwrap();
        assert!(select_deploy_artifact(&root, None, None)
            .unwrap_err()
            .contains("需要用户确认"));
        let selected = select_deploy_artifact(&root, Some("paid"), Some("entry")).unwrap();
        assert_eq!(selected.artifact.product.as_deref(), Some("paid"));
        std::fs::remove_dir_all(root).ok();
    }
}
