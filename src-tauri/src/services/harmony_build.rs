//! HarmonyOS 构建闭环的持久工作流状态与只读证据收集。

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
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
}
