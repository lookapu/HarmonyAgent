//! 结构化目标契约与证据驱动验收：模型只能申请完成，运行内核负责裁决。

use serde::{Deserialize, Serialize};

pub const GOAL_CONTRACT_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CriterionKind { Mutation, Verification, Build, Tests, Deploy, GitCommit, GitPush }

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GoalCriterionSpec {
    pub id: String,
    pub label: String,
    pub kind: CriterionKind,
    pub required: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GoalContract {
    pub version: u32,
    pub original_goal: String,
    pub criteria: Vec<GoalCriterionSpec>,
    pub artifact_hints: Vec<String>,
    pub constraints: Vec<String>,
}

impl GoalContract {
    pub fn compile(goal: &str) -> Self {
        let goal = goal.trim();
        let lower = goal.to_lowercase();
        let has = |words: &[&str]| words.iter().any(|word| lower.contains(word));
        let mutation = has(&[
            "修复", "修改", "改写", "重构", "实现", "创建", "新建", "删除", "接入",
            "完善", "优化", "升级", "迁移", "推进", "fix", "implement", "refactor",
            "create", "update", "delete", "optimize", "migrate",
        ]);
        let mut criteria = Vec::new();
        let mut push = |id: &str, label: &str, kind| criteria.push(GoalCriterionSpec {
            id: id.into(), label: label.into(), kind, required: true,
        });
        if mutation {
            push("requested_change", "请求的变更已真实落地", CriterionKind::Mutation);
            push("change_verification", "变更后已读取、差异检查、构建或测试验证", CriterionKind::Verification);
        }
        if has(&["构建", "编译", "build", "compile"]) { push("build", "构建成功", CriterionKind::Build); }
        if has(&["测试", "test", "验证用例", "用例通过"]) { push("tests", "测试通过", CriterionKind::Tests); }
        if has(&["部署", "安装到设备", "运行到设备", "deploy"]) { push("deploy", "部署完成", CriterionKind::Deploy); }
        if has(&["提交", "commit"]) { push("git_commit", "变更已提交", CriterionKind::GitCommit); }
        if has(&["推送", "push"]) { push("git_push", "提交已推送到远端", CriterionKind::GitPush); }
        Self {
            version: GOAL_CONTRACT_VERSION,
            original_goal: goal.chars().take(2000).collect(),
            criteria,
            artifact_hints: extract_artifact_hints(goal),
            constraints: vec!["完成声明必须绑定真实工具证据".into(), "有副作用操作之后必须进行独立验证".into()],
        }
    }

    pub fn directive(&self) -> String {
        let items = self.criteria.iter()
            .map(|item| format!("- [{}] {}", item.id, item.label))
            .collect::<Vec<_>>().join("\n");
        format!(
            "## 任务验收契约 v{}（运行内核强制执行）\n原始目标：{}\n{}\n你只能在必需项都有真实工具证据后申请完成；缺项时继续调用工具，不得用自然语言宣称代替执行。",
            self.version, self.original_goal,
            if items.is_empty() { "- 纯问答：给出准确完整结论" } else { &items }
        )
    }
}

fn extract_artifact_hints(goal: &str) -> Vec<String> {
    let mut hints = goal.split(|c: char| c.is_whitespace() || "，。；、()（）[]【】'\"".contains(c))
        .map(|token| token.trim_matches(|c: char| ",.:;!?`".contains(c)))
        .filter(|token| token.len() > 2 && (token.contains('/') || token.contains('\\')
            || [".rs", ".ts", ".tsx", ".js", ".jsx", ".ets", ".json", ".sql", ".md"].iter().any(|ext| token.ends_with(ext))))
        .map(str::to_string).collect::<Vec<_>>();
    hints.sort(); hints.dedup(); hints.truncate(32); hints
}

pub struct ToolEvidence<'a> {
    pub tool: &'a str,
    pub args: &'a str,
    pub output: &'a str,
    pub succeeded: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AcceptanceCriterion {
    pub id: String,
    pub label: String,
    pub required: bool,
    pub passed: bool,
    pub evidence: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AcceptanceReport {
    pub contract_version: u32,
    pub passed: bool,
    pub criteria: Vec<AcceptanceCriterion>,
    pub blockers: Vec<String>,
    pub evidence_count: usize,
}

fn is_mutation(tool: &str) -> bool {
    matches!(tool, "write_file" | "edit_file" | "delete_file" | "apply_patch" | "create_project" | "git_merge" | "db_migrate")
}

fn is_command(e: &ToolEvidence<'_>, words: &[&str]) -> bool {
    e.tool == "run_command" && words.iter().any(|word| e.args.to_lowercase().contains(word))
}

fn matches_kind(kind: &CriterionKind, e: &ToolEvidence<'_>) -> bool {
    match kind {
        CriterionKind::Mutation => is_mutation(e.tool),
        CriterionKind::Verification => matches!(e.tool, "read_file" | "git_diff" | "git_status" | "build_project" | "build_generic" | "run_tests" | "test_project")
            || is_command(e, &["git diff", "git status", " test", "test ", "cargo check", "npm run build"]),
        CriterionKind::Build => matches!(e.tool, "build_project" | "build_hap" | "hvigor_build" | "build_generic")
            || is_command(e, &["build", "compile", "hvigor", "assemble", "cargo check"]),
        CriterionKind::Tests => matches!(e.tool, "run_tests" | "test_project") || is_command(e, &["test", "vitest", "pytest", "cargo test"]),
        CriterionKind::Deploy => matches!(e.tool, "deploy" | "install_launch" | "install_app") || is_command(e, &["deploy", "hdc install"]),
        CriterionKind::GitCommit => e.tool == "git_commit" || is_command(e, &["git commit"]),
        CriterionKind::GitPush => e.tool == "git_push" || is_command(e, &["git push"]),
    }
}

fn evidence_label(index: usize, e: &ToolEvidence<'_>) -> String {
    let target: String = e.args.split(['\n', ',']).next().unwrap_or("").trim().chars().take(100).collect();
    let outcome: String = e.output.lines().find(|line| !line.trim().is_empty()).unwrap_or("").chars().take(100).collect();
    format!("#{} {} {} {}", index + 1, e.tool, target, outcome).trim().to_string()
}

fn argument_targets(args: &str) -> Vec<String> {
    fn walk(value: &serde_json::Value, field: &str, out: &mut Vec<String>) {
        match value {
            serde_json::Value::String(text) => {
                let field = field.to_lowercase();
                if (field.contains("path") || field.contains("file") || field.contains("target"))
                    && (text.contains('/') || text.contains('\\') || text.rsplit_once('.').is_some())
                {
                    out.push(text.replace('\\', "/").to_lowercase());
                }
            }
            serde_json::Value::Array(items) => items.iter().for_each(|item| walk(item, field, out)),
            serde_json::Value::Object(map) => map.iter().for_each(|(key, item)| walk(item, key, out)),
            _ => {}
        }
    }
    let mut targets = Vec::new();
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(args) {
        walk(&value, "", &mut targets);
    } else if args.contains('/') || args.contains('\\') || args.rsplit_once('.').is_some() {
        targets.push(args.replace('\\', "/").to_lowercase());
    }
    targets.sort(); targets.dedup(); targets
}

fn is_global_verifier(e: &ToolEvidence<'_>) -> bool {
    matches!(e.tool, "git_diff" | "git_status" | "build_project" | "build_generic" | "run_tests" | "test_project")
        || is_command(e, &["git diff", "git status", " test", "test ", "cargo check", "npm run build"])
}

pub fn evaluate_contract(contract: &GoalContract, tool_runs: &[ToolEvidence<'_>]) -> AcceptanceReport {
    let last_mutation = tool_runs.iter().enumerate()
        .filter(|(_, evidence)| evidence.succeeded && is_mutation(evidence.tool))
        .map(|(index, _)| index).next_back();
    let mutation_targets = tool_runs.iter().filter(|item| item.succeeded && is_mutation(item.tool))
        .flat_map(|item| argument_targets(item.args)).collect::<Vec<_>>();
    let criteria = contract.criteria.iter().map(|spec| {
        let mut evidence = tool_runs.iter().enumerate()
            .filter(|(index, item)| item.succeeded && matches_kind(&spec.kind, item)
                && (spec.kind != CriterionKind::Verification || last_mutation.map(|mutation| *index > mutation).unwrap_or(false)))
            .map(|(index, item)| evidence_label(index, item)).collect::<Vec<_>>();
        if spec.kind == CriterionKind::Verification {
            let after = last_mutation.map(|index| &tool_runs[index + 1..]).unwrap_or(&[]);
            let global = after.iter().enumerate().find(|(_, item)| item.succeeded && is_global_verifier(item));
            if let Some((offset, item)) = global {
                evidence = vec![evidence_label(last_mutation.unwrap_or(0) + offset + 1, item)];
            } else if !mutation_targets.is_empty() {
                let reads = after.iter().filter(|item| item.succeeded && item.tool == "read_file")
                    .flat_map(|item| argument_targets(item.args)).collect::<Vec<_>>();
                let covers_all = mutation_targets.iter().all(|target| reads.iter().any(|read| {
                    read == target || read.ends_with(target) || target.ends_with(read)
                }));
                if !covers_all { evidence.clear(); }
            } else {
                // 无法从写工具参数识别产物时，普通读取不能冒充验证；要求全局验证器。
                evidence.clear();
            }
        }
        AcceptanceCriterion { id: spec.id.clone(), label: spec.label.clone(), required: spec.required, passed: !evidence.is_empty(), evidence }
    }).collect::<Vec<_>>();
    let blockers = criteria.iter().filter(|criterion| criterion.required && !criterion.passed)
        .map(|criterion| criterion.label.clone()).collect::<Vec<_>>();
    AcceptanceReport {
        contract_version: contract.version,
        passed: blockers.is_empty(),
        evidence_count: criteria.iter().map(|criterion| criterion.evidence.len()).sum(),
        criteria, blockers,
    }
}

#[cfg(test)]
pub fn evaluate(goal: &str, tool_runs: &[ToolEvidence<'_>]) -> AcceptanceReport {
    evaluate_contract(&GoalContract::compile(goal), tool_runs)
}

pub fn remediation_prompt(report: &AcceptanceReport) -> String {
    format!("（运行内核拒绝了本次完成申请。仍缺少：{}。请立即调用工具补齐并独立验证；不要重复总结，不得用文字声称代替真实执行。补齐后再以『✅ 任务已完成』申请验收。）", report.blockers.join("；"))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ev<'a>(tool: &'a str, args: &'a str, output: &'a str, succeeded: bool) -> ToolEvidence<'a> { ToolEvidence { tool, args, output, succeeded } }

    #[test]
    fn mutation_needs_post_change_verification() {
        let report = evaluate("全面修复对话逻辑", &[ev("edit_file", "chat.rs", "ok", true)]);
        assert_eq!(report.blockers, vec!["变更后已读取、差异检查、构建或测试验证"]);
    }

    #[test]
    fn verification_before_write_does_not_count() {
        assert!(!evaluate("修改文件", &[ev("read_file", "a.rs", "old", true), ev("edit_file", "a.rs", "ok", true)]).passed);
    }

    #[test]
    fn unrelated_read_after_write_does_not_count() {
        assert!(!evaluate("修改文件", &[
            ev("edit_file", r#"{"path":"src/a.rs"}"#, "ok", true),
            ev("read_file", r#"{"path":"src/b.rs"}"#, "other", true),
        ]).passed);
        assert!(evaluate("修改文件", &[
            ev("edit_file", r#"{"path":"src/a.rs"}"#, "ok", true),
            ev("read_file", r#"{"path":"src/a.rs"}"#, "new", true),
        ]).passed);
    }

    #[test]
    fn requested_evidence_passes_and_is_bound() {
        let report = evaluate("修复并构建", &[ev("edit_file", "chat.rs", "patched", true), ev("build_project", "{}", "BUILD SUCCESS", true)]);
        assert!(report.passed, "blockers={:?}", report.blockers);
        assert!(report.evidence_count >= 3);
    }

    #[test]
    fn git_push_is_a_distinct_requirement() {
        let report = evaluate("提交并推送", &[ev("git_commit", "{}", "committed", true), ev("git_push", "{}", "rejected", false)]);
        assert_eq!(report.blockers, vec!["提交已推送到远端"]);
    }
}
