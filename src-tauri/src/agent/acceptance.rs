//! 证据驱动的任务验收：模型只能申请完成，运行内核根据目标和真实工具证据裁决。

use serde::{Deserialize, Serialize};

pub struct ToolEvidence<'a> {
    pub tool: &'a str,
    pub args: &'a str,
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
    pub passed: bool,
    pub criteria: Vec<AcceptanceCriterion>,
    pub blockers: Vec<String>,
}

type EvidenceSpec<'a> = (&'a str, &'a [&'a str], &'a [&'a str], &'a [&'a str]);

pub fn evaluate(goal: &str, tool_runs: &[ToolEvidence<'_>]) -> AcceptanceReport {
    let evidence_for = |predicate: &dyn Fn(&ToolEvidence<'_>) -> bool| -> Vec<String> {
        tool_runs
            .iter()
            .filter(|item| item.succeeded && predicate(item))
            .map(|item| item.tool.to_string())
            .collect()
    };
    let mut criteria = Vec::new();
    let goal_lower = goal.to_lowercase();
    let mutation_requested = [
        "修复",
        "修改",
        "改写",
        "重构",
        "实现",
        "创建",
        "新建",
        "删除",
        "接入",
        "完善",
        "优化",
        "升级",
        "迁移",
        "fix",
        "implement",
        "refactor",
        "create",
        "update",
        "delete",
    ]
    .iter()
    .any(|word| goal_lower.contains(word));
    if mutation_requested {
        let evidence = evidence_for(&|item| {
            matches!(
                item.tool,
                "write_file"
                    | "edit_file"
                    | "delete_file"
                    | "apply_patch"
                    | "create_project"
                    | "git_commit"
                    | "git_merge"
                    | "db_migrate"
            )
        });
        criteria.push(AcceptanceCriterion {
            id: "requested_change".into(),
            label: "请求的变更已真实落地".into(),
            required: true,
            passed: !evidence.is_empty(),
            evidence,
        });
    }

    let specs: [EvidenceSpec<'_>; 3] = [
        (
            "build",
            &["构建", "编译", "build", "compile"],
            &["build_project", "build_hap", "hvigor_build"],
            &["build", "compile", "hvigor", "assemble"],
        ),
        (
            "tests",
            &["测试", "test", "验证用例"],
            &["run_tests", "test_project"],
            &["test", "vitest", "pytest", "cargo test"],
        ),
        (
            "deploy",
            &["部署", "安装到设备", "运行到设备", "deploy"],
            &["deploy", "install_launch", "install_app"],
            &["deploy", "hdc install"],
        ),
    ];
    for (id, words, tools, command_words) in specs {
        if !words.iter().any(|word| goal_lower.contains(word)) {
            continue;
        }
        let evidence = evidence_for(&|item| {
            tools.contains(&item.tool)
                || (item.tool == "run_command"
                    && command_words
                        .iter()
                        .any(|word| item.args.to_lowercase().contains(word)))
        });
        criteria.push(AcceptanceCriterion {
            id: id.into(),
            label: match id {
                "build" => "构建成功",
                "tests" => "测试通过",
                _ => "部署完成",
            }
            .into(),
            required: true,
            passed: !evidence.is_empty(),
            evidence,
        });
    }
    if !tool_runs.is_empty() {
        let evidence = evidence_for(&|_| true);
        criteria.push(AcceptanceCriterion {
            id: "execution_evidence".into(),
            label: "存在成功的真实工具结果".into(),
            required: true,
            passed: !evidence.is_empty(),
            evidence,
        });
    }
    let blockers = criteria
        .iter()
        .filter(|c| c.required && !c.passed)
        .map(|c| c.label.clone())
        .collect::<Vec<_>>();
    AcceptanceReport {
        passed: blockers.is_empty(),
        criteria,
        blockers,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ev<'a>(tool: &'a str, args: &'a str, succeeded: bool) -> ToolEvidence<'a> {
        ToolEvidence {
            tool,
            args,
            succeeded,
        }
    }
    #[test]
    fn mutation_needs_write() {
        assert!(!evaluate("全面修复对话逻辑", &[ev("read_file", "{}", true)]).passed);
    }
    #[test]
    fn explicit_build_needs_success() {
        let report = evaluate(
            "修改并构建",
            &[
                ev("edit_file", "{}", true),
                ev("build_project", "{}", false),
            ],
        );
        assert!(!report.passed);
    }
    #[test]
    fn requested_evidence_passes() {
        let report = evaluate(
            "修复并构建",
            &[ev("edit_file", "{}", true), ev("build_project", "{}", true)],
        );
        assert!(report.passed, "blockers={:?}", report.blockers);
    }
}
