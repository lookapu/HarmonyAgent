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
        item.succeeded && is_mutation_tool(item.tool)
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
        item.succeeded && is_mutation_tool(item.tool)
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
    if tool == "run_lint" {
        // run_lint 只要 lint 工具跑通就返回 Ok，即使报告里写着「错误 (error)：37」——
        // 工具执行成功 ≠ 代码干净。沿用上面两条结论感知臂的口径：必须看到明确的干净结论。
        // 严重级过滤只筛 warning/info 的运行根本没统计 error，不能据此判定无错误。
        let Some((index, item)) = runs.iter().rev().find(|(_, item)| lint_covers_errors(item.args))
        else {
            return (false, Vec::new());
        };
        return match lint_error_count(&item.output) {
            Some(0) => (true, vec![format!("#{} run_lint 无 error 级问题", index + 1)]),
            Some(errors) => (false, vec![format!("#{} run_lint 仍有 {errors} 个 error 级问题", index + 1)]),
            None => (false, vec![format!("#{} run_lint 输出未给出 error 计数", index + 1)]),
        };
    }
    if tool == "check_code" {
        // check_code 同理：规则扫描命中再多也返回 Ok（「静态检查完成：… N 条命中」）。
        // 只把高危/中危当阻断项——debug-log、plaintext-http 这类提示/低危在任何真实
        // 仓库都会命中，一并阻断会让这个必需步骤永远无法完成。
        let Some((index, item)) = runs.last() else {
            return (false, Vec::new());
        };
        let Some(total) = scan_hit_count(&item.output) else {
            return (false, vec![format!("#{} check_code 输出未给出命中数", index + 1)]);
        };
        // 命中列表被截断时，高危分组可能整段没进输出，此时「没看到高危」不等于「没有高危」。
        if item.output.contains(SCAN_TRUNCATED) {
            return (false, vec![format!(
                "#{} check_code 输出被截断（共 {total} 条命中未完整列出），请缩小扫描范围后重跑",
                index + 1
            )]);
        }
        // 覆盖率不完整（目录/文件读不到、超 300 上限）与输出截断是同一类问题：
        // 结论的覆盖面小于它字面声称的范围。**读不到的文件上同样看不到高危命中**，
        // 所以「无高危」此时不成立。标记由 scanner::check_code 写入，两边共用同一常量。
        if item.output.contains(crate::agent::scanner::SCAN_INCOMPLETE) {
            return (false, vec![format!(
                "#{} check_code 扫描覆盖不完整（{}），本次「无高危」结论不覆盖这些文件；请修复访问权限或分目录重扫",
                index + 1,
                item.output
                    .lines()
                    .find(|line| line.contains(crate::agent::scanner::SCAN_INCOMPLETE))
                    .unwrap_or("原因未给出")
                    .split_once('：')
                    .map(|(_, reason)| reason.trim())
                    .unwrap_or("原因未给出")
            )]);
        }
        let blocking = scan_blocking_groups(&item.output);
        return if blocking == 0 {
            (true, vec![format!("#{} check_code 无高危/中危（共 {total} 条提示）", index + 1)])
        } else {
            (false, vec![format!("#{} check_code 仍有 {blocking} 组高危/中危规则命中", index + 1)])
        };
    }
    runs.last().map(|(index, _)| (true, vec![format!("#{} {tool}", index + 1)]))
        .unwrap_or((false, Vec::new()))
}

/// `scanner::cut` 在输出超长时追加的截断标记。
const SCAN_TRUNCATED: &str = "输出已截断";

/// run_lint 的 severity 过滤是否覆盖 error：空表示全量统计；只筛 warning 时报告里
/// 的「错误 (error)：0」是统计口径造成的空值，不是干净结论。
fn lint_covers_errors(args: &str) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(args) else {
        return false;
    };
    let severity = value.get("severity").and_then(|value| value.as_str())
        .unwrap_or("").trim().to_lowercase();
    severity.is_empty() || severity.contains("error")
}

/// 取 run_lint 报告里「错误 (error)：N」的 N。
fn lint_error_count(output: &str) -> Option<usize> {
    output.lines().find_map(|line| {
        line.trim().strip_prefix("错误 (error)：")
            .and_then(|rest| rest.trim().split_whitespace().next())
            .and_then(|count| count.parse::<usize>().ok())
    })
}

/// 取 check_code 报告里「扫描 N 个文件，M 条命中」的 M。
fn scan_hit_count(output: &str) -> Option<usize> {
    let (head, _) = output.lines().next()?.split_once("条命中")?;
    head.rsplit_once('，')?.1.trim().parse::<usize>().ok()
}

/// 统计 check_code 报告里处于阻断级别（高危/中危）的规则分组数。
fn scan_blocking_groups(output: &str) -> usize {
    output.lines().filter(|line| {
        let head = line.trim();
        head.starts_with("## [") && (head.contains("高危") || head.contains("中危"))
    }).count()
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

/// 会改变工作区文件的工具。**唯一真源**。
///
/// 验证范围（`plan`）与交付给用户的变更清单（`chat.rs` 的 `modified_files`）
/// 必须认同一份清单：前者决定「这次任务要验证哪些文件」，后者是用户实际看到的
/// 那张单子。两者一旦分叉，就会出现「验收认为改了 4 个文件、清单只列 1 个」，
/// 而用户正是拿这张清单去做最终验收的。
/// 审批侧（`tools::guards::pre_approval` 的 first_write 判定）同样复用本函数。
///
/// ⚠️ **加名字之前先确认它是个真工具**。`apply_patch` 在这里当过很久的写工具，
/// 但 `TOOL_SPECS` 里从来没有它（全树唯一的同名函数是评测 harness 的
/// `eval_patch::apply_patch`）——为它写的解析与修复全是死代码。
/// 核对命令（输出即工具全集）：
/// `rg -o --no-filename 'name: "[a-z0-9_]+"' src-tauri/src/agent/tools/mod.rs | Sort-Object -Unique`
pub(crate) fn is_mutation_tool(tool: &str) -> bool {
    matches!(tool, "write_file" | "edit_file" | "delete_file" | "multi_edit" | "lsp_rename")
}

fn changed_paths(evidence: &ToolEvidence<'_>) -> Vec<String> {
    paths_from_args(evidence.args)
}

/// 从工具参数里取出这次调用命中的文件路径（写工具专用）。
///
/// 覆盖三种形态：`path`/`file`/`from`/`to` 直给、`edits[]` 逐条、
/// 以及 `apply_patch` 的 `*** Update/Add/Delete File:` 与 `+++ b/` 补丁头。
///
/// **调用方不再各自写一份路径解析**——验证范围、任务账本、消息底部的文件列表
/// 消费的是同一批工具，解析规则一旦分叉就会出现「验收认为改了文件、
/// 交付清单里没有」这种对不上的状态。
///
/// `content` 只在调用**没有**给出路径时才当补丁扫：`write_file` 的 `content`
/// 是整份文件正文，写进去的内容里出现 `+++ b/` 这类行并不代表它改过那个文件，
/// 按补丁头解析只会凭空造出假变更。
pub(crate) fn paths_from_args(args_raw: &str) -> Vec<String> {
    let Ok(args) = serde_json::from_str::<serde_json::Value>(args_raw) else {
        return Vec::new();
    };
    let mut paths = Vec::new();
    for key in ["path", "file", "from", "to"] {
        if let Some(path) = args.get(key).and_then(|value| value.as_str()) {
            let path = path.trim();
            if !path.is_empty() {
                paths.push(path.to_string());
            }
        }
    }
    if let Some(edits) = args.get("edits").and_then(|value| value.as_array()) {
        paths.extend(edits.iter().filter_map(|edit| {
            let raw = edit.get("path").or_else(|| edit.get("file"))?.as_str()?.trim();
            (!raw.is_empty()).then(|| raw.to_string())
        }));
    }
    let names_target = !paths.is_empty();
    for key in ["patch", "content"] {
        if names_target && key == "content" {
            continue;
        }
        let Some(text) = args.get(key).and_then(|value| value.as_str()) else {
            continue;
        };
        paths.extend(patch_header_paths(text));
    }
    paths
}

/// 从 `apply_patch` 的补丁正文里取 `*** Update/Add/Delete File:` 与 `+++ b/` 后的路径。
///
/// 单独成函数是因为结构化结果信封（`structured_result::argument_artifacts`）也要用同一套
/// 补丁头解析：它原先只认「键名里带 path/file」的字段，而 `patch` 两个条件都不满足，
/// 于是 `apply_patch` 一次都产不出产物，最后落到「把整段 args 当路径」的兜底，
/// 写进 `side_effects` 与读回校验目标的是一段 JSON 原文。
pub(crate) fn patch_paths_from_args(args_raw: &str) -> Vec<String> {
    let Ok(args) = serde_json::from_str::<serde_json::Value>(args_raw) else {
        return Vec::new();
    };
    let Some(text) = args.get("patch").and_then(|value| value.as_str()) else {
        return Vec::new();
    };
    patch_header_paths(text)
}

fn patch_header_paths(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim_start();
            line.strip_prefix("*** Update File: ")
                .or_else(|| line.strip_prefix("*** Add File: "))
                .or_else(|| line.strip_prefix("*** Delete File: "))
                .or_else(|| line.strip_prefix("+++ b/"))
                .map(str::trim)
                .filter(|path| !path.is_empty())
                .map(str::to_string)
        })
        .collect()
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
    fn patch_headers_parse_from_args_and_never_override_named_target() {
        // 直接测 paths_from_args，而不是经由某个工具名。
        // 原用例挂在 `apply_patch` 上断言 changed_files，但 TOOL_SPECS 里从来没有
        // apply_patch——那是评测 harness 的 eval_patch::apply_patch 的同名误认。
        // 断言一条生产里走不到的路径，只会让「解析器可用」看起来像「接线正确」。
        assert_eq!(
            paths_from_args(r#"{"patch":"*** Update File: src/lib.rs\n@@"}"#),
            ["src/lib.rs"]
        );
        // 补丁正文不该盖掉调用方已经给出的目标路径
        assert_eq!(
            paths_from_args(
                r#"{"path":"real.rs","content":"*** Update File: ghost.rs\n"}"#
            ),
            ["real.rs"]
        );
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
            ToolEvidence { tool: "run_lint", args: "{}", output: "Lint 检查完成（工具：codelinter）\n共发现 0 个问题\n  错误 (error)：0\n  警告 (warn)：0\n  其他：0", succeeded: true },
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
