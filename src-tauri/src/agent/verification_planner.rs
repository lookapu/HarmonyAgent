//! 根据真实文件变更自动生成最小验证计划。

use serde::{Deserialize, Serialize};

use super::acceptance::ToolEvidence;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct VerificationStep {
    pub tool: String,
    pub reason: String,
    pub required: bool,
    #[serde(default)]
    pub completed: bool,
    #[serde(default)]
    pub evidence: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct VerificationPlan {
    pub changed_files: Vec<String>,
    pub steps: Vec<VerificationStep>,
}

pub fn plan(evidence: &[ToolEvidence<'_>]) -> VerificationPlan {
    let mut changed_files = evidence.iter().filter(|item| {
        item.succeeded && matches!(item.tool,
            "write_file" | "edit_file" | "delete_file" | "apply_patch" | "multi_edit" | "lsp_rename")
    }).flat_map(changed_paths).collect::<Vec<_>>();
    changed_files.sort();
    changed_files.dedup();
    let deleted_files = evidence.iter().filter(|item| item.succeeded)
        .flat_map(deleted_paths).collect::<Vec<_>>();
    let lsp_files = changed_files.iter().filter(|path| {
        path.to_ascii_lowercase().ends_with(".ets")
            && !deleted_files.iter().any(|deleted| same_path(deleted, path))
    }).cloned().collect::<Vec<_>>();
    let last_mutation = evidence.iter().enumerate().filter(|(_, item)| {
        item.succeeded && matches!(item.tool,
            "write_file" | "edit_file" | "delete_file" | "apply_patch" | "multi_edit" | "lsp_rename")
    }).map(|(index, _)| index).next_back();
    let mut steps = Vec::new();
    let mut add = |tool: &str, reason: &str, required: bool| {
        if !steps.iter().any(|item: &VerificationStep| item.tool == tool) {
            let (completed, proof) = completion(tool, &lsp_files, evidence, last_mutation);
            steps.push(VerificationStep {
                tool: tool.into(), reason: reason.into(), required,
                completed, evidence: proof,
            });
        }
    };
    let has = |extensions: &[&str]| changed_files.iter().any(|path| {
        extensions.iter().any(|extension| path.to_ascii_lowercase().ends_with(extension))
    });
    let harmony = has(&[".ets", ".json5"]) || changed_files.iter().any(|path| {
        path.ends_with("module.json5") || path.ends_with("build-profile.json5")
            || path.ends_with("oh-package.json5")
    });
    let code = has(&[".ets", ".ts", ".tsx", ".js", ".jsx", ".rs", ".java", ".kt", ".py", ".c", ".cpp", ".h"]);
    if has(&[".ets", ".ts", ".tsx", ".js", ".jsx", ".rs", ".java", ".kt", ".py", ".json", ".json5"]) {
        add("lsp_format", "按语言格式化变更文件，减少语法和风格噪声", false);
    }
    if has(&[".ets"]) {
        add("check_sdk_alignment", "ETS 变更必须用当前 product、本机 SDK 声明、权限和 SystemCapability 做一致性审计", true);
        if !lsp_files.is_empty() {
            add("lsp_diagnostics", "每个仍存在的变更 ETS 文件必须在最后一次写入后通过 ArkTS 语言服务类型检查", true);
        }
        add("run_lint", "ArkTS/ETS 变更需要静态规则检查", true);
    } else if code {
        add("check_code", "代码变更需要静态缺陷与敏感信息检查", true);
    }
    if code || has(&[".sql"]) {
        add("run_tests", "运行受影响测试，证明行为没有回归", true);
    }
    if harmony {
        add("build_project", "HarmonyOS 源码或工程配置变化需要 Hvigor 构建验证", true);
    } else if code || has(&["package.json", "cargo.toml", "pom.xml", "build.gradle"] ) {
        add("build_generic", "源码或构建配置变化需要工程构建/类型检查", true);
    }
    if !changed_files.is_empty() {
        add("git_diff", "核对最终差异范围与意外改动", true);
    }
    VerificationPlan { changed_files, steps }
}

fn completion(
    tool: &str,
    lsp_files: &[String],
    evidence: &[ToolEvidence<'_>],
    last_mutation: Option<usize>,
) -> (bool, Vec<String>) {
    let after = last_mutation.map(|index| index + 1).unwrap_or(0);
    let runs = evidence.iter().enumerate().skip(after)
        .filter(|(_, item)| item.succeeded && item.tool == tool).collect::<Vec<_>>();
    if tool == "lsp_diagnostics" {
        let mut proof = Vec::new();
        let all_clean = lsp_files.iter().all(|path| {
            let hit = runs.iter().find(|(_, item)| {
                evidence_path(item.args).is_some_and(|actual| same_path(&actual, path))
                    && item.output.contains("无诊断错误")
            });
            if let Some((index, _)) = hit {
                proof.push(format!("#{} {}", index + 1, path));
                true
            } else { false }
        });
        return (!lsp_files.is_empty() && all_clean, proof);
    }
    if tool == "check_sdk_alignment" {
        if let Some((index, _)) = runs.iter().find(|(_, item)| {
            item.output.contains("0 error")
                && !item.output.contains("sdk_index_unavailable")
                && (item.output.contains("状态：ok") || item.output.contains("状态：ahead"))
        }) {
            return (true, vec![format!("#{} SDK/一致性审计通过", index + 1)]);
        }
        return (false, Vec::new());
    }
    runs.last().map(|(index, _)| (true, vec![format!("#{} {tool}", index + 1)]))
        .unwrap_or((false, Vec::new()))
}

fn evidence_path(args: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(args).ok()?
        .get("path")?.as_str().map(str::to_string)
}

fn same_path(left: &str, right: &str) -> bool {
    let left = left.replace('\\', "/").trim_start_matches("./").to_ascii_lowercase();
    let right = right.replace('\\', "/").trim_start_matches("./").to_ascii_lowercase();
    left == right || left.ends_with(&format!("/{right}")) || right.ends_with(&format!("/{left}"))
}

fn changed_paths(evidence: &ToolEvidence<'_>) -> Vec<String> {
    let Ok(args) = serde_json::from_str::<serde_json::Value>(evidence.args) else {
        return Vec::new();
    };
    let mut paths = Vec::new();
    for key in ["path", "file", "from", "to"] {
        if let Some(path) = args.get(key).and_then(|value| value.as_str()) {
            paths.push(path.to_string());
        }
    }
    if let Some(edits) = args.get("edits").and_then(|value| value.as_array()) {
        paths.extend(edits.iter().filter_map(|edit| {
            edit.get("path").or_else(|| edit.get("file"))?.as_str().map(str::to_string)
        }));
    }
    for key in ["patch", "content"] {
        if let Some(patch) = args.get(key).and_then(|value| value.as_str()) {
            for line in patch.lines() {
                let path = line.strip_prefix("*** Update File: ")
                    .or_else(|| line.strip_prefix("*** Add File: "))
                    .or_else(|| line.strip_prefix("*** Delete File: "))
                    .or_else(|| line.strip_prefix("+++ b/"));
                if let Some(path) = path.map(str::trim).filter(|path| !path.is_empty()) {
                    paths.push(path.to_string());
                }
            }
        }
    }
    paths
}

fn deleted_paths(evidence: &ToolEvidence<'_>) -> Vec<String> {
    let Ok(args) = serde_json::from_str::<serde_json::Value>(evidence.args) else {
        return Vec::new();
    };
    let mut paths = Vec::new();
    if evidence.tool == "delete_file" {
        if let Some(path) = args.get("path").and_then(|value| value.as_str()) {
            paths.push(path.to_string());
        }
    }
    if let Some(patch) = args.get("patch").and_then(|value| value.as_str()) {
        paths.extend(patch.lines().filter_map(|line| {
            line.strip_prefix("*** Delete File: ").map(str::trim)
                .filter(|path| !path.is_empty()).map(str::to_string)
        }));
    }
    paths
}

impl VerificationPlan {
    pub fn pending_required(&self) -> Vec<&VerificationStep> {
        self.steps.iter().filter(|step| step.required && !step.completed).collect()
    }

    pub fn directive(&self) -> Option<String> {
        if self.changed_files.is_empty() {
            return None;
        }
        let steps = self.steps.iter().map(|step| format!(
            "- [{}] {}{}：{}{}",
            if step.completed { "已完成" } else { "待执行" },
            step.tool,
            if step.required { "（必需）" } else { "（建议）" },
            step.reason,
            if step.evidence.is_empty() { String::new() }
            else { format!("；证据 {}", step.evidence.join(", ")) },
        )).collect::<Vec<_>>().join("\n");
        Some(format!(
            "## 文件变更验证计划\n变更文件：{}\n按顺序执行：\n{}\n格式化成功不等于验收通过；至少完成所有必需检查，并在最后核对差异。",
            self.changed_files.join(", "), steps,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edit(path: &str) -> ToolEvidence<'_> {
        ToolEvidence {
            tool: "edit_file",
            args: Box::leak(format!(r#"{{"path":"{path}"}}"#).into_boxed_str()),
            output: "ok",
            succeeded: true,
        }
    }

    #[test]
    fn arkts_change_selects_format_lint_tests_build_and_diff() {
        let plan = plan(&[edit("entry/src/main/ets/pages/Index.ets")]);
        let tools = plan.steps.iter().map(|step| step.tool.as_str()).collect::<Vec<_>>();
        assert_eq!(tools, [
            "lsp_format", "check_sdk_alignment", "lsp_diagnostics", "run_lint",
            "run_tests", "build_project", "git_diff",
        ]);
        assert!(!plan.steps[0].required);
        assert!(plan.steps.iter().filter(|step| step.required).count() >= 6);
    }

    #[test]
    fn documentation_change_only_requires_diff_review() {
        let plan = plan(&[edit("docs/README.md")]);
        assert_eq!(plan.steps.iter().map(|step| step.tool.as_str()).collect::<Vec<_>>(), ["git_diff"]);
    }

    #[test]
    fn failed_edit_does_not_create_false_verification_scope() {
        let mut item = edit("src/main.rs");
        item.succeeded = false;
        assert!(plan(&[item]).changed_files.is_empty());
    }

    #[test]
    fn patch_headers_define_verification_scope() {
        let item = ToolEvidence {
            tool: "apply_patch",
            args: r#"{"patch":"*** Update File: src/lib.rs\n@@"}"#,
            output: "ok",
            succeeded: true,
        };
        assert_eq!(plan(&[item]).changed_files, ["src/lib.rs"]);
    }

    #[test]
    fn deleted_arkts_file_does_not_create_impossible_lsp_gate() {
        let item = ToolEvidence {
            tool: "delete_file",
            args: r#"{"path":"entry/src/main/ets/Legacy.ets"}"#,
            output: "deleted",
            succeeded: true,
        };
        let plan = plan(&[item]);
        assert!(plan.steps.iter().all(|step| step.tool != "lsp_diagnostics"));
        assert!(plan.steps.iter().any(|step| step.tool == "build_project"));
    }

    #[test]
    fn arkts_closure_requires_clean_sdk_lsp_and_build_after_last_write() {
        let items = [
            edit("entry/src/main/ets/pages/Index.ets"),
            ToolEvidence { tool: "check_sdk_alignment", args: "{}", output: "状态：ahead\n工程一致性审计：0 error / 0 warning / 0 info", succeeded: true },
            ToolEvidence { tool: "lsp_diagnostics", args: r#"{"path":"entry/src/main/ets/pages/Index.ets"}"#, output: "无诊断错误（文件通过类型检查）", succeeded: true },
            ToolEvidence { tool: "run_lint", args: "{}", output: "ok", succeeded: true },
            ToolEvidence { tool: "run_tests", args: "{}", output: "ok", succeeded: true },
            ToolEvidence { tool: "build_project", args: "{}", output: "BUILD SUCCESS", succeeded: true },
            ToolEvidence { tool: "git_diff", args: "{}", output: "diff", succeeded: true },
        ];
        assert!(plan(&items).pending_required().is_empty());
        let dirty_lsp = [items[0], ToolEvidence {
            output: "诊断结果（1 条）：[错误] Type mismatch", ..items[2]
        }];
        assert!(plan(&dirty_lsp).pending_required().iter()
            .any(|step| step.tool == "lsp_diagnostics"));

        let stale = [items[1], items[2], items[0]];
        let stale_plan = plan(&stale);
        assert!(stale_plan.pending_required().iter()
            .any(|step| step.tool == "check_sdk_alignment"));
        assert!(stale_plan.pending_required().iter()
            .any(|step| step.tool == "lsp_diagnostics"));
    }
}
