//! 可解释的 HarmonyOS/OpenHarmony 指纹识别。
//!
//! 识别结果不是权限依据，也不替代统一语义模型。它只把工程清单、ArkTS 源码、
//! API 导入和构建日志中的多类信号合并成带证据的分类，供能力包选择、上下文事实
//! 与固定评测复用。构建、部署和发布仍需重新读取工程与环境真源。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const MAX_SOURCE_FILES: usize = 32;
const MAX_SOURCE_BYTES: u64 = 256 * 1024;
const MAX_DEPTH: usize = 6;
const SKIP_DIRS: &[&str] = &[
    ".git",
    ".hvigor",
    ".idea",
    ".ohpm",
    "build",
    "node_modules",
    "oh_modules",
    "target",
];

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct HarmonyFingerprintEvidence {
    /// 稳定机器码，例如 project.app_manifest / source.arkui_decorator。
    pub code: String,
    /// project / source / api / toolchain / log。
    pub kind: String,
    /// 仅保存相对路径或调用方提供的非敏感标签。
    pub source: String,
    pub weight: u8,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct HarmonyFingerprintReport {
    pub schema_version: u32,
    /// harmony_project / harmony_module / harmony_source / harmony_log / unknown
    pub classification: String,
    pub confidence: u8,
    /// kit / ohos / mixed / unknown。只描述观察到的导入风格，不据此猜测 API Level。
    pub api_style: String,
    pub recommended_capability_pack: String,
    pub evidence: Vec<HarmonyFingerprintEvidence>,
    pub conflicts: Vec<String>,
}

impl HarmonyFingerprintReport {
    pub fn is_harmony(&self) -> bool {
        match self.classification.as_str() {
            "harmony_module" => self.confidence >= 35,
            "harmony_project" | "harmony_source" | "harmony_log" => self.confidence >= 45,
            _ => false,
        }
    }
}

#[derive(Default)]
struct Signals {
    evidence: Vec<HarmonyFingerprintEvidence>,
    conflicts: Vec<String>,
    has_project_root: bool,
    has_module: bool,
    has_source: bool,
    has_log: bool,
    has_kit: bool,
    has_ohos: bool,
}

impl Signals {
    fn push(&mut self, code: &str, kind: &str, source: impl Into<String>, weight: u8) {
        let source = source.into();
        if self
            .evidence
            .iter()
            .any(|item| item.code == code && item.source == source)
        {
            return;
        }
        self.evidence.push(HarmonyFingerprintEvidence {
            code: code.into(),
            kind: kind.into(),
            source,
            weight,
        });
    }

    fn finish(mut self) -> HarmonyFingerprintReport {
        self.evidence.sort_by(|a, b| {
            b.weight
                .cmp(&a.weight)
                .then_with(|| a.code.cmp(&b.code))
                .then_with(|| a.source.cmp(&b.source))
        });
        self.evidence.dedup();
        self.conflicts.sort();
        self.conflicts.dedup();
        // 同类信号在很多文件中重复出现时只计最高权重，避免“文件多”冒充“证据多样”。
        let mut weights = BTreeMap::<&str, u8>::new();
        for item in &self.evidence {
            weights
                .entry(&item.code)
                .and_modify(|weight| *weight = (*weight).max(item.weight))
                .or_insert(item.weight);
        }
        let confidence = weights
            .values()
            .map(|weight| usize::from(*weight))
            .sum::<usize>()
            .min(100) as u8;
        let classification = if self.has_project_root {
            "harmony_project"
        } else if self.has_module {
            "harmony_module"
        } else if self.has_log {
            "harmony_log"
        } else if self.has_source {
            "harmony_source"
        } else {
            "unknown"
        };
        let api_style = match (self.has_kit, self.has_ohos) {
            (true, true) => "mixed",
            (true, false) => "kit",
            (false, true) => "ohos",
            (false, false) => "unknown",
        };
        let has_fault = self
            .evidence
            .iter()
            .any(|item| item.code == "log.harmony_fault");
        let recommended_capability_pack = match classification {
            "harmony_project" | "harmony_module" | "harmony_source" => "project_understanding",
            "harmony_log" if has_fault => "device_diagnostics",
            "harmony_log" => "compile_fix",
            _ => "project_understanding",
        };
        HarmonyFingerprintReport {
            schema_version: 1,
            classification: classification.into(),
            confidence,
            api_style: api_style.into(),
            recommended_capability_pack: recommended_capability_pack.into(),
            evidence: self.evidence,
            conflicts: self.conflicts,
        }
    }
}

/// 检查一个目录。只读取有界的公开工程配置和 ArkTS 源文件，不跟随目录符号链接。
pub fn inspect_path(root: &Path) -> HarmonyFingerprintReport {
    let mut signals = Signals::default();
    let app_manifest = root.join("AppScope/app.json5");
    if is_regular_file(&app_manifest) {
        let valid = std::fs::read_to_string(&app_manifest)
            .ok()
            .and_then(|text| crate::services::harmony::parse_json5(&text).ok())
            .is_some_and(|value| value.get("app").is_some());
        if valid {
            signals.has_project_root = true;
            signals.push("project.app_manifest", "project", "AppScope/app.json5", 45);
        } else {
            signals
                .conflicts
                .push("invalid_app_manifest:AppScope/app.json5".into());
        }
    }

    let build_profile = root.join("build-profile.json5");
    if is_regular_file(&build_profile) {
        let text = std::fs::read_to_string(&build_profile).unwrap_or_default();
        match crate::services::harmony::parse_json5(&text) {
            Ok(value) if value.get("app").is_some() => {
                signals.has_project_root = true;
                signals.push(
                    "project.root_build_profile",
                    "project",
                    "build-profile.json5",
                    35,
                );
            }
            Ok(_) => {
                signals.has_module = true;
                signals.push(
                    "project.module_build_profile",
                    "project",
                    "build-profile.json5",
                    18,
                );
            }
            Err(_) => signals
                .conflicts
                .push("invalid_build_profile:build-profile.json5".into()),
        }
    }

    let module_manifest = root.join("src/main/module.json5");
    if is_regular_file(&module_manifest) {
        let text = std::fs::read_to_string(&module_manifest).unwrap_or_default();
        inspect_text_into(&text, "src/main/module.json5", &mut signals);
    }
    if is_regular_file(&root.join("oh-package.json5")) {
        signals.has_module = true;
        signals.push("project.ohpm_manifest", "project", "oh-package.json5", 12);
    }
    if ["hvigorfile.ts", "hvigorfile.js"]
        .iter()
        .any(|name| is_regular_file(&root.join(name)))
    {
        signals.has_module = true;
        signals.push("toolchain.hvigor_file", "toolchain", ".", 20);
    }

    let mut files = Vec::new();
    collect_ets_files(root, 0, &mut files);
    for path in files {
        let Ok(metadata) = std::fs::metadata(&path) else {
            continue;
        };
        if metadata.len() > MAX_SOURCE_BYTES {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        inspect_text_into(&text, &rel, &mut signals);
    }
    signals.finish()
}

/// 检查单段代码、配置或日志。`source_hint` 只用于扩展名和证据标签。
pub fn inspect_text(text: &str, source_hint: Option<&str>) -> HarmonyFingerprintReport {
    let mut signals = Signals::default();
    let source = safe_source_hint(source_hint.unwrap_or("inline"));
    inspect_text_into(text, &source, &mut signals);
    signals.finish()
}

fn safe_source_hint(source: &str) -> String {
    let path = Path::new(source);
    let value = if path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("inline")
            .to_string()
    } else {
        source.replace('\\', "/")
    };
    value.chars().take(160).collect()
}

fn is_regular_file(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_file())
}

fn inspect_text_into(text: &str, source: &str, signals: &mut Signals) {
    let lower = text.to_ascii_lowercase();
    if Path::new(source)
        .extension()
        .and_then(|value| value.to_str())
        == Some("ets")
    {
        signals.has_source = true;
        signals.push("source.ets_extension", "source", source, 15);
    }
    if [
        "@Entry",
        "@Component",
        "@State",
        "@Link",
        "@Provide",
        "@Consume",
    ]
    .iter()
    .any(|needle| text.contains(needle))
    {
        signals.has_source = true;
        signals.push("source.arkui_decorator", "source", source, 22);
    }
    if [
        "Column()",
        "Row()",
        "Stack()",
        "Text(",
        "Button(",
        "RichEditor(",
    ]
    .iter()
    .any(|needle| text.contains(needle))
        || (text.contains("struct ") && text.contains("build()"))
    {
        signals.has_source = true;
        signals.push("source.arkui_dsl", "source", source, 18);
    }
    if text.contains("'@kit.") || text.contains("\"@kit.") {
        signals.has_source = true;
        signals.has_kit = true;
        signals.push("api.kit_import", "api", source, 25);
    }
    if text.contains("'@ohos.") || text.contains("\"@ohos.") {
        signals.has_source = true;
        signals.has_ohos = true;
        signals.push("api.ohos_import", "api", source, 25);
    }
    if lower.contains("arkts:error") || lower.contains("arktscheckerror") {
        signals.has_log = true;
        signals.push("log.arkts_error", "log", source, 55);
    }
    if lower.contains("hvigor error")
        || lower.contains("failed :") && lower.contains("compilearkts")
    {
        signals.has_log = true;
        signals.push("log.hvigor_error", "log", source, 45);
    }
    if lower.contains("/data/log/faultlog")
        || lower.contains("jscrash")
        || lower.contains("cppcrash")
    {
        signals.has_log = true;
        signals.push("log.harmony_fault", "log", source, 45);
    }

    if source.ends_with("module.json5") {
        if crate::services::harmony::parse_json5(text)
            .is_ok_and(|value| value.get("module").is_some())
        {
            signals.has_module = true;
            signals.push("project.module_manifest_content", "project", source, 35);
        } else {
            signals
                .conflicts
                .push(format!("invalid_module_manifest:{source}"));
        }
    }
}

fn collect_ets_files(current: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > MAX_DEPTH || out.len() >= MAX_SOURCE_FILES {
        return;
    }
    let Ok(entries) = std::fs::read_dir(current) else {
        return;
    };
    let mut entries = entries.filter_map(Result::ok).collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        if out.len() >= MAX_SOURCE_FILES {
            break;
        }
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            let name = entry.file_name();
            if !SKIP_DIRS.iter().any(|skip| name == *skip) {
                collect_ets_files(&path, depth + 1, out);
            }
        } else if kind.is_file() && path.extension().and_then(|value| value.to_str()) == Some("ets")
        {
            out.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "harmony-fingerprint-{name}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn combines_project_source_and_api_evidence() {
        let root = fixture("project");
        std::fs::create_dir_all(root.join("AppScope")).unwrap();
        std::fs::create_dir_all(root.join("entry/src/main/ets/pages")).unwrap();
        std::fs::write(
            root.join("AppScope/app.json5"),
            r#"{"app":{"bundleName":"com.example"}}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("build-profile.json5"),
            r#"{"app":{"products":[{"name":"default"}]},"modules":[{"name":"entry","srcPath":"./entry"}]}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("entry/src/main/ets/pages/Index.ets"),
            "import { router } from '@kit.ArkUI';\n@Entry\n@Component\nstruct Index { build() { Column() { Text('Hi') } } }",
        )
        .unwrap();
        let report = inspect_path(&root);
        assert_eq!(report.classification, "harmony_project");
        assert_eq!(report.api_style, "kit");
        assert_eq!(report.confidence, 100);
        assert!(report
            .evidence
            .iter()
            .any(|item| item.code == "source.arkui_decorator"));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn recognizes_snippets_and_logs_without_claiming_a_project() {
        let source = inspect_text(
            "import fs from '@ohos.file.fs';\n@Entry\n@Component\nstruct Index { build() { Text('x') } }",
            Some("Index.ets"),
        );
        assert_eq!(source.classification, "harmony_source");
        assert_eq!(source.api_style, "ohos");
        assert!(source.is_harmony());

        let log = inspect_text(
            "ERROR: [ArkTSCheckError] ArkTS:ERROR File: entry/src/main/ets/Index.ets:8:12",
            Some("build.log"),
        );
        assert_eq!(log.classification, "harmony_log");
        assert_eq!(log.recommended_capability_pack, "compile_fix");
    }

    #[test]
    fn generic_typescript_does_not_false_positive() {
        let report = inspect_text(
            "import React from 'react'; export function App() { return <main>Hello</main> }",
            Some("App.tsx"),
        );
        assert_eq!(report.classification, "unknown");
        assert_eq!(report.confidence, 0);
        assert!(!report.is_harmony());
    }

    #[test]
    fn confidence_counts_signal_diversity_and_invalid_manifests_conflict() {
        let many_ets = (0..20)
            .map(|index| format!("// file {index}"))
            .collect::<Vec<_>>()
            .join("\n");
        let source = inspect_text(&many_ets, Some("OnlyExtension.ets"));
        assert_eq!(source.confidence, 15);
        assert!(!source.is_harmony());

        let root = fixture("invalid");
        std::fs::create_dir_all(root.join("src/main")).unwrap();
        std::fs::write(root.join("src/main/module.json5"), "{ invalid").unwrap();
        let invalid = inspect_path(&root);
        assert_eq!(invalid.classification, "unknown");
        assert_eq!(invalid.confidence, 0);
        assert_eq!(
            invalid.conflicts,
            vec!["invalid_module_manifest:src/main/module.json5"]
        );
        std::fs::remove_dir_all(root).ok();
    }
}
