//! 根据真实文件变更自动生成最小验证计划。

use serde::{Deserialize, Serialize};

use super::acceptance::ToolEvidence;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct VerificationStep {
    pub tool: String,
    pub reason: String,
    pub required: bool,
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
    let mut steps = Vec::new();
    let mut add = |tool: &str, reason: &str, required: bool| {
        if !steps.iter().any(|item: &VerificationStep| item.tool == tool) {
            steps.push(VerificationStep { tool: tool.into(), reason: reason.into(), required });
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

impl VerificationPlan {
    pub fn directive(&self) -> Option<String> {
        if self.changed_files.is_empty() {
            return None;
        }
        let steps = self.steps.iter().map(|step| format!(
            "- {}{}：{}",
            step.tool,
            if step.required { "（必需）" } else { "（建议）" },
            step.reason,
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
        assert_eq!(tools, ["lsp_format", "run_lint", "run_tests", "build_project", "git_diff"]);
        assert!(!plan.steps[0].required);
        assert!(plan.steps.iter().filter(|step| step.required).count() >= 4);
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
}
